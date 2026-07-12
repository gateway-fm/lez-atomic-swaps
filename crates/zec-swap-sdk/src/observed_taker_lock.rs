//! Maker-local observation and durable projection of the taker's first lock.

use lez_swap_core::{ChainProof, Participant, SwapDirection, SwapId};
use serde::{Deserialize, Serialize};

use crate::{
    AcceptedZecAgreementV1, CanonicalZcashOutputObservation, FirstLockProjectionCommit,
    FirstLockStepV1, ZcashObservationEvent, ZcashObservationEventRecordV1, ZecAgreementV1,
    revalidate_historical_event,
};

const OBSERVED_TAKER_FIRST_LOCK_SCHEMA_V1: u16 = 1;
const OBSERVED_TAKER_FIRST_LOCK_KIND_V1: &str = "maker_observed_taker_first_lock";

/// Stable result returned by an observation-only chain adapter.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TakerFirstLockObservationV1 {
    /// A fresh stable query proves the agreement-bound lock is absent.
    Absent,
    /// The query cannot prove stable presence or stable absence.
    Unstable,
    /// Provisional LEZ adapter assertion for the agreement-bound taker lock.
    ///
    /// This primitive form is rejected for a Zcash-funded first lock.
    Confirmed(ObservedTakerFirstLockEvidenceV1),
    /// Complete canonical LEZ escrow evidence validated from one stable RPC snapshot.
    CanonicalLez(Box<crate::CanonicalLezEscrowObservationV1>),
    /// Affirmative stable evidence that the prior canonical LEZ escrow was removed.
    LezRemoved(Box<crate::CanonicalLezEscrowRemovalV1>),
    /// One stable poll atomically removed prior LEZ evidence and found its replacement.
    LezReplaced {
        /// Affirmative evidence for the detached escrow.
        removed: Box<crate::CanonicalLezEscrowRemovalV1>,
        /// Complete canonical replacement evidence.
        canonical: Box<crate::CanonicalLezEscrowObservationV1>,
    },
    /// Complete canonical Zcash evidence validated from a stable node snapshot.
    CanonicalZcash(Box<CanonicalZcashOutputObservation>),
    /// Affirmative stable evidence that the prior canonical Zcash output was removed.
    ZcashRemoved(Box<crate::CanonicalZcashOutputRemoval>),
    /// One stable poll atomically removed prior evidence and found its replacement.
    ZcashReplaced {
        /// Affirmative evidence for the detached output.
        removed: Box<crate::CanonicalZcashOutputRemoval>,
        /// Complete canonical replacement evidence.
        canonical: Box<CanonicalZcashOutputObservation>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum ObservedTakerFirstLockEvidenceSourceV1 {
    AdapterAssertion,
    CanonicalLez(Box<crate::CanonicalLezEscrowObservationV1>),
    CanonicalLezRemoval(Box<crate::CanonicalLezEscrowRemovalV1>),
    CanonicalLezReplacement {
        removed: Box<crate::CanonicalLezEscrowRemovalV1>,
        canonical: Box<crate::CanonicalLezEscrowObservationV1>,
    },
    CanonicalZcash(Box<CanonicalZcashOutputObservation>),
    CanonicalZcashRemoval(Box<crate::CanonicalZcashOutputRemoval>),
    CanonicalZcashReplacement {
        removed: Box<crate::CanonicalZcashOutputRemoval>,
        canonical: Box<CanonicalZcashOutputObservation>,
    },
}

/// Versioned provisional LEZ evidence asserted by a typed observation adapter.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ObservedTakerFirstLockEvidenceV1 {
    schema_version: u16,
    step: FirstLockStepV1,
    transaction_id: Box<str>,
    confirmations: u32,
    source: ObservedTakerFirstLockEvidenceSourceV1,
}

impl ObservedTakerFirstLockEvidenceV1 {
    /// Constructs well-formed primitive evidence.
    ///
    /// Agreement direction and confirmation policy are checked when the maker
    /// constructs the transition. Zcash-funded transitions reject this
    /// primitive form and require complete canonical evidence instead.
    ///
    /// # Errors
    ///
    /// Rejects the initialization-only step, an invalid transaction ID, or
    /// zero confirmations.
    pub fn new(
        step: FirstLockStepV1,
        transaction_id: impl Into<Box<str>>,
        confirmations: u32,
    ) -> Result<Self, ObservedTakerFirstLockTransitionError> {
        if step == FirstLockStepV1::LezInitialize {
            return Err(ObservedTakerFirstLockTransitionError::NonFinalStep(step));
        }
        let transaction_id = transaction_id.into();
        ChainProof::new(transaction_id.clone(), confirmations)
            .map_err(ObservedTakerFirstLockTransitionError::Core)?;
        if confirmations == 0 {
            return Err(ObservedTakerFirstLockTransitionError::ZeroConfirmations);
        }
        Ok(Self {
            schema_version: OBSERVED_TAKER_FIRST_LOCK_SCHEMA_V1,
            step,
            transaction_id,
            confirmations,
            source: ObservedTakerFirstLockEvidenceSourceV1::AdapterAssertion,
        })
    }

