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

/// Which asset the taker contributes in the first on-chain action.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SwapDirection {
    /// Taker locks the foreign asset; maker subsequently locks LEZ.
    #[default]
    TakerSellsForeign,
    /// Taker locks LEZ; maker subsequently locks the foreign asset.
    TakerSellsLez,
}

/// A protocol participant, independent of which chain holds their funded leg.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Participant {
    /// The offer maker, who funds second.
    Maker,
    /// The offer taker, who funds first.
    Taker,
}

impl Participant {
    /// Returns the counterparty.
    #[must_use]
    pub const fn other(self) -> Self {
        match self {
            Self::Maker => Self::Taker,
            Self::Taker => Self::Maker,
        }
    }
}

/// Chain whose consensus clock governs an observation or deadline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Chain {
    /// Logos Execution Zone.
    Lez,
    /// Bitcoin.
    Bitcoin,
    /// Monero.
    Monero,
    /// Zcash.
    Zcash,
}

impl From<Pair> for Chain {
    fn from(pair: Pair) -> Self {
        match pair {
            Pair::Bitcoin => Self::Bitcoin,
            Pair::Monero => Self::Monero,
            Pair::Zcash => Self::Zcash,
        }
    }
}

/// Consensus clock domain. Values from different domains are never numerically compared.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClockBasis {
    /// Canonical block height.
    BlockHeight,
    /// Consensus-visible Unix timestamp in seconds.
    Timestamp,
}

/// Unix time in whole seconds used by negotiated terms and safety projections.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnixSeconds(u64);

impl UnixSeconds {
    /// Creates a typed Unix-second value.
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// Returns the whole Unix seconds.
    #[must_use]
    pub const fn value(self) -> u64 {
        self.0
    }

    /// Converts negotiated whole-second terms to the exact LEZ millisecond clock value.
    ///
    /// # Errors
    ///
    /// Returns [`Error::TimestampConversionOverflow`] instead of wrapping a deadline.
    pub const fn checked_to_lez_milliseconds(self) -> Result<LezUnixMilliseconds, Error> {
        match self.0.checked_mul(1_000) {
            Some(value) => Ok(LezUnixMilliseconds(value)),
            None => Err(Error::TimestampConversionOverflow),
        }
    }
}

/// Consensus-visible LEZ Unix timestamp in milliseconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LezUnixMilliseconds(u64);

impl LezUnixMilliseconds {
    /// Creates a typed LEZ millisecond clock value.
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// Returns the exact LEZ Unix milliseconds.
    #[must_use]
    pub const fn value(self) -> u64 {
        self.0
    }

    /// Floors an observed LEZ timestamp for conservative deadline comparison.
    #[must_use]
    pub const fn to_unix_seconds_floor(self) -> UnixSeconds {
        UnixSeconds(self.0 / 1_000)
    }

    /// Ceils a LEZ timestamp used as an earlier-refund-latest safety bound.
    #[must_use]
    pub const fn to_unix_seconds_ceil(self) -> UnixSeconds {
        let seconds = self.0 / 1_000;
        let partial = if self.0.is_multiple_of(1_000) { 0 } else { 1 };
        UnixSeconds(seconds + partial)
    }
}

/// Typed position in one chain's consensus clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChainPosition {
    chain: Chain,
    basis: ClockBasis,
    value: u64,
}

impl ChainPosition {
    /// Creates a block-height position.
    #[must_use]
    pub const fn block_height(chain: Chain, height: u64) -> Self {
        Self {
            chain,
            basis: ClockBasis::BlockHeight,
            value: height,
        }
    }

    /// Creates a consensus timestamp position.
    #[must_use]
    pub const fn timestamp(chain: Chain, unix_seconds: u64) -> Self {
        Self {
            chain,
            basis: ClockBasis::Timestamp,
            value: unix_seconds,
        }
    }

    /// Converts an actual LEZ millisecond observation to conservative whole seconds.
    #[must_use]
    pub const fn lez_timestamp_from_milliseconds_floor(timestamp: LezUnixMilliseconds) -> Self {
        Self::timestamp(Chain::Lez, timestamp.to_unix_seconds_floor().value())
    }

    /// Position chain.
    #[must_use]
    pub const fn chain(self) -> Chain {
        self.chain
    }

    /// Position clock basis.
    #[must_use]
    pub const fn basis(self) -> ClockBasis {
        self.basis
    }

    /// Height or timestamp value within its typed domain.
    #[must_use]
    pub const fn value(self) -> u64 {
        self.value
    }
}

/// Conservative wall-clock bounds used only to validate cross-chain safety margin.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimelockSafety {
    earlier_chain: Chain,
    later_chain: Chain,
    earlier_refund_latest: u64,
    later_refund_earliest: u64,
    required_margin: u64,
}

