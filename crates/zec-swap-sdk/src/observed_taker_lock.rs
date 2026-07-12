//! Maker-local observation and durable projection of the taker's first lock.

use lez_swap_core::{ChainProof, Participant, SwapDirection, SwapId};
use serde::{Deserialize, Serialize};

use crate::{AcceptedZecAgreementV1, FirstLockProjectionCommit, FirstLockStepV1, ZecAgreementV1};

const OBSERVED_TAKER_FIRST_LOCK_SCHEMA_V1: u16 = 1;
const OBSERVED_TAKER_FIRST_LOCK_KIND_V1: &str = "maker_observed_taker_first_lock";

/// Stable result returned by an observation-only chain adapter.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TakerFirstLockObservationV1 {
    /// A fresh stable query proves the agreement-bound lock is absent.
    Absent,
    /// The query cannot prove stable presence or stable absence.
    Unstable,
    /// Fresh canonical evidence for the agreement-bound taker lock.
    Confirmed(ObservedTakerFirstLockEvidenceV1),
}

/// Versioned stable evidence asserted by a typed chain observation adapter.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ObservedTakerFirstLockEvidenceV1 {
    schema_version: u16,
    step: FirstLockStepV1,
    transaction_id: Box<str>,
    confirmations: u32,
}

impl ObservedTakerFirstLockEvidenceV1 {
    /// Constructs well-formed primitive evidence.
    ///
    /// Agreement direction and confirmation policy are checked when the maker
    /// constructs the transition.
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
        })
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
        let required = agreement
            .coordinator()
            .required_confirmations(Participant::Taker);
        if evidence.confirmations < required {
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

    pub(crate) fn apply_to(
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
        if self.evidence.confirmations < required {
            return Err(
                ObservedTakerFirstLockTransitionError::InsufficientConfirmations {
                    required,
                    actual: self.evidence.confirmations,
                },
            );
        }
        let mut next = coordinator.clone();
        next.observe_funding(
            Participant::Taker,
            ChainProof::new(
                self.evidence.transaction_id.clone(),
                self.evidence.confirmations,
            )
            .map_err(ObservedTakerFirstLockTransitionError::Core)?,
        )
        .map_err(ObservedTakerFirstLockTransitionError::Core)?;
        Ok(next)
    }
}

/// One safe maker polling outcome.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ObserveTakerFirstLockOutcome {
    /// No stable confirmed lock was projected.
    AwaitingStableObservation(FirstLockStepV1),
    /// Exact evidence was committed before in-memory projection.
    Projected(FirstLockProjectionCommit),
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
        let evidence = ObservedTakerFirstLockEvidenceV1::new(
            parse_step(&self.step)?,
            self.transaction_id.clone(),
            self.confirmations,
        )?;
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