    pub(crate) fn from_canonical_zcash(value: CanonicalZcashOutputObservation) -> Self {
        Self {
            schema_version: OBSERVED_TAKER_FIRST_LOCK_SCHEMA_V1,
            step: FirstLockStepV1::ZcashFund,
            transaction_id: value.transaction_id().to_string().into(),
            confirmations: value.confirmations().get(),
            source: ObservedTakerFirstLockEvidenceSourceV1::CanonicalZcash(Box::new(value)),
        }
    }

    pub(crate) fn from_canonical_lez(value: crate::CanonicalLezEscrowObservationV1) -> Self {
        Self {
            schema_version: OBSERVED_TAKER_FIRST_LOCK_SCHEMA_V1,
            step: FirstLockStepV1::LezFund,
            transaction_id: lez_transaction_id(value.transaction_id()).into(),
            confirmations: value.confirmations().get(),
            source: ObservedTakerFirstLockEvidenceSourceV1::CanonicalLez(Box::new(value)),
        }
    }

    pub(crate) fn from_canonical_lez_removal(value: crate::CanonicalLezEscrowRemovalV1) -> Self {
        Self {
            schema_version: OBSERVED_TAKER_FIRST_LOCK_SCHEMA_V1,
            step: FirstLockStepV1::LezFund,
            transaction_id: lez_transaction_id(value.previous().transaction_id()).into(),
            confirmations: value.previous().confirmations().get(),
            source: ObservedTakerFirstLockEvidenceSourceV1::CanonicalLezRemoval(Box::new(value)),
        }
    }

    pub(crate) fn from_canonical_lez_replacement(
        removed: crate::CanonicalLezEscrowRemovalV1,
        canonical: crate::CanonicalLezEscrowObservationV1,
    ) -> Self {
        Self {
            schema_version: OBSERVED_TAKER_FIRST_LOCK_SCHEMA_V1,
            step: FirstLockStepV1::LezFund,
            transaction_id: lez_transaction_id(canonical.transaction_id()).into(),
            confirmations: canonical.confirmations().get(),
            source: ObservedTakerFirstLockEvidenceSourceV1::CanonicalLezReplacement {
                removed: Box::new(removed),
                canonical: Box::new(canonical),
            },
        }
    }

    pub(crate) fn from_canonical_zcash_removal(value: crate::CanonicalZcashOutputRemoval) -> Self {
        Self {
            schema_version: OBSERVED_TAKER_FIRST_LOCK_SCHEMA_V1,
            step: FirstLockStepV1::ZcashFund,
            transaction_id: value.previous().transaction_id().to_string().into(),
            confirmations: value.previous().confirmations().get(),
            source: ObservedTakerFirstLockEvidenceSourceV1::CanonicalZcashRemoval(Box::new(value)),
        }
    }

    pub(crate) fn from_canonical_zcash_replacement(
        removed: crate::CanonicalZcashOutputRemoval,
        canonical: CanonicalZcashOutputObservation,
    ) -> Self {
        Self {
            schema_version: OBSERVED_TAKER_FIRST_LOCK_SCHEMA_V1,
            step: FirstLockStepV1::ZcashFund,
            transaction_id: canonical.transaction_id().to_string().into(),
            confirmations: canonical.confirmations().get(),
            source: ObservedTakerFirstLockEvidenceSourceV1::CanonicalZcashReplacement {
                removed: Box::new(removed),
                canonical: Box::new(canonical),
            },
        }
    }

    /// Primitive evidence schema.
    #[must_use]
    pub const fn schema_version(&self) -> u16 {
        self.schema_version
    }

    /// Agreement-selected final first-lock step.
    #[must_use]
    pub const fn step(&self) -> FirstLockStepV1 {
        self.step
    }

    /// Canonical chain transaction identifier.
    #[must_use]
    pub fn transaction_id(&self) -> &str {
        &self.transaction_id
    }

    /// Stable canonical confirmations.
    #[must_use]
    pub const fn confirmations(&self) -> u32 {
        self.confirmations
    }
}

/// Maker-local transition proving the taker's agreement-bound first lock.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ObservedTakerFirstLockTransitionV1 {
    schema_version: u16,
    swap_id: SwapId,
    agreement_commitment: [u8; 32],
    local_participant: Participant,
    predecessor_revision: u64,
    evidence: ObservedTakerFirstLockEvidenceV1,
}

