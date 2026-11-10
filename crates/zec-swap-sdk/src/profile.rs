//! Immutable network-bound Zcash recovery profiles.

use lez_swap_core::{
    Chain, ChainPosition, LezUnixMilliseconds, RecoverySchedule, SwapDirection, TimelockSafety,
    UnixSeconds,
};
use zcash_protocol::consensus::{BlockHeight, BranchId, NetworkType};

/// A reviewed immutable ZEC parameter profile.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ZecProfileId {
    /// Controlled standalone/Regtest profile used only by deterministic tests.
    DeterministicLocalV1,
    /// The deterministic Regtest profile with a LEZ refund delay a devnet can
    /// meet: the LEZ escrow program emits a timestamp validity window that
    /// ends at the agreement's LEZ refund time, and sixty seconds after the
    /// agreement's anchor is less than a Node recreate plus a take on a chain
    /// that finalises in minutes. Same confirmations, Zcash refund height and
    /// margin as [`Self::DeterministicLocalV1`].
    DeterministicLocalDevnetV1,
    /// [`Self::DeterministicLocalDevnetV1`] with an eight-block Zcash refund
    /// window. Profiles are immutable per id: two swaps of one signer taken
    /// together at one Zcash anchor need the first swap's four blocks (funding,
    /// two pre-reveal confirmations, claim) plus the second swap's own, and
    /// changing V1's four under its id would have invalidated every agreement
    /// and export made under it (2026-10-29).
    DeterministicLocalDevnetV2,
    /// [`Self::DeterministicLocalDevnetV2`] with a 12-minute LEZ refund delay.
    /// The pinned devnet chain freezes finality (upstream #3643) 17–56 min after
    /// it is created, ~27 min on the review host; a refund under V2's 30 min
    /// needs ~34 min of finality and never completed there (2026-10-30). Twelve
    /// minutes keep a refund inside ~17 min of finality and still leave the
    /// longest measured take→claim (≈7 min) a window. Devnet only; a
    /// production profile is unaffected.
    DeterministicLocalDevnetV3,
    /// LEZ 0.2 and Zcash public-testnet acceptance profile.
    PublicTestnetV1,
}

impl ZecProfileId {
    /// Returns the stable signed-terms identifier.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::DeterministicLocalV1 => "deterministic-local-v1",
            Self::DeterministicLocalDevnetV1 => "deterministic-local-devnet-v1",
            Self::DeterministicLocalDevnetV2 => "deterministic-local-devnet-v2",
            Self::DeterministicLocalDevnetV3 => "deterministic-local-devnet-v3",
            Self::PublicTestnetV1 => "public-testnet-v1",
        }
    }
}

/// Rejected profile or deadline construction.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum ProfileError {
    /// The selected node network does not match the immutable profile.
    #[error("Zcash network does not match the selected profile")]
    NetworkMismatch,
    /// The transaction consensus branch does not match the immutable profile.
    #[error("Zcash consensus branch does not match the selected profile")]
    ConsensusBranchMismatch,
    /// LEZ deadline construction overflowed Unix seconds or milliseconds.
    #[error("LEZ timestamp construction overflowed")]
    TimestampOverflow,
    /// Zcash CLTV height construction overflowed `u32`.
    #[error("Zcash refund height overflowed")]
    HeightOverflow,
    /// Fastest/slowest wall-clock bounds were not supplied by calibration or the harness.
    #[error("cross-chain safety calibration is required")]
    MissingSafetyCalibration,
    /// Supplied conservative bounds do not leave the profile's required margin.
    #[error("cross-chain bounds do not leave the profile's required margin")]
    InsufficientSafetyMargin,
    /// Role deadlines could not form a valid core recovery schedule.
    #[error("invalid recovery schedule: {0}")]
    InvalidSchedule(lez_swap_core::Error),
}