impl TimelockSafety {
    /// Creates bounds proving that one chain's later refund cannot precede the earlier refund.
    ///
    /// `earlier_refund_latest` includes worst-case inclusion/reorg delay on the first-refund
    /// chain; `later_refund_earliest` uses the fastest plausible later-chain clock.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InsufficientTimelockMargin`] when the conservative interval is too small
    /// or arithmetic would overflow.
    pub fn between(
        earlier_chain: Chain,
        later_chain: Chain,
        earlier_refund_latest_unix_seconds: u64,
        later_refund_earliest_unix_seconds: u64,
        required_margin_seconds: u64,
    ) -> Result<Self, Error> {
        let Some(required_later_time) =
            earlier_refund_latest_unix_seconds.checked_add(required_margin_seconds)
        else {
            return Err(Error::InsufficientTimelockMargin);
        };
        if earlier_chain == later_chain
            || required_margin_seconds == 0
            || later_refund_earliest_unix_seconds < required_later_time
        {
            return Err(Error::InsufficientTimelockMargin);
        }
        Ok(Self {
            earlier_chain,
            later_chain,
            earlier_refund_latest: earlier_refund_latest_unix_seconds,
            later_refund_earliest: later_refund_earliest_unix_seconds,
            required_margin: required_margin_seconds,
        })
    }
}

/// Condition that makes recovery of the maker-funded leg available.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MakerRecoveryTrigger {
    /// A consensus deadline on the maker-funded leg.
    Deadline(ChainPosition),
    /// The XMR maker recovery becomes available after the taker's LEZ refund is canonical.
    CanonicalTakerRefund {
        /// Chain on which the taker refund must be observed.
        chain: Chain,
        /// Confirmations required before using the recovered Monero key share.
        required_confirmations: u32,
    },
}

/// Typed pair/direction-aware recovery schedule.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoverySchedule {
    maker_trigger: MakerRecoveryTrigger,
    taker_refund: ChainPosition,
    safety: Option<TimelockSafety>,
}

impl RecoverySchedule {
    /// Creates a deadline-based schedule for BTC or ZEC and verifies role chains.
    ///
    /// # Errors
    ///
    /// Returns [`Error::WrongRefundChain`] when direction and deadline chains disagree.
    pub fn new(
        pair: Pair,
        direction: SwapDirection,
        maker_refund: ChainPosition,
        taker_refund: ChainPosition,
        safety: TimelockSafety,
    ) -> Result<Self, Error> {
        if pair == Pair::Monero {
            return if direction == SwapDirection::TakerSellsForeign {
                Err(Error::UnsupportedDirection { pair, direction })
            } else {
                Err(Error::RecoveryRequiresTakerRefundEvent)
            };
        }
        let foreign = Chain::from(pair);
        let expected_chains = match direction {
            SwapDirection::TakerSellsForeign => [Chain::Lez, foreign],
            SwapDirection::TakerSellsLez => [foreign, Chain::Lez],
        };
        if maker_refund.chain != expected_chains[0] {
            return Err(Error::WrongRefundChain {
                role: "maker",
                expected: expected_chains[0],
                actual: maker_refund.chain,
            });
        }
        if taker_refund.chain != expected_chains[1] {
            return Err(Error::WrongRefundChain {
                role: "taker",
                expected: expected_chains[1],
                actual: taker_refund.chain,
            });
        }
        let expected_safety_order = match pair {
            Pair::Bitcoin => [maker_refund.chain, taker_refund.chain],
            Pair::Zcash => [Chain::Lez, Chain::Zcash],
            Pair::Monero => unreachable!("Monero returned before deadline validation"),
        };
        if [safety.earlier_chain, safety.later_chain] != expected_safety_order {
            return Err(Error::WrongTimelockOrder {
                pair,
                earlier: expected_safety_order[0],
                later: expected_safety_order[1],
            });
        }
        Ok(Self {
            maker_trigger: MakerRecoveryTrigger::Deadline(maker_refund),
            taker_refund,
            safety: Some(safety),
        })
    }

    /// Creates the reviewed LEZ-first XMR recovery schedule.
    ///
    /// # Errors
    ///
    /// Returns a wrong-chain or invalid-confirmation error when the taker's refund is not a
    /// non-zero-confirmation LEZ event.
    pub fn xmr_lez_first(
        taker_refund: ChainPosition,
        required_event_confirmations: u32,
    ) -> Result<Self, Error> {
        if taker_refund.chain != Chain::Lez {
            return Err(Error::WrongRefundChain {
                role: "taker",
                expected: Chain::Lez,
                actual: taker_refund.chain,
            });
        }
        if required_event_confirmations == 0 {
            return Err(Error::InvalidConfirmationPolicy);
        }
        Ok(Self {
            maker_trigger: MakerRecoveryTrigger::CanonicalTakerRefund {
                chain: Chain::Lez,
                required_confirmations: required_event_confirmations,
            },
            taker_refund,
            safety: None,
        })
    }