impl ObservedTakerFirstLockTransitionV1 {
    pub(crate) fn from_active(
        agreement: &ZecAgreementV1,
        local_participant: Participant,
        predecessor_revision: u64,
        evidence: ObservedTakerFirstLockEvidenceV1,
    ) -> Result<Self, ObservedTakerFirstLockTransitionError> {
        if local_participant != Participant::Maker {
            return Err(ObservedTakerFirstLockTransitionError::WrongRole(
                local_participant,
            ));
        }
        let expected_step = taker_first_lock_step(agreement.direction());
        if evidence.step != expected_step {
            return Err(ObservedTakerFirstLockTransitionError::WrongChain {
                expected: expected_step,
                actual: evidence.step,
            });
        }
        validate_observed_source(agreement, expected_step, &evidence.source)?;
        let required = agreement
            .coordinator()
            .required_confirmations(Participant::Taker);
        if matches!(
            evidence.source,
            ObservedTakerFirstLockEvidenceSourceV1::AdapterAssertion
        ) && evidence.confirmations < required
        {
            return Err(
                ObservedTakerFirstLockTransitionError::InsufficientConfirmations {
                    required,
                    actual: evidence.confirmations,
                },
            );
        }
        Ok(Self {
            schema_version: OBSERVED_TAKER_FIRST_LOCK_SCHEMA_V1,
            swap_id: agreement.coordinator().id().clone(),
            agreement_commitment: *agreement.agreement_commitment(),
            local_participant,
            predecessor_revision,
            evidence,
        })
    }

    /// Transition schema.
    #[must_use]
    pub const fn schema_version(&self) -> u16 {
        self.schema_version
    }

    /// Signed application swap ID.
    #[must_use]
    pub const fn swap_id(&self) -> &SwapId {
        &self.swap_id
    }

    /// Exact agreement commitment.
    #[must_use]
    pub const fn agreement_commitment(&self) -> &[u8; 32] {
        &self.agreement_commitment
    }

    /// Role whose independent store owns this transition.
    #[must_use]
    pub const fn local_participant(&self) -> Participant {
        self.local_participant
    }

    /// Aggregate revision immediately before this transition.
    #[must_use]
    pub const fn predecessor_revision(&self) -> u64 {
        self.predecessor_revision
    }

    /// Fresh canonical adapter assertion.
    #[must_use]
    pub const fn evidence(&self) -> &ObservedTakerFirstLockEvidenceV1 {
        &self.evidence
    }

    /// Complete canonical Zcash event retained by this transition, when any.
    ///
    /// Provisional reverse-LEZ assertions have no Zcash event.
    #[must_use]
    pub fn zcash_observation_event(&self) -> Option<ZcashObservationEvent> {
        match &self.evidence.source {
            ObservedTakerFirstLockEvidenceSourceV1::AdapterAssertion
            | ObservedTakerFirstLockEvidenceSourceV1::CanonicalLez(_)
            | ObservedTakerFirstLockEvidenceSourceV1::CanonicalLezRemoval(_)
            | ObservedTakerFirstLockEvidenceSourceV1::CanonicalLezReplacement { .. } => None,
            ObservedTakerFirstLockEvidenceSourceV1::CanonicalZcash(canonical) => {
                Some(ZcashObservationEvent::Canonical(canonical.as_ref().clone()))
            }
            ObservedTakerFirstLockEvidenceSourceV1::CanonicalZcashRemoval(removed) => {
                Some(ZcashObservationEvent::Removed(removed.as_ref().clone()))
            }
            ObservedTakerFirstLockEvidenceSourceV1::CanonicalZcashReplacement {
                removed,
                canonical,
            } => Some(ZcashObservationEvent::Replaced {
                removed: removed.clone(),
                canonical: canonical.clone(),
            }),
        }
    }

    /// Complete canonical LEZ event retained by this transition, when any.
    #[must_use]
    pub fn lez_observation_event(&self) -> Option<crate::LezObservationEventV1> {
        match &self.evidence.source {
            ObservedTakerFirstLockEvidenceSourceV1::CanonicalLez(canonical) => Some(
                crate::LezObservationEventV1::Canonical(canonical.as_ref().clone()),
            ),
            ObservedTakerFirstLockEvidenceSourceV1::CanonicalLezRemoval(removed) => Some(
                crate::LezObservationEventV1::Removed(removed.as_ref().clone()),
            ),
            ObservedTakerFirstLockEvidenceSourceV1::CanonicalLezReplacement {
                removed,
                canonical,
            } => Some(crate::LezObservationEventV1::Replaced {
                removed: removed.clone(),
                canonical: canonical.clone(),
            }),
            _ => None,
        }
    }

