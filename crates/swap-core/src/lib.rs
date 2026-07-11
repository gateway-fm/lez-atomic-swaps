//! Chain-independent atomic-swap protocol state machine.
//!
//! The core accepts facts recoverable from chain nodes. Discovery and negotiation belong outside
//! this crate so an in-flight swap does not depend on off-chain coordination.

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// A supported foreign-chain leg.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Pair {
    /// Bitcoin Taproot/adaptor-signature leg.
    Bitcoin,
    /// Monero adaptor-signature/cross-curve-DLEQ leg.
    Monero,
    /// Zcash transparent-pool HTLC leg.
    Zcash,
}

/// Durable phase of one swap.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Phase {
    /// Terms are fixed but neither leg has a known lock transaction.
    Offered,
    /// The taker's foreign-chain lock exists but lacks required confirmations.
    AwaitingTakerConfirmations,
    /// The taker's lock satisfies confirmation policy; the maker may now lock LEZ funds.
    TakerLockConfirmed,
    /// Both legs are locked.
    BothLegsLocked,
    /// The maker claimed the foreign leg, revealing claim evidence for the taker.
    ClaimEvidenceAvailable,
    /// Both parties claimed their proceeds.
    Completed,
    /// The maker recovered the LEZ leg; the longer foreign timelock is still pending.
    MakerLegRefunded,
    /// The taker recovered the foreign leg; the maker's LEZ refund has not been observed.
    TakerLegRefunded,
    /// Both parties recovered their original funds.
    Refunded,
}

/// Protocol validation or transition error.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum Error {
    /// A stable identifier was empty or too long.
    #[error("identifier must contain 1 to 128 bytes")]
    InvalidIdentifier,
    /// A confirmation policy must require at least one confirmation.
    #[error("confirmation policy must be non-zero")]
    InvalidConfirmationPolicy,
    /// Foreign-chain refund must be strictly later than the LEZ refund.
    #[error("foreign refund deadline must be later than LEZ deadline")]
    ForeignTimelockMustFollowLez,
    /// The maker attempted to lock before the taker's lock reached confirmation policy.
    #[error("taker lock is not confirmed")]
    TakerLockNotConfirmed,
    /// A different transaction was presented while confirmations were being accumulated.
    #[error("conflicting taker lock transaction")]
    ConflictingTakerLock,
    /// The requested transition is not valid in the current phase.
    #[error("expected phase {expected:?}, found {actual:?}")]
    InvalidPhase { expected: Phase, actual: Phase },
    /// A refund was attempted before its deadline.
    #[error("refund timelock has not expired")]
    TimelockNotExpired,
}

/// Stable application-level swap identifier.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SwapId(Box<str>);

impl SwapId {
    /// Validates and creates an identifier.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidIdentifier`] when the identifier is empty or exceeds 128 bytes.
    pub fn new(value: impl Into<Box<str>>) -> Result<Self, Error> {
        let value = value.into();
        if value.is_empty() || value.len() > 128 {
            return Err(Error::InvalidIdentifier);
        }
        Ok(Self(value))
    }

    /// Returns the identifier text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Chain transaction evidence plus its observed confirmation count.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChainProof {
    transaction_id: Box<str>,
    confirmations: u32,
}

impl ChainProof {
    /// Creates observed transaction evidence.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidIdentifier`] when the transaction ID is empty or exceeds 128 bytes.
    pub fn new(transaction_id: impl Into<Box<str>>, confirmations: u32) -> Result<Self, Error> {
        let transaction_id = transaction_id.into();
        if transaction_id.is_empty() || transaction_id.len() > 128 {
            return Err(Error::InvalidIdentifier);
        }
        Ok(Self {
            transaction_id,
            confirmations,
        })
    }

    /// Returns the chain-specific transaction identifier.
    #[must_use]
    pub fn transaction_id(&self) -> &str {
        &self.transaction_id
    }

    /// Returns confirmations observed by the chain adapter.
    #[must_use]
    pub const fn confirmations(&self) -> u32 {
        self.confirmations
    }
}