    /// Returns the maker-funded leg's recovery trigger.
    #[must_use]
    pub const fn maker_trigger(self) -> MakerRecoveryTrigger {
        self.maker_trigger
    }

    /// Tests a deadline-based maker recovery against the same chain clock.
    ///
    /// # Errors
    ///
    /// Returns [`Error::WrongDeadlineClock`] instead of comparing unrelated raw numbers.
    pub fn maker_deadline_reached(self, observed: ChainPosition) -> Result<bool, Error> {
        match self.maker_trigger {
            MakerRecoveryTrigger::Deadline(deadline) => deadline_reached(deadline, observed),
            MakerRecoveryTrigger::CanonicalTakerRefund { .. } => {
                Err(Error::RecoveryRequiresTakerRefundEvent)
            }
        }
    }

    /// Tests the taker deadline against a position in exactly the same chain clock.
    ///
    /// # Errors
    ///
    /// Returns [`Error::WrongDeadlineClock`] instead of comparing unrelated raw numbers.
    pub fn taker_refund_reached(self, observed: ChainPosition) -> Result<bool, Error> {
        deadline_reached(self.taker_refund, observed)
    }
}

fn deadline_reached(deadline: ChainPosition, observed: ChainPosition) -> Result<bool, Error> {
    if deadline.chain != observed.chain || deadline.basis != observed.basis {
        return Err(Error::WrongDeadlineClock {
            expected_chain: deadline.chain,
            expected_basis: deadline.basis,
            actual_chain: observed.chain,
            actual_basis: observed.basis,
        });
    }
    Ok(observed.value >= deadline.value)
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
    /// The maker's lock exists but lacks its independent required confirmations.
    AwaitingMakerConfirmations,
    /// Both legs are locked.
    BothLegsLocked,
    /// The taker's funding transaction regressed below confirmation policy after maker lock.
    TakerLockReorged,
    /// The maker's committed funding transaction left the canonical chain.
    MakerLockReorged,
    /// The construction-specific first claimant published evidence for the counterparty.
    ClaimEvidenceAvailable,
    /// Both parties claimed their proceeds.
    Completed,
    /// The maker recovered its funded leg; the taker's funded leg remains unresolved.
    MakerLegRefunded,
    /// The taker recovered its funded leg; the maker's funded leg remains unresolved.
    TakerLegRefunded,
    /// Both parties recovered their original funds.
    Refunded,
    /// A canonical taker refund exposed the material needed for maker recovery.
    MakerRecoveryAvailable,
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
    /// A checked conversion between negotiated seconds and the LEZ millisecond clock overflowed.
    #[error("timestamp conversion overflowed")]
    TimestampConversionOverflow,
    /// Conservative cross-chain bounds do not leave the required recovery margin.
    #[error("refund deadlines do not provide the required cross-chain safety margin")]
    InsufficientTimelockMargin,
    /// A role-relative refund deadline was assigned to the wrong chain.
    #[error("{role} refund uses {actual:?}; expected {expected:?}")]
    WrongRefundChain {
        role: &'static str,
        expected: Chain,
        actual: Chain,
    },
    /// An observation used a different chain or clock basis than its deadline.
    #[error(
        "deadline expects {expected_chain:?}/{expected_basis:?}; observed {actual_chain:?}/{actual_basis:?}"
    )]
    WrongDeadlineClock {
        expected_chain: Chain,
        expected_basis: ClockBasis,
        actual_chain: Chain,
        actual_basis: ClockBasis,
    },
    /// A deadline profile assigned the cross-chain safety margin in the wrong order.
    #[error(
        "{pair:?} requires the {earlier:?} refund before the {later:?} refund by the safety margin"
    )]
    WrongTimelockOrder {
        /// Pair whose construction fixes the ordering.
        pair: Pair,
        /// Chain whose refund must be conservatively earlier.
        earlier: Chain,
        /// Chain whose refund must be conservatively later.
        later: Chain,
    },
    /// The selected pair has no reviewed construction for this funding direction.
    #[error("{pair:?} does not support direction {direction:?}")]
    UnsupportedDirection {
        pair: Pair,
        direction: SwapDirection,
    },
    /// A claim was attributed to the wrong participant for this pair and direction.
    #[error("claim expected {expected:?}, observed {actual:?}")]
    UnexpectedClaimant {
        /// Participant required by the reviewed construction.
        expected: Participant,
        /// Participant supplied by the observation.
        actual: Participant,
    },
    /// The maker attempted to lock before the taker's lock reached confirmation policy.
    #[error("taker lock is not confirmed")]
    TakerLockNotConfirmed,
    /// The maker's committed funding transaction is not currently canonical.
    #[error("maker lock is not confirmed")]
    MakerLockNotConfirmed,
    /// A different transaction was presented while confirmations were being accumulated.
    #[error("conflicting taker lock transaction")]
    ConflictingTakerLock,
    /// A different maker-funded lock was presented for the same swap.
    #[error("conflicting maker lock transaction")]
    ConflictingMakerLock,
    /// Different claim evidence was presented for the same swap.
    #[error("conflicting claim evidence")]
    ConflictingClaimEvidence,
    /// A different follow-up claim was presented for the same swap.
    #[error("conflicting follow-up claim transaction")]
    ConflictingTakerClaim,
    /// The requested transition is not valid in the current phase.
    #[error("expected phase {expected:?}, found {actual:?}")]
    InvalidPhase { expected: Phase, actual: Phase },
    /// A refund was attempted before its deadline.
    #[error("refund timelock has not expired")]
    TimelockNotExpired,
    /// The maker-funded leg is recovered from a canonical taker-refund event, not a deadline.
    #[error("maker recovery requires a canonical taker refund event")]
    RecoveryRequiresTakerRefundEvent,
    /// A recovery event was observed on the wrong chain.
    #[error("recovery event uses {actual:?}; expected {expected:?}")]
    WrongRecoveryEventChain { expected: Chain, actual: Chain },
    /// A recovery event has not reached its configured canonicality policy.
    #[error("recovery event has {actual} confirmations; requires {required}")]
    InsufficientRecoveryEventConfirmations { required: u32, actual: u32 },
    /// A different taker refund event was presented for the same swap.
    #[error("conflicting taker refund event")]
    ConflictingTakerRefundEvent,
    /// A different maker recovery transaction was presented for the same swap.
    #[error("conflicting maker recovery transaction")]
    ConflictingMakerRecovery,
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