    /// Applies this trusted transition to the exact predecessor coordinator.
    ///
    /// The input is cloned before either half of an atomic replacement is
    /// applied, so an invalid replacement never mutates caller state.
    ///
    /// # Errors
    ///
    /// Rejects context, revision, confirmation, removal, replacement, and core
    /// lifecycle conflicts.
    pub fn apply_to(
        &self,
        agreement: &ZecAgreementV1,
        coordinator: &lez_swap_core::SwapCoordinator,
        revision: u64,
    ) -> Result<lez_swap_core::SwapCoordinator, ObservedTakerFirstLockTransitionError> {
        if self.schema_version != OBSERVED_TAKER_FIRST_LOCK_SCHEMA_V1
            || self.swap_id != *agreement.coordinator().id()
            || self.agreement_commitment != *agreement.agreement_commitment()
            || self.local_participant != Participant::Maker
            || self.predecessor_revision != revision
            || coordinator.id() != &self.swap_id
            || self.evidence.step != taker_first_lock_step(agreement.direction())
        {
            return Err(ObservedTakerFirstLockTransitionError::ContextMismatch);
        }
        let required = coordinator.required_confirmations(Participant::Taker);
        if matches!(
            self.evidence.source,
            ObservedTakerFirstLockEvidenceSourceV1::AdapterAssertion
        ) && self.evidence.confirmations < required
        {
            return Err(
                ObservedTakerFirstLockTransitionError::InsufficientConfirmations {
                    required,
                    actual: self.evidence.confirmations,
                },
            );
        }
        let mut next = coordinator.clone();
        match &self.evidence.source {
            ObservedTakerFirstLockEvidenceSourceV1::CanonicalLezRemoval(removed) => next
                .observe_funding_removed(
                    Participant::Taker,
                    &lez_transaction_id(removed.previous().transaction_id()),
                )
                .map_err(ObservedTakerFirstLockTransitionError::Core)?,
            ObservedTakerFirstLockEvidenceSourceV1::CanonicalLezReplacement {
                removed,
                canonical,
            } => {
                next.observe_funding_removed(
                    Participant::Taker,
                    &lez_transaction_id(removed.previous().transaction_id()),
                )
                .map_err(ObservedTakerFirstLockTransitionError::Core)?;
                next.observe_funding(
                    Participant::Taker,
                    ChainProof::new(
                        lez_transaction_id(canonical.transaction_id()),
                        canonical.confirmations().get(),
                    )
                    .map_err(ObservedTakerFirstLockTransitionError::Core)?,
                )
                .map_err(ObservedTakerFirstLockTransitionError::Core)?;
            }
            ObservedTakerFirstLockEvidenceSourceV1::CanonicalZcashRemoval(removed) => next
                .observe_funding_removed(
                    Participant::Taker,
                    &removed.previous().transaction_id().to_string(),
                )
                .map_err(ObservedTakerFirstLockTransitionError::Core)?,
            ObservedTakerFirstLockEvidenceSourceV1::CanonicalZcashReplacement {
                removed,
                canonical,
            } => {
                next.observe_funding_removed(
                    Participant::Taker,
                    &removed.previous().transaction_id().to_string(),
                )
                .map_err(ObservedTakerFirstLockTransitionError::Core)?;
                next.observe_funding(
                    Participant::Taker,
                    ChainProof::new(
                        canonical.transaction_id().to_string(),
                        canonical.confirmations().get(),
                    )
                    .map_err(ObservedTakerFirstLockTransitionError::Core)?,
                )
                .map_err(ObservedTakerFirstLockTransitionError::Core)?;
            }
            _ => next
                .observe_funding(
                    Participant::Taker,
                    ChainProof::new(
                        self.evidence.transaction_id.clone(),
                        self.evidence.confirmations,
                    )
                    .map_err(ObservedTakerFirstLockTransitionError::Core)?,
                )
                .map_err(ObservedTakerFirstLockTransitionError::Core)?,
        }
        Ok(next)
    }
}

