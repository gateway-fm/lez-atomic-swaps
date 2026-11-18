//! Immutable network-bound Zcash recovery profiles.

use lez_swap_core::{
    Chain, ChainPosition, LezUnixMilliseconds, RecoverySchedule, SwapDirection, TimelockSafety,
    UnixSeconds,
};
use zcash_protocol::consensus::{BlockHeight, BranchId, NetworkType};

/// The fastest Zcash Testnet pace a public profile's refund window is sized
/// for, milliseconds a block: 1,000 blocks in 3,922 s under Testnet's
/// min-difficulty rule, measured just before NU7 (2026-11-11). A Testnet
/// provisioner states the Zcash refund's earliest wall time with it.
pub const PUBLIC_TESTNET_FASTEST_BLOCK_MILLIS: u64 = 3_900;

/// The slowest Zcash Testnet pace a Taker-reveals-first public profile is
/// sized for, milliseconds a block: three times NU7's 25 s target, over three
/// times the 22.7 s a block Testnet averaged from NU7 to 2026-11-11. In
/// `TakerSellsLez` the Maker's Zcash refund is the earlier one; its latest
/// wall time is the Zcash anchor plus the refund window at this pace.
pub const PUBLIC_TESTNET_SLOWEST_BLOCK_MILLIS: u64 = 75_000;