/// Required confirmations before the maker can lock its leg.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfirmationPolicy(u32);

impl ConfirmationPolicy {
    /// Creates a non-zero confirmation policy.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidConfirmationPolicy`] when `required` is zero.
    pub const fn new(required: u32) -> Result<Self, Error> {
        if required == 0 {
            return Err(Error::InvalidConfirmationPolicy);
        }
        Ok(Self(required))
    }

    /// Returns the required confirmation count.
    #[must_use]
    pub const fn required(self) -> u32 {
        self.0
    }
}

/// Absolute refund deadlines in the coordinator's normalized time domain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Timelocks {
    lez_refund_at: u64,
    foreign_refund_at: u64,
}

impl Timelocks {
    /// Creates safe refund ordering: LEZ first, foreign chain strictly later.
    ///
    /// # Errors
    ///
    /// Returns [`Error::ForeignTimelockMustFollowLez`] unless the foreign deadline is later.
    pub const fn new(lez_refund_at: u64, foreign_refund_at: u64) -> Result<Self, Error> {
        if foreign_refund_at <= lez_refund_at {
            return Err(Error::ForeignTimelockMustFollowLez);
        }
        Ok(Self {
            lez_refund_at,
            foreign_refund_at,
        })
    }

    /// LEZ-side deadline.
    #[must_use]
    pub const fn lez_refund_at(self) -> u64 {
        self.lez_refund_at
    }

    /// Foreign-chain deadline.
    #[must_use]
    pub const fn foreign_refund_at(self) -> u64 {
        self.foreign_refund_at
    }
}

/// Witness made public by the maker's claim.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaimEvidence([u8; 32]);

impl std::fmt::Debug for ClaimEvidence {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ClaimEvidence([REDACTED])")
    }
}

impl ClaimEvidence {
    /// Creates pair-specific claim evidence represented as a 32-byte secret.
    #[must_use]
    pub const fn new(secret: [u8; 32]) -> Self {
        Self(secret)
    }

    /// Returns the adaptor secret or HTLC preimage.
    #[must_use]
    pub const fn secret(&self) -> &[u8; 32] {
        &self.0
    }
}

/// State machine for one independently persisted swap.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SwapCoordinator {
    id: SwapId,
    pair: Pair,
    confirmation_policy: ConfirmationPolicy,
    timelocks: Timelocks,
    phase: Phase,
    taker_lock_transaction_id: Option<Box<str>>,
    claim_evidence: Option<ClaimEvidence>,
}

impl SwapCoordinator {
    /// Creates a coordinator with negotiated, immutable safety parameters.
    #[must_use]
    pub const fn new(
        id: SwapId,
        pair: Pair,
        confirmation_policy: ConfirmationPolicy,
        timelocks: Timelocks,
    ) -> Self {
        Self {
            id,
            pair,
            confirmation_policy,
            timelocks,
            phase: Phase::Offered,
            taker_lock_transaction_id: None,
            claim_evidence: None,
        }
    }

    /// Current durable phase.
    #[must_use]
    pub const fn phase(&self) -> Phase {
        self.phase
    }

    /// Stable identifier.
    #[must_use]
    pub const fn id(&self) -> &SwapId {
        &self.id
    }

    /// Foreign pair.
    #[must_use]
    pub const fn pair(&self) -> Pair {
        self.pair
    }

    /// Returns the recovered adaptor secret or HTLC preimage, when observed.
    #[must_use]
    pub const fn claim_evidence(&self) -> Option<&ClaimEvidence> {
        self.claim_evidence.as_ref()
    }