fn validate_observed_source(
    agreement: &ZecAgreementV1,
    expected_step: FirstLockStepV1,
    source: &ObservedTakerFirstLockEvidenceSourceV1,
) -> Result<(), ObservedTakerFirstLockTransitionError> {
    match (expected_step, source) {
        (
            FirstLockStepV1::ZcashFund,
            ObservedTakerFirstLockEvidenceSourceV1::CanonicalZcash(canonical),
        ) => validate_canonical_zcash_binding(agreement, canonical),
        (
            FirstLockStepV1::ZcashFund,
            ObservedTakerFirstLockEvidenceSourceV1::CanonicalZcashRemoval(removed),
        ) => validate_canonical_zcash_binding(agreement, removed.previous()),
        (
            FirstLockStepV1::ZcashFund,
            ObservedTakerFirstLockEvidenceSourceV1::CanonicalZcashReplacement {
                removed,
                canonical,
            },
        ) => {
            validate_canonical_zcash_binding(agreement, removed.previous())?;
            validate_canonical_zcash_binding(agreement, canonical)?;
            if removed.tip_block_hash() != canonical.tip_block_hash()
                || removed.tip_height() != canonical.tip_height()
            {
                Err(ObservedTakerFirstLockTransitionError::CanonicalZcashReplacementTipMismatch)
            } else {
                Ok(())
            }
        }
        (FirstLockStepV1::ZcashFund, ObservedTakerFirstLockEvidenceSourceV1::AdapterAssertion) => {
            Err(ObservedTakerFirstLockTransitionError::CanonicalZcashRequired)
        }
        (
            FirstLockStepV1::ZcashFund,
            ObservedTakerFirstLockEvidenceSourceV1::CanonicalLez(_)
            | ObservedTakerFirstLockEvidenceSourceV1::CanonicalLezRemoval(_)
            | ObservedTakerFirstLockEvidenceSourceV1::CanonicalLezReplacement { .. },
        ) => Err(ObservedTakerFirstLockTransitionError::UnexpectedCanonicalLez),
        (
            FirstLockStepV1::LezFund,
            ObservedTakerFirstLockEvidenceSourceV1::CanonicalLez(canonical),
        ) => validate_canonical_lez_binding(agreement, canonical),
        (
            FirstLockStepV1::LezFund,
            ObservedTakerFirstLockEvidenceSourceV1::CanonicalLezRemoval(removed),
        ) => validate_canonical_lez_removal_binding(agreement, removed),
        (
            FirstLockStepV1::LezFund,
            ObservedTakerFirstLockEvidenceSourceV1::CanonicalLezReplacement { removed, canonical },
        ) => {
            validate_canonical_lez_removal_binding(agreement, removed)?;
            validate_canonical_lez_binding(agreement, canonical)?;
            if removed.tip_block_hash() != canonical.tip_block_hash()
                || removed.tip_height() != canonical.tip_height()
            {
                Err(ObservedTakerFirstLockTransitionError::CanonicalLezReplacementTipMismatch)
            } else {
                Ok(())
            }
        }
        (FirstLockStepV1::LezFund, ObservedTakerFirstLockEvidenceSourceV1::AdapterAssertion) => {
            Err(ObservedTakerFirstLockTransitionError::CanonicalLezRequired)
        }
        (
            FirstLockStepV1::LezFund,
            ObservedTakerFirstLockEvidenceSourceV1::CanonicalZcash(_)
            | ObservedTakerFirstLockEvidenceSourceV1::CanonicalZcashRemoval(_)
            | ObservedTakerFirstLockEvidenceSourceV1::CanonicalZcashReplacement { .. },
        ) => Err(ObservedTakerFirstLockTransitionError::UnexpectedCanonicalZcash),
        (FirstLockStepV1::LezInitialize, _) => Err(
            ObservedTakerFirstLockTransitionError::NonFinalStep(expected_step),
        ),
    }
}

fn validate_canonical_lez_binding(
    agreement: &ZecAgreementV1,
    canonical: &crate::CanonicalLezEscrowObservationV1,
) -> Result<(), ObservedTakerFirstLockTransitionError> {
    let rebound = crate::CanonicalLezEscrowObservationV1::validate(agreement, canonical.snapshot())
        .map_err(|_| ObservedTakerFirstLockTransitionError::CanonicalLezBindingMismatch)?;
    if &rebound == canonical {
        Ok(())
    } else {
        Err(ObservedTakerFirstLockTransitionError::CanonicalLezBindingMismatch)
    }
}

fn validate_canonical_lez_removal_binding(
    agreement: &ZecAgreementV1,
    removed: &crate::CanonicalLezEscrowRemovalV1,
) -> Result<(), ObservedTakerFirstLockTransitionError> {
    let previous =
        crate::CanonicalLezEscrowObservationV1::validate(agreement, removed.previous().snapshot())
            .map_err(|_| ObservedTakerFirstLockTransitionError::CanonicalLezBindingMismatch)?;
    let rebound = crate::CanonicalLezEscrowRemovalV1::validate(&previous, removed.snapshot())
        .map_err(|_| ObservedTakerFirstLockTransitionError::CanonicalLezBindingMismatch)?;
    if &rebound == removed {
        Ok(())
    } else {
        Err(ObservedTakerFirstLockTransitionError::CanonicalLezBindingMismatch)
    }
}

/// One safe maker polling outcome.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ObserveTakerFirstLockOutcome {
    /// No stable confirmed lock was projected.
    AwaitingStableObservation(FirstLockStepV1),
    /// Stable evidence exactly matched the current canonical tracker head.
    Unchanged(FirstLockStepV1),
    /// Exact evidence was committed before in-memory projection.
    Projected(FirstLockProjectionCommit),
}

/// Result of a distinct fresh canonical check immediately before maker funding.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MakerFundingEligibilityOutcome {
    /// The exact current canonical tracker head was freshly observed.
    Eligible {
        /// Durable role-local revision bound to this ephemeral eligibility.
        revision: u64,
    },
    /// No eligible exact-head observation was available; no state changed.
    AwaitingStableObservation(FirstLockStepV1),
    /// Fresh chain truth changed and was durably projected; poll again.
    CanonicalStateChanged(FirstLockProjectionCommit),
    /// Canonical evidence exists but remains below the signed threshold.
    AwaitingConfirmations,
    /// Public LEZ depth is sufficient but Bedrock finality is not yet final.
    AwaitingLezFinality(crate::LezInclusionStatusV1),
}