/// Chain-typed event evidence used by event-gated recovery.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChainEventProof {
    chain: Chain,
    proof: ChainProof,
}

impl ChainEventProof {
    /// Creates chain-typed transaction evidence.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidIdentifier`] when the transaction ID is invalid.
    pub fn new(
        chain: Chain,
        transaction_id: impl Into<Box<str>>,
        confirmations: u32,
    ) -> Result<Self, Error> {
        Ok(Self {
            chain,
            proof: ChainProof::new(transaction_id, confirmations)?,
        })
    }

    /// Chain on which the event occurred.
    #[must_use]
    pub const fn chain(&self) -> Chain {
        self.chain
    }

    /// Event transaction identifier.
    #[must_use]
    pub fn transaction_id(&self) -> &str {
        self.proof.transaction_id()
    }

    /// Canonical confirmation count.
    #[must_use]
    pub const fn confirmations(&self) -> u32 {
        self.proof.confirmations()
    }
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

const fn default_maker_confirmation_policy() -> ConfirmationPolicy {
    ConfirmationPolicy(1)
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
    #[serde(default)]
    direction: SwapDirection,
    confirmation_policy: ConfirmationPolicy,
    #[serde(default = "default_maker_confirmation_policy")]
    maker_confirmation_policy: ConfirmationPolicy,
    #[serde(alias = "refund_schedule")]
    recovery_schedule: RecoverySchedule,
    phase: Phase,
    #[serde(default)]
    taker_lock_transaction_id: Option<Box<str>>,
    #[serde(default)]
    #[serde(alias = "maker_lez_lock_transaction_id")]
    maker_lock_transaction_id: Option<Box<str>>,
    #[serde(default)]
    claim_evidence: Option<ClaimEvidence>,
    #[serde(default)]
    revealing_claim_transaction_id: Option<Box<str>>,
    #[serde(default, alias = "taker_lez_claim_transaction_id")]
    followup_claim_transaction_id: Option<Box<str>>,
    #[serde(default)]
    taker_refund_event_transaction_id: Option<Box<str>>,
    #[serde(default)]
    maker_recovery_transaction_id: Option<Box<str>>,
}

impl SwapCoordinator {
    /// Creates a coordinator with negotiated, immutable safety parameters.
    #[must_use]
    pub const fn new(
        id: SwapId,
        pair: Pair,
        confirmation_policy: ConfirmationPolicy,
        recovery_schedule: RecoverySchedule,
    ) -> Self {
        Self {
            id,
            pair,
            direction: SwapDirection::TakerSellsForeign,
            confirmation_policy,
            maker_confirmation_policy: default_maker_confirmation_policy(),
            recovery_schedule,
            phase: Phase::Offered,
            taker_lock_transaction_id: None,
            maker_lock_transaction_id: None,
            claim_evidence: None,
            revealing_claim_transaction_id: None,
            followup_claim_transaction_id: None,
            taker_refund_event_transaction_id: None,
            maker_recovery_transaction_id: None,
        }
    }