/// Immutable ZEC confirmation, expiry, and recovery parameters.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ZecRefundProfile {
    id: ZecProfileId,
    zcash_network: NetworkType,
    consensus_branch_id: BranchId,
    lez_confirmations: u32,
    zcash_confirmations: u32,
    lez_refund_delay: UnixSeconds,
    zcash_refund_blocks: u32,
    required_margin: UnixSeconds,
    expiry_delta_blocks: u32,
}

impl ZecRefundProfile {
    /// Resolves the exact immutable parameters for a named profile.
    #[must_use]
    pub const fn for_id(id: ZecProfileId) -> Self {
        match id {
            ZecProfileId::DeterministicLocalV1 => Self {
                id,
                zcash_network: NetworkType::Regtest,
                consensus_branch_id: BranchId::Nu6_2,
                lez_confirmations: 1,
                zcash_confirmations: 1,
                lez_refund_delay: UnixSeconds::new(60),
                zcash_refund_blocks: 4,
                required_margin: UnixSeconds::new(30),
                expiry_delta_blocks: 40,
            },
            ZecProfileId::DeterministicLocalDevnetV1 => Self {
                id,
                zcash_network: NetworkType::Regtest,
                consensus_branch_id: BranchId::Nu6_2,
                lez_confirmations: 1,
                zcash_confirmations: 1,
                lez_refund_delay: UnixSeconds::new(1_800),
                zcash_refund_blocks: 4,
                required_margin: UnixSeconds::new(30),
                expiry_delta_blocks: 40,
            },
            ZecProfileId::DeterministicLocalDevnetV2 => Self {
                id,
                zcash_network: NetworkType::Regtest,
                consensus_branch_id: BranchId::Nu6_2,
                lez_confirmations: 1,
                zcash_confirmations: 1,
                lez_refund_delay: UnixSeconds::new(1_800),
                // Two swaps of one signer taken together (zec-concurrent, the
                // BTC shape) share one Zcash anchor: the first swap's funding,
                // its two pre-reveal confirmations and its claim take four
                // blocks, and the second swap's lock re-plans behind them (D1,
                // 2026-10-29). Four blocks held one swap; eight hold both,
                // with the harness mining one block per reason.
                zcash_refund_blocks: 8,
                required_margin: UnixSeconds::new(30),
                expiry_delta_blocks: 40,
            },
            ZecProfileId::DeterministicLocalDevnetV3 => Self {
                id,
                zcash_network: NetworkType::Regtest,
                consensus_branch_id: BranchId::Nu6_2,
                lez_confirmations: 1,
                zcash_confirmations: 1,
                lez_refund_delay: UnixSeconds::new(720),
                zcash_refund_blocks: 8,
                required_margin: UnixSeconds::new(30),
                expiry_delta_blocks: 40,
            },
            ZecProfileId::PublicTestnetV1 => Self {
                id,
                zcash_network: NetworkType::Test,
                consensus_branch_id: BranchId::Nu6_2,
                lez_confirmations: 2,
                zcash_confirmations: 10,
                lez_refund_delay: UnixSeconds::new(7_200),
                zcash_refund_blocks: 192,
                required_margin: UnixSeconds::new(7_200),
                expiry_delta_blocks: 40,
            },
        }
    }

    /// Stable profile identifier.
    #[must_use]
    pub const fn id(self) -> ZecProfileId {
        self.id
    }

    /// Required Zcash network.
    #[must_use]
    pub const fn zcash_network(self) -> NetworkType {
        self.zcash_network
    }

    /// Required Zcash transaction consensus branch.
    #[must_use]
    pub const fn consensus_branch_id(self) -> BranchId {
        self.consensus_branch_id
    }

    /// LEZ confirmations required before a dependent action.
    #[must_use]
    pub const fn lez_confirmations(self) -> u32 {
        self.lez_confirmations
    }

    /// Zcash confirmations required before a dependent action.
    #[must_use]
    pub const fn zcash_confirmations(self) -> u32 {
        self.zcash_confirmations
    }

    /// Delay from LEZ funding inclusion to its earlier refund.
    #[must_use]
    pub const fn lez_refund_delay(self) -> UnixSeconds {
        self.lez_refund_delay
    }

