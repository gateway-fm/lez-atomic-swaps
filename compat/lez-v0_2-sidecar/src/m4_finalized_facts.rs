use indexer_service_protocol::{BedrockStatus, Block};
use lez_bridge_protocol::{ChainClock, Hex32};
use nssa::AccountId;
use serde::Serialize;

use crate::{BridgeRuntimeError, FinalizedIndexerApi, HistoricalAccount, M4StageAFinalizedNonces};

/// Exact checked M4 escrow image identifier from the source-controlled deployment manifest.
pub const CHECKED_M4_ESCROW_PROGRAM_ID_HEX: &str =
    "4d6590332948743c2db88a183755815354ef92560550cd206ac27bddeea12c82";

/// Word encoding of [`CHECKED_M4_ESCROW_PROGRAM_ID_HEX`] accepted by official LEZ v0.2.
pub const CHECKED_M4_ESCROW_PROGRAM_ID: [u32; 8] = [
    0x3390_654d,
    0x3c74_4829,
    0x188a_b82d,
    0x5381_5537,
    0x5692_ef54,
    0x20cd_5005,
    0xdd7b_c26a,
    0x822c_a1ee,
];

const FINALIZED_SNAPSHOT_BRACKET: &str =
    "fixed_finalized_anchor_genesis_and_tip_reread_by_id_and_hash_latest_tip_monotonic";

/// The four role-bound accounts whose finalized nonces define the M4 future-message schedule.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct M4FinalizedAccountIds {
    maker_owner: AccountId,
    taker_owner: AccountId,
    claim_authority: AccountId,
    refund_authority: AccountId,
}

impl M4FinalizedAccountIds {
    /// Binds all four account roles before any indexer observation occurs.
    #[must_use]
    pub const fn new(
        maker_owner: AccountId,
        taker_owner: AccountId,
        claim_authority: AccountId,
        refund_authority: AccountId,
    ) -> Self {
        Self {
            maker_owner,
            taker_owner,
            claim_authority,
            refund_authority,
        }
    }
}

/// Whether a finalized nonce came from an indexed account or the protocol's absent-account zero.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum M4FinalizedAccountPresence {
    /// The account existed at the pinned finalized block.
    Present,
    /// The account was absent at the pinned finalized block, so its nonce is exactly zero.
    AbsentDefaultNonce,
}

/// One role-bound account nonce with its exact finalized provenance.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct M4FinalizedAccountNonce {
    account_id: Hex32,
    nonce: u128,
    presence: M4FinalizedAccountPresence,
}

impl M4FinalizedAccountNonce {
    /// Returns the exact role-bound account identity.
    pub const fn account_id(&self) -> Hex32 {
        self.account_id
    }

    /// Returns the nonce at the pinned finalized block.
    #[must_use]
    pub const fn nonce(&self) -> u128 {
        self.nonce
    }

    /// Returns whether the nonce came from present or canonical absent-account state.
    #[must_use]
    pub const fn presence(&self) -> M4FinalizedAccountPresence {
        self.presence
    }
}

/// Stable, genesis-bound finalized nonce facts for the four M4 actor accounts.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StableM4FinalizedNonceSnapshot {
    finalized_clock: ChainClock,
    genesis_block_hash: Hex32,
    maker_owner: M4FinalizedAccountNonce,
    taker_owner: M4FinalizedAccountNonce,
    claim_authority: M4FinalizedAccountNonce,
    refund_authority: M4FinalizedAccountNonce,
    bracket: &'static str,
}

impl StableM4FinalizedNonceSnapshot {
    /// Returns the consensus clock committed by the pinned finalized block.
    pub const fn finalized_clock(&self) -> ChainClock {
        self.finalized_clock
    }

    /// Returns the exact expected and independently observed genesis hash.
    pub const fn genesis_block_hash(&self) -> Hex32 {
        self.genesis_block_hash
    }

    /// Returns the Maker owner nonce evidence.
    #[must_use]
    pub const fn maker_owner(&self) -> M4FinalizedAccountNonce {
        self.maker_owner
    }

    /// Returns the Taker owner nonce evidence.
    #[must_use]
    pub const fn taker_owner(&self) -> M4FinalizedAccountNonce {
        self.taker_owner
    }

    /// Returns the claim-authority nonce evidence.
    #[must_use]
    pub const fn claim_authority(&self) -> M4FinalizedAccountNonce {
        self.claim_authority
    }

    /// Returns the refund-authority nonce evidence.
    #[must_use]
    pub const fn refund_authority(&self) -> M4FinalizedAccountNonce {
        self.refund_authority
    }

    /// Projects the exact four nonces into the Stage-A future-message planner input.
    #[must_use]
    pub const fn planned_nonces(&self) -> M4StageAFinalizedNonces {
        M4StageAFinalizedNonces::new(
            self.maker_owner.nonce,
            self.taker_owner.nonce,
            self.claim_authority.nonce,
            self.refund_authority.nonce,
        )
    }
}