    /// Creates a coordinator for an explicit negotiated trade direction.
    #[must_use]
    pub const fn new_with_direction(
        id: SwapId,
        pair: Pair,
        direction: SwapDirection,
        confirmation_policy: ConfirmationPolicy,
        recovery_schedule: RecoverySchedule,
    ) -> Self {
        Self {
            id,
            pair,
            direction,
            confirmation_policy,
            maker_confirmation_policy: default_maker_confirmation_policy(),
            recovery_schedule,
            phase: Phase::Offered,
            taker_lock_transaction_id: None,
            maker_lock_transaction_id: None,
            claim_evidence: None,
            revealing_claim_transaction_id: None,
            followup_claim_transaction_id: None,
            taker_refund_event_transaction_id: None,
            maker_recovery_transaction_id: None,
        }
    }

    /// Creates a coordinator with independent taker- and maker-leg confirmation policies.
    #[must_use]
    pub const fn new_with_confirmation_policies(
        id: SwapId,
        pair: Pair,
        direction: SwapDirection,
        taker_confirmation_policy: ConfirmationPolicy,
        maker_confirmation_policy: ConfirmationPolicy,
        recovery_schedule: RecoverySchedule,
    ) -> Self {
        Self {
            id,
            pair,
            direction,
            confirmation_policy: taker_confirmation_policy,
            maker_confirmation_policy,
            recovery_schedule,
            phase: Phase::Offered,
            taker_lock_transaction_id: None,
            maker_lock_transaction_id: None,
            claim_evidence: None,
            revealing_claim_transaction_id: None,
            followup_claim_transaction_id: None,
            taker_refund_event_transaction_id: None,
            maker_recovery_transaction_id: None,
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

    /// Negotiated direction. It determines which chain each role-specific leg uses.
    #[must_use]
    pub const fn direction(&self) -> SwapDirection {
        self.direction
    }

    /// Chain funded by one participant under the negotiated product direction.
    #[must_use]
    pub const fn funded_chain(&self, participant: Participant) -> Chain {
        let foreign = match self.pair {
            Pair::Bitcoin => Chain::Bitcoin,
            Pair::Monero => Chain::Monero,
            Pair::Zcash => Chain::Zcash,
        };
        match (self.direction, participant) {
            (SwapDirection::TakerSellsForeign, Participant::Taker)
            | (SwapDirection::TakerSellsLez, Participant::Maker) => foreign,
            (SwapDirection::TakerSellsForeign, Participant::Maker)
            | (SwapDirection::TakerSellsLez, Participant::Taker) => Chain::Lez,
        }
    }

    /// Transaction ID pinned to one participant's funded leg, when observed.
    #[must_use]
    pub fn funding_transaction_id(&self, participant: Participant) -> Option<&str> {
        match participant {
            Participant::Maker => self.maker_lock_transaction_id.as_deref(),
            Participant::Taker => self.taker_lock_transaction_id.as_deref(),
        }
    }

    /// Observes participant-relative funding without assuming which chain they fund.
    ///
    /// # Errors
    ///
    /// Delegates to the same ordering, confirmation, conflict, and phase checks as
    /// [`Self::observe_taker_lock`] or [`Self::observe_maker_lock`].
    pub fn observe_funding(
        &mut self,
        participant: Participant,
        proof: ChainProof,
    ) -> Result<(), Error> {
        match participant {
            Participant::Maker => self.observe_maker_lock(proof),
            Participant::Taker => self.observe_taker_lock(proof),
        }
    }

    /// Applies an affirmative canonical removal to the participant who funded that leg.
    ///
    /// # Errors
    ///
    /// Returns the role-specific conflict error when the transaction does not match
    /// the committed funding ID.
    pub fn observe_funding_removed(
        &mut self,
        participant: Participant,
        transaction_id: &str,
    ) -> Result<(), Error> {
        match participant {
            Participant::Maker => self.observe_maker_lock_removed(transaction_id),
            Participant::Taker => self.observe_taker_lock_removed(transaction_id),
        }
    }

    /// Participant whose claim must publish the adaptor witness or HTLC preimage first.
    ///
    /// ZEC follows the RFP's chain-relative order: the LEZ recipient claims first and the ZEC
    /// recipient follows. BTC's first-funding taker claims the maker-funded leg first. XMR keeps
    /// the reviewed LEZ-first COMIT order in which the maker claims first.
    #[must_use]
    pub const fn first_claimant(&self) -> Participant {
        match (self.pair, self.direction) {
            (Pair::Bitcoin, _) | (Pair::Zcash, SwapDirection::TakerSellsForeign) => {
                Participant::Taker
            }
            (Pair::Monero, _) | (Pair::Zcash, SwapDirection::TakerSellsLez) => Participant::Maker,
        }
    }

    /// Tracks the taker's first lock on whichever chain the direction assigns to the taker.
    ///
    /// # Errors
    ///
    /// Returns [`Error::ConflictingTakerLock`] for changed evidence or an invalid-phase error.
    pub fn observe_taker_lock(&mut self, proof: ChainProof) -> Result<(), Error> {
        self.observe_taker_lock_impl(proof)
    }

    /// Records that the previously observed taker funding transaction left the canonical chain.
    ///
    /// Before maker funding, this clears the observation so an explicit replacement can be
    /// accepted. After maker funding, the committed transaction ID is retained and claims are
    /// suspended; a different transaction can never be substituted silently.
    ///
    /// # Errors
    ///
    /// Returns [`Error::ConflictingTakerLock`] when the removed transaction is not the committed
    /// taker transaction.
    pub fn observe_taker_lock_removed(&mut self, transaction_id: &str) -> Result<(), Error> {
        if self.taker_lock_transaction_id.as_deref() != Some(transaction_id) {
            return Err(Error::ConflictingTakerLock);
        }
        match self.phase {
            Phase::AwaitingTakerConfirmations | Phase::TakerLockConfirmed => {
                self.taker_lock_transaction_id = None;
                self.phase = Phase::Offered;
            }
            Phase::AwaitingMakerConfirmations
            | Phase::BothLegsLocked
            | Phase::ClaimEvidenceAvailable => {
                self.phase = Phase::TakerLockReorged;
            }
            _ => {}
        }
        Ok(())
    }

    /// Records the maker's second lock, enforcing confirmed taker-first ordering.
    ///
    /// # Errors
    ///
    /// Returns [`Error::TakerLockNotConfirmed`] until confirmation policy is satisfied.
    pub fn observe_maker_lock(&mut self, proof: ChainProof) -> Result<(), Error> {
        self.observe_maker_lock_impl(proof)
    }

    /// Records that the committed maker-funded transaction left the canonical chain.
    ///
    /// The transaction ID remains pinned. Claims are suspended until the exact
    /// transaction reappears, while independent refunds remain available.
    ///
    /// # Errors
    ///
    /// Returns [`Error::ConflictingMakerLock`] when the removed transaction does not
    /// match the committed maker funding ID.
    pub fn observe_maker_lock_removed(&mut self, transaction_id: &str) -> Result<(), Error> {
        if self.maker_lock_transaction_id.as_deref() != Some(transaction_id) {
            return Err(Error::ConflictingMakerLock);
        }
        match self.phase {
            Phase::AwaitingMakerConfirmations => {
                self.maker_lock_transaction_id = None;
                self.phase = Phase::TakerLockConfirmed;
            }
            Phase::BothLegsLocked | Phase::ClaimEvidenceAvailable => {
                self.phase = Phase::MakerLockReorged;
            }
            _ => {}
        }
        Ok(())
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
    fn observe_taker_lock_impl(&mut self, proof: ChainProof) -> Result<(), Error> {
        if self
            .taker_lock_transaction_id
            .as_deref()
            .is_some_and(|known| known != proof.transaction_id())
        {
            return Err(Error::ConflictingTakerLock);
        }
        if matches!(self.phase, Phase::Completed | Phase::Refunded) {
            return Ok(());
        }
        self.taker_lock_transaction_id = Some(proof.transaction_id);
        let confirmed = proof.confirmations >= self.confirmation_policy.required();
        self.phase = match (self.phase, confirmed) {
            (
                Phase::Offered | Phase::AwaitingTakerConfirmations | Phase::TakerLockConfirmed,
                true,
            ) => Phase::TakerLockConfirmed,
            (
                Phase::Offered | Phase::AwaitingTakerConfirmations | Phase::TakerLockConfirmed,
                false,
            ) => Phase::AwaitingTakerConfirmations,
            (Phase::BothLegsLocked | Phase::ClaimEvidenceAvailable, false) => {
                Phase::TakerLockReorged
            }
            (Phase::TakerLockReorged, true) if self.claim_evidence.is_some() => {
                Phase::ClaimEvidenceAvailable
            }
            (Phase::TakerLockReorged, true) => Phase::BothLegsLocked,
            (phase, _) => phase,
        };
        Ok(())
    }

    /// Records the maker's funded lock, enforcing taker-first ordering.
    ///
    /// # Errors
    ///
    /// Returns [`Error::TakerLockNotConfirmed`] until confirmation policy is satisfied.
    fn observe_maker_lock_impl(&mut self, proof: ChainProof) -> Result<(), Error> {
        let confirmed = proof.confirmations >= self.maker_confirmation_policy.required();
        if matches!(
            self.phase,
            Phase::AwaitingMakerConfirmations
                | Phase::BothLegsLocked
                | Phase::MakerLockReorged
                | Phase::ClaimEvidenceAvailable
        ) {
            if self.maker_lock_transaction_id.as_deref() != Some(proof.transaction_id()) {
                return Err(Error::ConflictingMakerLock);
            }
            self.phase = match (self.phase, confirmed) {
                (Phase::AwaitingMakerConfirmations, false) => Phase::AwaitingMakerConfirmations,
                (Phase::BothLegsLocked | Phase::ClaimEvidenceAvailable, false) => {
                    Phase::MakerLockReorged
                }
                (Phase::MakerLockReorged, true) if self.claim_evidence.is_some() => {
                    Phase::ClaimEvidenceAvailable
                }
                (Phase::AwaitingMakerConfirmations | Phase::MakerLockReorged, true) => {
                    Phase::BothLegsLocked
                }
                (phase, _) => phase,
            };
            return Ok(());
        }
        if self.phase != Phase::TakerLockConfirmed {
            return match self.maker_lock_transaction_id.as_deref() {
                Some(known) if known == proof.transaction_id() => Ok(()),
                Some(_) => Err(Error::ConflictingMakerLock),
                None => Err(Error::TakerLockNotConfirmed),
            };
        }
        self.maker_lock_transaction_id = Some(proof.transaction_id);
        self.phase = if confirmed {
            Phase::BothLegsLocked
        } else {
            Phase::AwaitingMakerConfirmations
        };
        Ok(())
    }

    /// Records the construction-specific first claim and its extracted public evidence.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidPhase`] unless both legs are locked.
    pub fn observe_revealing_claim(
        &mut self,
        claimant: Participant,
        proof: ChainProof,
        evidence: ClaimEvidence,
    ) -> Result<(), Error> {
        let expected_claimant = self.first_claimant();
        if claimant != expected_claimant {
            return Err(Error::UnexpectedClaimant {
                expected: expected_claimant,
                actual: claimant,
            });
        }
        if self.phase == Phase::TakerLockReorged {
            return Err(Error::TakerLockNotConfirmed);
        }
        if self.phase == Phase::MakerLockReorged {
            return Err(Error::MakerLockNotConfirmed);
        }
        if self.phase != Phase::BothLegsLocked {
            return match (
                self.revealing_claim_transaction_id.as_deref(),
                self.claim_evidence.as_ref(),
            ) {
                (Some(known_id), Some(known_evidence))
                    if known_id == proof.transaction_id() && known_evidence == &evidence =>
                {
                    Ok(())
                }
                (Some(_), Some(_)) => Err(Error::ConflictingClaimEvidence),
                _ => Err(Error::InvalidPhase {
                    expected: Phase::BothLegsLocked,
                    actual: self.phase,
                }),
            };
        }
        self.revealing_claim_transaction_id = Some(proof.transaction_id);
        self.claim_evidence = Some(evidence);
        self.phase = Phase::ClaimEvidenceAvailable;
        Ok(())
    }

    /// Records the counterparty's follow-up claim and completes the swap.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidPhase`] until revealing claim evidence is available.
    pub fn observe_followup_claim(
        &mut self,
        claimant: Participant,
        proof: ChainProof,
    ) -> Result<(), Error> {
        let expected_claimant = self.first_claimant().other();
        if claimant != expected_claimant {
            return Err(Error::UnexpectedClaimant {
                expected: expected_claimant,
                actual: claimant,
            });
        }
        if self.phase == Phase::TakerLockReorged {
            return Err(Error::TakerLockNotConfirmed);
        }
        if self.phase == Phase::MakerLockReorged {
            return Err(Error::MakerLockNotConfirmed);
        }
        if self.phase != Phase::ClaimEvidenceAvailable {
            return match self.followup_claim_transaction_id.as_deref() {
                Some(known) if known == proof.transaction_id() => Ok(()),
                Some(_) => Err(Error::ConflictingTakerClaim),
                None => Err(Error::InvalidPhase {
                    expected: Phase::ClaimEvidenceAvailable,
                    actual: self.phase,
                }),
            };
        }
        self.followup_claim_transaction_id = Some(proof.transaction_id);
        self.phase = Phase::Completed;
        Ok(())
    }

    /// Refunds the maker-funded leg at its typed chain position.
    ///
    /// # Errors
    ///
    /// Returns an invalid-phase, wrong-clock, or unexpired-timelock error when unavailable.
    pub fn refund_maker_leg(&mut self, observed: ChainPosition) -> Result<(), Error> {
        if !matches!(
            self.phase,
            Phase::BothLegsLocked
                | Phase::AwaitingMakerConfirmations
                | Phase::TakerLockReorged
                | Phase::MakerLockReorged
                | Phase::TakerLegRefunded
        ) {
            return Err(Error::InvalidPhase {
                expected: Phase::BothLegsLocked,
                actual: self.phase,
            });
        }
        if !self.recovery_schedule.maker_deadline_reached(observed)? {
            return Err(Error::TimelockNotExpired);
        }
        self.phase = if self.phase == Phase::TakerLegRefunded {
            Phase::Refunded
        } else {
            Phase::MakerLegRefunded
        };
        Ok(())
    }

    /// Records the canonical taker refund that unlocks event-gated maker recovery.
    ///
    /// # Errors
    ///
    /// Returns a wrong-chain, insufficient-confirmation, conflicting-evidence, or invalid-phase
    /// error when the evidence does not satisfy the immutable recovery trigger.
    pub fn observe_taker_refund_for_maker_recovery(
        &mut self,
        evidence: ChainEventProof,
    ) -> Result<(), Error> {
        let MakerRecoveryTrigger::CanonicalTakerRefund {
            chain,
            required_confirmations,
        } = self.recovery_schedule.maker_trigger()
        else {
            return Err(Error::InvalidPhase {
                expected: Phase::TakerLegRefunded,
                actual: self.phase,
            });
        };
        if evidence.chain() != chain {
            return Err(Error::WrongRecoveryEventChain {
                expected: chain,
                actual: evidence.chain(),
            });
        }
        if self
            .taker_refund_event_transaction_id
            .as_deref()
            .is_some_and(|known| known != evidence.transaction_id())
        {
            return Err(Error::ConflictingTakerRefundEvent);
        }
        if evidence.confirmations() < required_confirmations {
            if self.phase == Phase::MakerRecoveryAvailable {
                self.phase = Phase::TakerLegRefunded;
                return Ok(());
            }
            return Err(Error::InsufficientRecoveryEventConfirmations {
                required: required_confirmations,
                actual: evidence.confirmations(),
            });
        }
        if self.phase == Phase::MakerRecoveryAvailable || self.phase == Phase::Refunded {
            return Ok(());
        }
        if self.phase != Phase::TakerLegRefunded {
            return Err(Error::InvalidPhase {
                expected: Phase::TakerLegRefunded,
                actual: self.phase,
            });
        }
        self.taker_refund_event_transaction_id = Some(evidence.proof.transaction_id);
        self.phase = Phase::MakerRecoveryAvailable;
        Ok(())
    }

    /// Records the maker's completed recovery transaction after an event-gated refund.
    ///
    /// # Errors
    ///
    /// Returns a conflicting-evidence or invalid-phase error until maker recovery is available.
    pub fn observe_maker_recovery(&mut self, proof: ChainProof) -> Result<(), Error> {
        if self
            .maker_recovery_transaction_id
            .as_deref()
            .is_some_and(|known| known != proof.transaction_id())
        {
            return Err(Error::ConflictingMakerRecovery);
        }
        if self.phase == Phase::Refunded && self.maker_recovery_transaction_id.is_some() {
            return Ok(());
        }
        if self.phase != Phase::MakerRecoveryAvailable {
            return Err(Error::InvalidPhase {
                expected: Phase::MakerRecoveryAvailable,
                actual: self.phase,
            });
        }
        self.maker_recovery_transaction_id = Some(proof.transaction_id);
        self.phase = Phase::Refunded;
        Ok(())
    }

    /// Refunds the taker-funded leg at its typed deadline.
    ///
    /// # Errors
    ///
    /// Returns an invalid-phase, wrong-clock, or unexpired-timelock error when unavailable.
    pub fn refund_taker_leg(&mut self, observed: ChainPosition) -> Result<(), Error> {
        if !matches!(
            self.phase,
            Phase::AwaitingTakerConfirmations
                | Phase::TakerLockConfirmed
                | Phase::AwaitingMakerConfirmations
                | Phase::BothLegsLocked
                | Phase::TakerLockReorged
                | Phase::MakerLockReorged
                | Phase::MakerLegRefunded
        ) {
            return Err(Error::InvalidPhase {
                expected: Phase::TakerLockConfirmed,
                actual: self.phase,
            });
        }
        if !self.recovery_schedule.taker_refund_reached(observed)? {
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
}