    /// Blocks from Zcash funding inclusion to its later CLTV refund.
    #[must_use]
    pub const fn zcash_refund_blocks(self) -> u32 {
        self.zcash_refund_blocks
    }

    /// Required conservative cross-chain reaction margin.
    #[must_use]
    pub const fn required_margin(self) -> UnixSeconds {
        self.required_margin
    }

    /// Transaction expiry delta; independent of the HTLC refund boundary.
    #[must_use]
    pub const fn expiry_delta_blocks(self) -> u32 {
        self.expiry_delta_blocks
    }

    /// Verifies the actual node network and signing branch against the profile.
    ///
    /// # Errors
    ///
    /// Returns a network or consensus-branch mismatch before funds are locked.
    pub fn validate_consensus(
        self,
        network: NetworkType,
        branch_id: BranchId,
    ) -> Result<(), ProfileError> {
        if network != self.zcash_network {
            return Err(ProfileError::NetworkMismatch);
        }
        if branch_id != self.consensus_branch_id {
            return Err(ProfileError::ConsensusBranchMismatch);
        }
        Ok(())
    }

    /// Constructs the exact LEZ guest deadline from a whole-second funding timestamp.
    ///
    /// # Errors
    ///
    /// Returns [`ProfileError::TimestampOverflow`] on addition or conversion overflow.
    pub const fn lez_refund_at(
        self,
        funding_time: UnixSeconds,
    ) -> Result<LezUnixMilliseconds, ProfileError> {
        let Some(deadline) = funding_time
            .value()
            .checked_add(self.lez_refund_delay.value())
        else {
            return Err(ProfileError::TimestampOverflow);
        };
        match UnixSeconds::new(deadline).checked_to_lez_milliseconds() {
            Ok(timestamp) => Ok(timestamp),
            Err(_) => Err(ProfileError::TimestampOverflow),
        }
    }

    /// Constructs the exact Zcash CLTV height from the funding inclusion height.
    ///
    /// # Errors
    ///
    /// Returns [`ProfileError::HeightOverflow`] rather than wrapping.
    pub fn zcash_refund_at(self, funding_height: BlockHeight) -> Result<BlockHeight, ProfileError> {
        u32::from(funding_height)
            .checked_add(self.zcash_refund_blocks)
            .map(BlockHeight::from_u32)
            .ok_or(ProfileError::HeightOverflow)
    }

    /// Builds a direction-aware schedule after validating calibrated wall-clock bounds.
    ///
    /// `later_refund_earliest` must come from measured configuration or a controlled
    /// deterministic harness. It is never inferred from Zcash's nominal block target.
    ///
    /// # Errors
    ///
    /// Returns a missing-calibration, insufficient-margin, or invalid-schedule error.
    pub fn recovery_schedule(
        self,
        direction: SwapDirection,
        lez_refund_at: UnixSeconds,
        zcash_refund_at: BlockHeight,
        earlier_refund_latest: LezUnixMilliseconds,
        later_refund_earliest: Option<UnixSeconds>,
    ) -> Result<RecoverySchedule, ProfileError> {
        let later_refund_earliest =
            later_refund_earliest.ok_or(ProfileError::MissingSafetyCalibration)?;
        let safety = TimelockSafety::between(
            Chain::Lez,
            Chain::Zcash,
            earlier_refund_latest.to_unix_seconds_ceil().value(),
            later_refund_earliest.value(),
            self.required_margin.value(),
        )
        .map_err(|_| ProfileError::InsufficientSafetyMargin)?;
        let lez = ChainPosition::timestamp(Chain::Lez, lez_refund_at.value());
        let zcash = ChainPosition::block_height(Chain::Zcash, u64::from(zcash_refund_at));
        let (maker, taker) = match direction {
            SwapDirection::TakerSellsForeign => (lez, zcash),
            SwapDirection::TakerSellsLez => (zcash, lez),
        };
        RecoverySchedule::new(lez_swap_core::Pair::Zcash, direction, maker, taker, safety)
            .map_err(ProfileError::InvalidSchedule)
    }
}