/// Reads a stable four-account finalized snapshot bound to the expected genesis.
///
/// The function fixes one finalized anchor, verifies genesis and that anchor independently by
/// both ID and hash before and after the account reads, and rejects a regressing finalized height.
/// A newer finalized height after the pinned reads is safe and does not change the returned anchor.
///
/// Maker and Taker owner accounts must exist. The not-yet-created claim and refund authorities use
/// the LEZ canonical absent-account nonce of zero.
///
/// # Errors
///
/// Returns [`BridgeRuntimeError::Unavailable`] when required finalized data is absent,
/// [`BridgeRuntimeError::InvalidObservation`] for a wrong genesis or malformed block/account fact,
/// and [`BridgeRuntimeError::MovingTip`] when the pinned view changes or finality regresses.
pub async fn read_stable_m4_finalized_nonce_snapshot(
    indexer: &dyn FinalizedIndexerApi,
    expected_genesis_hash: Hex32,
    accounts: M4FinalizedAccountIds,
) -> Result<StableM4FinalizedNonceSnapshot, BridgeRuntimeError> {
    let finalized_before = indexer
        .last_finalized_block_id()
        .await?
        .ok_or(BridgeRuntimeError::Unavailable)?;
    let genesis_before = read_finalized_block(indexer, nssa::GENESIS_BLOCK_ID).await?;
    if genesis_before.header.hash.0 != *expected_genesis_hash.as_bytes() {
        return Err(BridgeRuntimeError::InvalidObservation);
    }
    let tip_before = read_finalized_block(indexer, finalized_before).await?;
    if tip_before.header.hash.0 == [0; 32] || tip_before.header.timestamp == 0 {
        return Err(BridgeRuntimeError::InvalidObservation);
    }

    let maker = finalized_nonce(indexer, accounts.maker_owner, finalized_before, true).await?;
    let taker = finalized_nonce(indexer, accounts.taker_owner, finalized_before, true).await?;
    let claim = finalized_nonce(indexer, accounts.claim_authority, finalized_before, false).await?;
    let refund =
        finalized_nonce(indexer, accounts.refund_authority, finalized_before, false).await?;

    let genesis_after = read_finalized_block(indexer, nssa::GENESIS_BLOCK_ID).await?;
    let tip_after = read_finalized_block(indexer, finalized_before).await?;
    let finalized_after = indexer
        .last_finalized_block_id()
        .await?
        .ok_or(BridgeRuntimeError::Unavailable)?;
    if genesis_after != genesis_before || tip_after != tip_before {
        return Err(BridgeRuntimeError::MovingTip);
    }
    if finalized_after < finalized_before {
        return Err(BridgeRuntimeError::MovingTip);
    }

    Ok(StableM4FinalizedNonceSnapshot {
        finalized_clock: ChainClock::new(
            Hex32::from_bytes(tip_before.header.hash.0),
            tip_before.header.block_id,
            tip_before.header.timestamp,
        ),
        genesis_block_hash: expected_genesis_hash,
        maker_owner: maker,
        taker_owner: taker,
        claim_authority: claim,
        refund_authority: refund,
        bracket: FINALIZED_SNAPSHOT_BRACKET,
    })
}

/// Accepts only the source-controlled checked M4 escrow `ProgramID`.
///
/// # Errors
///
/// Returns [`BridgeRuntimeError::InvalidObservation`] for every other `ProgramID`.
pub fn validate_checked_m4_escrow_program_id(
    program_id: [u32; 8],
) -> Result<(), BridgeRuntimeError> {
    if program_id == CHECKED_M4_ESCROW_PROGRAM_ID {
        Ok(())
    } else {
        Err(BridgeRuntimeError::InvalidObservation)
    }
}

async fn read_finalized_block(
    indexer: &dyn FinalizedIndexerApi,
    block_id: u64,
) -> Result<Block, BridgeRuntimeError> {
    let by_id = indexer
        .block_by_id(block_id)
        .await?
        .ok_or(BridgeRuntimeError::Unavailable)?;
    if by_id.header.block_id != block_id || by_id.bedrock_status != BedrockStatus::Finalized {
        return Err(BridgeRuntimeError::InvalidObservation);
    }
    let by_hash = indexer
        .block_by_hash(by_id.header.hash.0)
        .await?
        .ok_or(BridgeRuntimeError::Unavailable)?;
    if by_hash != by_id {
        return Err(BridgeRuntimeError::InvalidObservation);
    }
    Ok(by_id)
}

async fn finalized_nonce(
    indexer: &dyn FinalizedIndexerApi,
    account_id: AccountId,
    block_id: u64,
    require_present: bool,
) -> Result<M4FinalizedAccountNonce, BridgeRuntimeError> {
    let account_id_hex = Hex32::from_bytes(account_id.into_value());
    match indexer
        .account_at_block(account_id.into_value(), block_id)
        .await?
    {
        HistoricalAccount::Present(account) => Ok(M4FinalizedAccountNonce {
            account_id: account_id_hex,
            nonce: account.nonce,
            presence: M4FinalizedAccountPresence::Present,
        }),
        HistoricalAccount::Absent if !require_present => Ok(M4FinalizedAccountNonce {
            account_id: account_id_hex,
            nonce: 0,
            presence: M4FinalizedAccountPresence::AbsentDefaultNonce,
        }),
        HistoricalAccount::Absent => Err(BridgeRuntimeError::InvalidObservation),
    }
}