    /// Tracks the taker's lock and promotes it once confirmation policy is met.
    ///
    /// # Errors
    ///
    /// Returns [`Error::ConflictingTakerLock`] for a changed transaction ID, or
    /// [`Error::InvalidPhase`] after the taker lock has already been confirmed.
    pub fn observe_taker_foreign_lock(&mut self, proof: ChainProof) -> Result<(), Error> {
        if !matches!(
            self.phase,
            Phase::Offered | Phase::AwaitingTakerConfirmations
        ) {
            return Err(Error::InvalidPhase {
                expected: Phase::AwaitingTakerConfirmations,
                actual: self.phase,
            });
        }
        if self
            .taker_lock_transaction_id
            .as_deref()
            .is_some_and(|known| known != proof.transaction_id())
        {
            return Err(Error::ConflictingTakerLock);
        }
        self.taker_lock_transaction_id = Some(proof.transaction_id);
        self.phase = if proof.confirmations >= self.confirmation_policy.required() {
            Phase::TakerLockConfirmed
        } else {
            Phase::AwaitingTakerConfirmations
        };
        Ok(())
    }

    /// Records the maker's LEZ lock, enforcing taker-first ordering.
    ///
    /// # Errors
    ///
    /// Returns [`Error::TakerLockNotConfirmed`] until confirmation policy is satisfied.
    pub fn observe_maker_lez_lock(&mut self, _proof: ChainProof) -> Result<(), Error> {
        if self.phase != Phase::TakerLockConfirmed {
            return Err(Error::TakerLockNotConfirmed);
        }
        self.phase = Phase::BothLegsLocked;
        Ok(())
    }

    /// Records a foreign-leg claim and its extracted witness.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidPhase`] unless both legs are locked.
    pub fn observe_maker_claim(&mut self, evidence: ClaimEvidence) -> Result<(), Error> {
        self.require_phase(Phase::BothLegsLocked)?;
        self.claim_evidence = Some(evidence);
        self.phase = Phase::ClaimEvidenceAvailable;
        Ok(())
    }

    /// Records the taker's LEZ claim and completes the swap.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidPhase`] until maker claim evidence is available.
    pub fn observe_taker_lez_claim(&mut self, _proof: ChainProof) -> Result<(), Error> {
        self.require_phase(Phase::ClaimEvidenceAvailable)?;
        self.phase = Phase::Completed;
        Ok(())
    }

    /// Refunds the shorter LEZ leg at or after its deadline.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidPhase`] unless both legs are locked, or
    /// [`Error::TimelockNotExpired`] before the LEZ deadline.
    pub fn refund_maker_lez_leg(&mut self, now: u64) -> Result<(), Error> {
        if !matches!(self.phase, Phase::BothLegsLocked | Phase::TakerLegRefunded) {
            return Err(Error::InvalidPhase {
                expected: Phase::BothLegsLocked,
                actual: self.phase,
            });
        }
        if now < self.timelocks.lez_refund_at() {
            return Err(Error::TimelockNotExpired);
        }
        self.phase = if self.phase == Phase::TakerLegRefunded {
            Phase::Refunded
        } else {
            Phase::MakerLegRefunded
        };
        Ok(())
    }

    /// Refunds the taker's foreign leg after the longer deadline.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidPhase`] unless the taker has locked its leg, or
    /// [`Error::TimelockNotExpired`] before the foreign deadline.
    pub fn refund_taker_foreign_leg(&mut self, now: u64) -> Result<(), Error> {
        if !matches!(
            self.phase,
            Phase::AwaitingTakerConfirmations
                | Phase::TakerLockConfirmed
                | Phase::BothLegsLocked
                | Phase::MakerLegRefunded
        ) {
            return Err(Error::InvalidPhase {
                expected: Phase::TakerLockConfirmed,
                actual: self.phase,
            });
        }
        if now < self.timelocks.foreign_refund_at() {
            return Err(Error::TimelockNotExpired);
        }
        self.phase = if matches!(
            self.phase,
            Phase::AwaitingTakerConfirmations | Phase::TakerLockConfirmed | Phase::MakerLegRefunded
        ) {
            Phase::Refunded
        } else {
            Phase::TakerLegRefunded
        };
        Ok(())
    }

    fn require_phase(&self, expected: Phase) -> Result<(), Error> {
        if self.phase != expected {
            return Err(Error::InvalidPhase {
                expected,
                actual: self.phase,
            });
        }
        Ok(())
    }
}