/// Which participant publishes the secret first under a profile.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ZecRevealOrder {
    /// Profiles up to `deterministic-local-devnet-v6` and `public-testnet-v2`:
    /// the LEZ recipient claims first and the Zcash refund is the later one.
    /// Safe only in `TakerSellsForeign`, where the LEZ recipient is the
    /// first-locking Taker; a `TakerSellsLez` agreement under such a profile is
    /// refused.
    LezRecipientFirst,
    /// The common flow (AGENTS.md): the Taker holds the secret and claims the
    /// Maker-funded leg first in both directions; the Maker-funded leg refunds
    /// first.
    TakerFirst,
}

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
    /// [`Self::DeterministicLocalDevnetV3`] with a 4-minute LEZ refund delay,
    /// for the devnet's refund journeys alone: they never claim, so the delay
    /// only has to outlast provisioning (~45 s), the take and both first
    /// locks (~2 min to the Taker's claim offer once the indexer serves
    /// historical reads in milliseconds, 2026-11-10). V3's 12 minutes were
    /// sized for a take→claim of ≈7 min, which slow indexer reads caused, and
    /// spent ~8 of them waiting in every refund journey. Swaps that claim
    /// keep V3. Devnet only.
    DeterministicLocalDevnetV4,
    /// LEZ 0.2 and Zcash public-testnet acceptance profile.
    PublicTestnetV1,
    /// [`Self::PublicTestnetV1`] on Zcash Testnet's NU7 (branch `77190ad9`,
    /// active since height 4,465,026), with the Zcash refund window sized for
    /// Testnet's measured pace. V1 signs for NU6.2, which Testnet left at
    /// 4,134,000, and its 192 blocks last ~73 min at the 22.7 s a block
    /// Testnet averaged after NU7, far less than the four hours its own
    /// schedule asks for (2026-11-11). Same LEZ parameters and confirmations.
    PublicTestnetV2,
    /// [`Self::DeterministicLocalV1`] on Regtest's NU7 (branch `77190ad9`,
    /// active from height 1 on the pinned Zebra 7 fixture). The Regtest
    /// fixtures moved from Zebra 5.2 (NU6.2) to Zebra 7 so that they sign
    /// for the branch public Testnet runs; V1 keeps NU6.2 under its id.
    /// Every other parameter is V1's: Regtest mines on demand, so NU7's
    /// 25 s target changes no window (2026-11-11).
    DeterministicLocalV2,
    /// [`Self::DeterministicLocalDevnetV3`] on Regtest's NU7: the devnet's
    /// profile for swaps that claim, once its Zebra runs NU7. V3's 12-minute
    /// LEZ refund delay, eight-block Zcash refund window and 40-block expiry.
    DeterministicLocalDevnetV5,
    /// [`Self::DeterministicLocalDevnetV4`] on Regtest's NU7: the devnet's
    /// refund journeys' profile (4-minute LEZ refund delay), as V5 is V3's.
    DeterministicLocalDevnetV6,
    /// The isolated test lane's Taker-reveals-first profile (Regtest NU7):
    /// in `TakerSellsLez` the Maker's Zcash refund opens first (8 blocks the
    /// lane mines), the Taker's LEZ refund 120 s after the anchor, and the
    /// Taker claims ZEC only while 2 blocks and 30 s remain.
    DeterministicLocalV3,
    /// The devnet's Taker-reveals-first profile for swaps that claim
    /// (Regtest NU7): [`Self::DeterministicLocalDevnetV5`]'s 12-minute LEZ
    /// refund delay with a 16-block Zcash window (two swaps of one signer at
    /// one anchor use 8 blocks before their claims) and a 120 s margin for
    /// the Maker's LEZ claim after the Taker's Zcash claim; the Taker claims
    /// only while 3 blocks remain.
    DeterministicLocalDevnetV7,
    /// [`Self::DeterministicLocalDevnetV7`] with the 4-minute LEZ refund delay
    /// of the refund journeys, which never claim.
    DeterministicLocalDevnetV8,
    /// Zcash Testnet NU7 and the LEZ v0.2 testnet, Taker reveals first: in
    /// `TakerSellsLez` the Maker's Zcash refund (95 blocks) is reached by
    /// 2 h after the anchor at [`PUBLIC_TESTNET_SLOWEST_BLOCK_MILLIS`], the
    /// Taker's LEZ refund is 4 h after it, and the 2 h between give the Maker
    /// ten Zcash confirmations of the Taker's claim and its LEZ claim.
    PublicTestnetV3,
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
            Self::DeterministicLocalDevnetV4 => "deterministic-local-devnet-v4",
            Self::PublicTestnetV1 => "public-testnet-v1",
            Self::PublicTestnetV2 => "public-testnet-v2",
            Self::DeterministicLocalV2 => "deterministic-local-v2",
            Self::DeterministicLocalDevnetV5 => "deterministic-local-devnet-v5",
            Self::DeterministicLocalDevnetV6 => "deterministic-local-devnet-v6",
            Self::DeterministicLocalV3 => "deterministic-local-v3",
            Self::DeterministicLocalDevnetV7 => "deterministic-local-devnet-v7",
            Self::DeterministicLocalDevnetV8 => "deterministic-local-devnet-v8",
            Self::PublicTestnetV3 => "public-testnet-v3",
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
    /// The profile's reveal order is unsafe in this direction: a
    /// `TakerSellsLez` agreement under a LEZ-recipient-first profile would
    /// hand the preimage to the Maker while the Taker locks first.
    #[error("the profile's reveal order is unsafe for this swap direction")]
    UnsafeRevealOrder,
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
    reveal_order: ZecRevealOrder,
    zcash_reveal_margin_blocks: u32,
    zcash_slowest_block_millis: Option<u64>,
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
                reveal_order: ZecRevealOrder::LezRecipientFirst,
                zcash_reveal_margin_blocks: 0,
                zcash_slowest_block_millis: None,
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
                reveal_order: ZecRevealOrder::LezRecipientFirst,
                zcash_reveal_margin_blocks: 0,
                zcash_slowest_block_millis: None,
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
                reveal_order: ZecRevealOrder::LezRecipientFirst,
                zcash_reveal_margin_blocks: 0,
                zcash_slowest_block_millis: None,
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
                reveal_order: ZecRevealOrder::LezRecipientFirst,
                zcash_reveal_margin_blocks: 0,
                zcash_slowest_block_millis: None,
            },
            ZecProfileId::DeterministicLocalDevnetV4 => Self {
                id,
                zcash_network: NetworkType::Regtest,
                consensus_branch_id: BranchId::Nu6_2,
                lez_confirmations: 1,
                zcash_confirmations: 1,
                lez_refund_delay: UnixSeconds::new(240),
                zcash_refund_blocks: 8,
                required_margin: UnixSeconds::new(30),
                expiry_delta_blocks: 40,
                reveal_order: ZecRevealOrder::LezRecipientFirst,
                zcash_reveal_margin_blocks: 0,
                zcash_slowest_block_millis: None,
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
                reveal_order: ZecRevealOrder::LezRecipientFirst,
                zcash_reveal_margin_blocks: 0,
                zcash_slowest_block_millis: None,
            },
            ZecProfileId::PublicTestnetV2 => Self {
                id,
                zcash_network: NetworkType::Test,
                consensus_branch_id: BranchId::Nu7,
                lez_confirmations: 2,
                zcash_confirmations: 10,
                lez_refund_delay: UnixSeconds::new(7_200),
                // The Zcash refund (the later one) must not open before the
                // LEZ refund delay plus the margin, 4 h after funding, even
                // at the fastest pace Testnet has run: refund blocks times
                // PUBLIC_TESTNET_FASTEST_BLOCK_MILLIS >= LEZ refund delay +
                // required margin. Testnet's min-difficulty rule lets runs of
                // cheap blocks through: 1,000 blocks in 3,922 s just before
                // NU7 (3.9 s a block). 3,700 blocks x 3.9 s = 14,430 s >=
                // 14,400 s. At the 22.7 s a block Testnet averaged from NU7
                // to 2026-11-11 the window is ~23 h.
                zcash_refund_blocks: 3_700,
                required_margin: UnixSeconds::new(7_200),
                // NU7's 25 s target keeps librustzcash's 120-block default
                // expiry at the 50 min that 40 blocks of 75 s were.
                expiry_delta_blocks: 120,
                reveal_order: ZecRevealOrder::LezRecipientFirst,
                zcash_reveal_margin_blocks: 0,
                zcash_slowest_block_millis: None,
            },
            // The NU7 Regtest profiles: their NU6.2 namesakes' parameters,
            // NU7's branch. Expiry stays 40 blocks: on Regtest blocks come
            // when the harness mines them, not every 25 s.
            ZecProfileId::DeterministicLocalV2 => {
                Self::for_id(ZecProfileId::DeterministicLocalV1).on_regtest_nu7(id)
            }
            ZecProfileId::DeterministicLocalDevnetV5 => {
                Self::for_id(ZecProfileId::DeterministicLocalDevnetV3).on_regtest_nu7(id)
            }
            ZecProfileId::DeterministicLocalDevnetV6 => {
                Self::for_id(ZecProfileId::DeterministicLocalDevnetV4).on_regtest_nu7(id)
            }
            // Taker reveals first. Regtest: the harness mines every block, so
            // the earlier refund's latest wall time is declared by the
            // provisioner (no pace bound).
            ZecProfileId::DeterministicLocalV3 => Self {
                id,
                zcash_network: NetworkType::Regtest,
                consensus_branch_id: BranchId::Nu7,
                lez_confirmations: 1,
                zcash_confirmations: 1,
                lez_refund_delay: UnixSeconds::new(120),
                zcash_refund_blocks: 8,
                required_margin: UnixSeconds::new(30),
                expiry_delta_blocks: 40,
                reveal_order: ZecRevealOrder::TakerFirst,
                zcash_reveal_margin_blocks: 2,
                zcash_slowest_block_millis: None,
            },
            ZecProfileId::DeterministicLocalDevnetV7 => Self {
                id,
                zcash_network: NetworkType::Regtest,
                consensus_branch_id: BranchId::Nu7,
                lez_confirmations: 1,
                zcash_confirmations: 1,
                lez_refund_delay: UnixSeconds::new(720),
                zcash_refund_blocks: 16,
                required_margin: UnixSeconds::new(120),
                expiry_delta_blocks: 40,
                reveal_order: ZecRevealOrder::TakerFirst,
                zcash_reveal_margin_blocks: 3,
                zcash_slowest_block_millis: None,
            },
            ZecProfileId::DeterministicLocalDevnetV8 => Self {
                id,
                lez_refund_delay: UnixSeconds::new(240),
                ..Self::for_id(ZecProfileId::DeterministicLocalDevnetV7)
            },
            ZecProfileId::PublicTestnetV3 => Self {
                id,
                zcash_network: NetworkType::Test,
                consensus_branch_id: BranchId::Nu7,
                lez_confirmations: 2,
                zcash_confirmations: 10,
                // Earlier refund (Zcash, TakerSellsLez): the anchor is the tip
                // plus one, so the refund height is 96 blocks away at most;
                // 96 x 75 s = 7,200 s. Later refund (LEZ): 7,200 s + the
                // 7,200 s margin = 14,400 s.
                lez_refund_delay: UnixSeconds::new(14_400),
                zcash_refund_blocks: 95,
                required_margin: UnixSeconds::new(7_200),
                expiry_delta_blocks: 120,
                reveal_order: ZecRevealOrder::TakerFirst,
                zcash_reveal_margin_blocks: 10,
                zcash_slowest_block_millis: Some(PUBLIC_TESTNET_SLOWEST_BLOCK_MILLIS),
            },
        }
    }

    /// A Regtest NU6.2 profile's parameters under `id` on Regtest's NU7.
    const fn on_regtest_nu7(self, id: ZecProfileId) -> Self {
        Self {
            id,
            consensus_branch_id: BranchId::Nu7,
            ..self
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

    /// Which participant publishes the secret first under this profile.
    #[must_use]
    pub const fn reveal_order(self) -> ZecRevealOrder {
        self.reveal_order
    }

    /// Whether this profile admits the direction: every profile admits
    /// `TakerSellsForeign`; only Taker-reveals-first profiles admit
    /// `TakerSellsLez`.
    #[must_use]
    pub const fn admits(self, direction: SwapDirection) -> bool {
        matches!(
            (direction, self.reveal_order),
            (SwapDirection::TakerSellsForeign, _)
                | (SwapDirection::TakerSellsLez, ZecRevealOrder::TakerFirst)
        )
    }

    /// Blocks that must remain before the Zcash refund height when the Taker
    /// sends its revealing Zcash claim (`TakerSellsLez`): the claim is sent
    /// only while `tip + margin < refund height`, so it is buried that deep
    /// before the Maker's refund can be mined.
    #[must_use]
    pub const fn zcash_reveal_margin_blocks(self) -> u32 {
        self.zcash_reveal_margin_blocks
    }

    /// The slowest Zcash pace, milliseconds a block, a public profile's
    /// earlier Zcash refund is sized for; `None` on Regtest, where the
    /// harness mines every block.
    #[must_use]
    pub const fn zcash_slowest_block_millis(self) -> Option<u64> {
        self.zcash_slowest_block_millis
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
    /// The Maker-funded leg's refund is the earlier one: LEZ in `TakerSellsForeign`,
    /// Zcash in `TakerSellsLez`. `earlier_refund_latest` is the latest wall time the
    /// earlier refund can open; `later_refund_earliest` the earliest wall time the later
    /// one can. Both must come from measured configuration or a controlled deterministic
    /// harness; neither is inferred from Zcash's nominal block target.
    ///
    /// # Errors
    ///
    /// Returns an unsafe-reveal-order, missing-calibration, insufficient-margin, or
    /// invalid-schedule error.
    pub fn recovery_schedule(
        self,
        direction: SwapDirection,
        lez_refund_at: UnixSeconds,
        zcash_refund_at: BlockHeight,
        earlier_refund_latest: LezUnixMilliseconds,
        later_refund_earliest: Option<UnixSeconds>,
    ) -> Result<RecoverySchedule, ProfileError> {
        if !self.admits(direction) {
            return Err(ProfileError::UnsafeRevealOrder);
        }
        let later_refund_earliest =
            later_refund_earliest.ok_or(ProfileError::MissingSafetyCalibration)?;
        let (earlier_chain, later_chain) = match direction {
            SwapDirection::TakerSellsForeign => (Chain::Lez, Chain::Zcash),
            SwapDirection::TakerSellsLez => (Chain::Zcash, Chain::Lez),
        };
        let safety = TimelockSafety::between(
            earlier_chain,
            later_chain,
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