/// Failure to validate or apply maker-local taker-lock evidence.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum ObservedTakerFirstLockTransitionError {
    /// LEZ initialization does not prove funded escrow.
    #[error("{0:?} is not a final taker-lock step")]
    NonFinalStep(FirstLockStepV1),
    /// Evidence must contain at least one confirmation.
    #[error("observed taker-lock evidence has zero confirmations")]
    ZeroConfirmations,
    /// Forward Zcash projection requires the complete canonical observation.
    #[error("maker Zcash observation requires complete canonical evidence")]
    CanonicalZcashRequired,
    /// Reverse LEZ projection requires the complete canonical escrow observation.
    #[error("maker LEZ observation requires complete canonical escrow evidence")]
    CanonicalLezRequired,
    /// Canonical LEZ snapshot differs from the signed agreement binding.
    #[error("canonical LEZ evidence does not match the signed agreement")]
    CanonicalLezBindingMismatch,
    /// Canonical Zcash evidence cannot prove a LEZ-funded first lock.
    #[error("canonical Zcash evidence is invalid for a LEZ-funded first lock")]
    UnexpectedCanonicalZcash,
    /// Canonical LEZ evidence cannot prove a Zcash-funded first lock.
    #[error("canonical LEZ evidence is invalid for a Zcash-funded first lock")]
    UnexpectedCanonicalLez,
    /// Canonical Zcash evidence differs from the signed agreement binding.
    #[error("canonical Zcash evidence does not match the signed agreement")]
    CanonicalZcashBindingMismatch,
    /// Atomic replacement halves were not derived from one stable node tip.
    #[error("canonical Zcash replacement halves use different stable tips")]
    CanonicalZcashReplacementTipMismatch,
    /// Atomic LEZ replacement halves were not derived from one stable node tip.
    #[error("canonical LEZ replacement halves use different stable tips")]
    CanonicalLezReplacementTipMismatch,
    /// Only the maker owns this observation transition.
    #[error("observed taker-lock transition requires maker; actual role is {0:?}")]
    WrongRole(Participant),
    /// Evidence belongs to the other chain.
    #[error("observed taker-lock step is {actual:?}; expected {expected:?}")]
    WrongChain {
        /// Agreement-derived first-lock step.
        expected: FirstLockStepV1,
        /// Adapter-supplied step.
        actual: FirstLockStepV1,
    },
    /// Stable evidence is below the signed taker threshold.
    #[error("observed taker lock has {actual} confirmations; {required} required")]
    InsufficientConfirmations {
        /// Signed threshold.
        required: u32,
        /// Observed depth.
        actual: u32,
    },
    /// Durable transition fields do not match active agreement context.
    #[error("observed taker-lock transition context mismatch")]
    ContextMismatch,
    /// Core rejected the canonical observation.
    #[error(transparent)]
    Core(lez_swap_core::Error),
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
enum LezObservationChangeRecordV1 {
    Removed {
        previous: Box<crate::LezNodeSnapshotV1>,
        removal: crate::LezNodeRemovalSnapshotV1,
    },
    Replaced {
        previous: Box<crate::LezNodeSnapshotV1>,
        removal: crate::LezNodeRemovalSnapshotV1,
        canonical: Box<crate::LezNodeSnapshotV1>,
    },
}

/// Primitive untrusted persistence record for a maker observation transition.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ObservedTakerFirstLockTransitionRecordV1 {
    schema_version: u16,
    transition_kind: Box<str>,
    swap_id: Box<str>,
    agreement_commitment: [u8; 32],
    local_participant: Box<str>,
    predecessor_revision: u64,
    evidence_schema_version: u16,
    step: Box<str>,
    transaction_id: Box<str>,
    confirmations: u32,
    #[serde(default)]
    lez_canonical: Option<crate::LezNodeSnapshotV1>,
    #[serde(default)]
    lez_change: Option<LezObservationChangeRecordV1>,
    zcash_canonical: Option<ZcashObservationEventRecordV1>,
}

impl From<&ObservedTakerFirstLockTransitionV1> for ObservedTakerFirstLockTransitionRecordV1 {
    fn from(value: &ObservedTakerFirstLockTransitionV1) -> Self {
        Self {
            schema_version: value.schema_version,
            transition_kind: OBSERVED_TAKER_FIRST_LOCK_KIND_V1.into(),
            swap_id: value.swap_id.as_str().into(),
            agreement_commitment: value.agreement_commitment,
            local_participant: "maker".into(),
            predecessor_revision: value.predecessor_revision,
            evidence_schema_version: value.evidence.schema_version,
            step: step_name(value.evidence.step).into(),
            transaction_id: value.evidence.transaction_id.clone(),
            confirmations: value.evidence.confirmations,
            lez_canonical: match &value.evidence.source {
                ObservedTakerFirstLockEvidenceSourceV1::CanonicalLez(canonical) => {
                    Some(canonical.snapshot().clone())
                }
                _ => None,
            },
            lez_change: match &value.evidence.source {
                ObservedTakerFirstLockEvidenceSourceV1::CanonicalLezRemoval(removed) => {
                    Some(LezObservationChangeRecordV1::Removed {
                        previous: Box::new(removed.previous().snapshot().clone()),
                        removal: *removed.snapshot(),
                    })
                }
                ObservedTakerFirstLockEvidenceSourceV1::CanonicalLezReplacement {
                    removed,
                    canonical,
                } => Some(LezObservationChangeRecordV1::Replaced {
                    previous: Box::new(removed.previous().snapshot().clone()),
                    removal: *removed.snapshot(),
                    canonical: Box::new(canonical.snapshot().clone()),
                }),
                _ => None,
            },
            zcash_canonical: match &value.evidence.source {
                ObservedTakerFirstLockEvidenceSourceV1::AdapterAssertion
                | ObservedTakerFirstLockEvidenceSourceV1::CanonicalLez(_)
                | ObservedTakerFirstLockEvidenceSourceV1::CanonicalLezRemoval(_)
                | ObservedTakerFirstLockEvidenceSourceV1::CanonicalLezReplacement { .. } => None,
                ObservedTakerFirstLockEvidenceSourceV1::CanonicalZcash(canonical) => Some(
                    ZcashObservationEventRecordV1::from_canonical(canonical.as_ref()),
                ),
                ObservedTakerFirstLockEvidenceSourceV1::CanonicalZcashRemoval(removed) => {
                    Some(ZcashObservationEventRecordV1::from_event(
                        &ZcashObservationEvent::Removed(removed.as_ref().clone()),
                    ))
                }
                ObservedTakerFirstLockEvidenceSourceV1::CanonicalZcashReplacement {
                    removed,
                    canonical,
                } => Some(ZcashObservationEventRecordV1::from_event(
                    &ZcashObservationEvent::Replaced {
                        removed: removed.clone(),
                        canonical: canonical.clone(),
                    },
                )),
            },
        }
    }
}

impl ObservedTakerFirstLockTransitionRecordV1 {
    /// Primitive payload version.
    #[must_use]
    pub const fn schema_version(&self) -> u16 {
        self.schema_version
    }

    /// Reconstructs a trusted transition against an independently resumed agreement.
    ///
    /// # Errors
    ///
    /// Rejects schema, identity, commitment, role, revision, chain, depth, and
    /// transaction-evidence mismatches.
    pub fn revalidate(
        &self,
        accepted: &AcceptedZecAgreementV1,
        predecessor_revision: u64,
    ) -> Result<ObservedTakerFirstLockTransitionV1, ObservedTakerFirstLockTransitionError> {
        if self.schema_version != OBSERVED_TAKER_FIRST_LOCK_SCHEMA_V1
            || self.transition_kind.as_ref() != OBSERVED_TAKER_FIRST_LOCK_KIND_V1
            || self.evidence_schema_version != OBSERVED_TAKER_FIRST_LOCK_SCHEMA_V1
            || self.swap_id.as_ref() != accepted.agreement().coordinator().id().as_str()
            || self.agreement_commitment != *accepted.agreement().agreement_commitment()
            || self.local_participant.as_ref() != "maker"
            || accepted.local_participant() != Participant::Maker
            || self.predecessor_revision != predecessor_revision
        {
            return Err(ObservedTakerFirstLockTransitionError::ContextMismatch);
        }
        let evidence = match (&self.lez_canonical, &self.lez_change, &self.zcash_canonical) {
            (None, None, None) => ObservedTakerFirstLockEvidenceV1::new(
                parse_step(&self.step)?,
                self.transaction_id.clone(),
                self.confirmations,
            )?,
            (Some(snapshot), None, None) => {
                let canonical = crate::CanonicalLezEscrowObservationV1::validate(
                    accepted.agreement(),
                    snapshot,
                )
                .map_err(|_| ObservedTakerFirstLockTransitionError::CanonicalLezBindingMismatch)?;
                ObservedTakerFirstLockEvidenceV1::from_canonical_lez(canonical)
            }
            (None, Some(change), None) => revalidate_lez_change(accepted.agreement(), change)?,
            (None, None, Some(record)) => {
                match revalidate_historical_event(record).map_err(|_| {
                    ObservedTakerFirstLockTransitionError::CanonicalZcashBindingMismatch
                })? {
                    ZcashObservationEvent::Canonical(canonical) => {
                        ObservedTakerFirstLockEvidenceV1::from_canonical_zcash(canonical)
                    }
                    ZcashObservationEvent::Removed(removed) => {
                        ObservedTakerFirstLockEvidenceV1::from_canonical_zcash_removal(removed)
                    }
                    ZcashObservationEvent::Replaced { removed, canonical } => {
                        ObservedTakerFirstLockEvidenceV1::from_canonical_zcash_replacement(
                            *removed, *canonical,
                        )
                    }
                }
            }
            _ => {
                return Err(ObservedTakerFirstLockTransitionError::ContextMismatch);
            }
        };
        let trusted = ObservedTakerFirstLockTransitionV1::from_active(
            accepted.agreement(),
            Participant::Maker,
            predecessor_revision,
            evidence,
        )?;
        if Self::from(&trusted) != *self {
            return Err(ObservedTakerFirstLockTransitionError::ContextMismatch);
        }
        Ok(trusted)
    }
}

fn revalidate_lez_change(
    agreement: &ZecAgreementV1,
    change: &LezObservationChangeRecordV1,
) -> Result<ObservedTakerFirstLockEvidenceV1, ObservedTakerFirstLockTransitionError> {
    match change {
        LezObservationChangeRecordV1::Removed { previous, removal } => {
            let previous = crate::CanonicalLezEscrowObservationV1::validate(agreement, previous)
                .map_err(|_| ObservedTakerFirstLockTransitionError::CanonicalLezBindingMismatch)?;
            let removed = crate::CanonicalLezEscrowRemovalV1::validate(&previous, removal)
                .map_err(|_| ObservedTakerFirstLockTransitionError::CanonicalLezBindingMismatch)?;
            Ok(ObservedTakerFirstLockEvidenceV1::from_canonical_lez_removal(removed))
        }
        LezObservationChangeRecordV1::Replaced {
            previous,
            removal,
            canonical,
        } => {
            let previous = crate::CanonicalLezEscrowObservationV1::validate(agreement, previous)
                .map_err(|_| ObservedTakerFirstLockTransitionError::CanonicalLezBindingMismatch)?;
            let removed = crate::CanonicalLezEscrowRemovalV1::validate(&previous, removal)
                .map_err(|_| ObservedTakerFirstLockTransitionError::CanonicalLezBindingMismatch)?;
            let canonical = crate::CanonicalLezEscrowObservationV1::validate(agreement, canonical)
                .map_err(|_| ObservedTakerFirstLockTransitionError::CanonicalLezBindingMismatch)?;
            Ok(
                ObservedTakerFirstLockEvidenceV1::from_canonical_lez_replacement(
                    removed, canonical,
                ),
            )
        }
    }
}

fn lez_transaction_id(transaction_id: &[u8; 32]) -> String {
    let mut encoded = String::with_capacity(64);
    for byte in transaction_id {
        use std::fmt::Write as _;
        write!(&mut encoded, "{byte:02x}").expect("writing to String cannot fail");
    }
    encoded
}

fn validate_canonical_zcash_binding(
    agreement: &ZecAgreementV1,
    canonical: &CanonicalZcashOutputObservation,
) -> Result<(), ObservedTakerFirstLockTransitionError> {
    // Remote acceptance binds the consensus-valid canonical output. Funding
    // inputs, change, fee target, and expiry are role-local construction policy:
    // they are validated when this SDK builds its own transaction and are not
    // required disclosures from a counterparty wallet.
    let expected = agreement.binding().expected_output();
    if canonical.network() != expected.network()
        || canonical.consensus_branch_id() != expected.consensus_branch_id()
        || canonical.output().value() != expected.value()
        || canonical.redeem_script() != expected.contract().redeem_script()
        || canonical.p2sh_script_pubkey() != expected.contract().p2sh_script_pubkey()
    {
        return Err(ObservedTakerFirstLockTransitionError::CanonicalZcashBindingMismatch);
    }
    Ok(())
}

pub(crate) const fn taker_first_lock_step(direction: SwapDirection) -> FirstLockStepV1 {
    match direction {
        SwapDirection::TakerSellsForeign => FirstLockStepV1::ZcashFund,
        SwapDirection::TakerSellsLez => FirstLockStepV1::LezFund,
    }
}

const fn step_name(step: FirstLockStepV1) -> &'static str {
    match step {
        FirstLockStepV1::ZcashFund => "zcash_fund",
        FirstLockStepV1::LezInitialize => "lez_initialize",
        FirstLockStepV1::LezFund => "lez_fund",
    }
}

fn parse_step(value: &str) -> Result<FirstLockStepV1, ObservedTakerFirstLockTransitionError> {
    match value {
        "zcash_fund" => Ok(FirstLockStepV1::ZcashFund),
        "lez_initialize" => Ok(FirstLockStepV1::LezInitialize),
        "lez_fund" => Ok(FirstLockStepV1::LezFund),
        _ => Err(ObservedTakerFirstLockTransitionError::ContextMismatch),
    }
}
