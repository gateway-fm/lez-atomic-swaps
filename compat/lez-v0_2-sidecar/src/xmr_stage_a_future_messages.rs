//! Deterministic official LEZ messages committed before M4 Stage A.

use nssa::{AccountId, PublicKey, public_transaction::Message};

use crate::native_prepare::{
    ClaimNativeXmrAccounts, PunishNativeXmrAccounts, RefundNativeXmrAccounts, ZecEscrowInstruction,
    compute_custody_pda, compute_metadata_pda,
};

/// Stable finalized nonces needed before an M4 Stage-A agreement commits its
/// future claim, signed-refund, and punishment message hashes.
///
/// Owner accounts and aggregate-authority accounts are distinct LEZ accounts,
/// so every nonce must be observed independently in one stable finalized view.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct M4StageAFinalizedNonces {
    maker_owner: u128,
    taker_owner: u128,
    claim_authority: u128,
    refund_authority: u128,
}

impl M4StageAFinalizedNonces {
    /// Creates one stable finalized nonce snapshot.
    #[must_use]
    pub const fn new(
        maker_owner: u128,
        taker_owner: u128,
        claim_authority: u128,
        refund_authority: u128,
    ) -> Self {
        Self {
            maker_owner,
            taker_owner,
            claim_authority,
            refund_authority,
        }
    }
}

/// Public inputs needed to plan all three immutable Stage-A LEZ messages.
///
/// Planning happens before `XmrAgreementV1` and Stage B. It performs no node
/// calls and accepts no precomputed or placeholder message hashes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct M4StageAFutureMessageInput {
    escrow_program_id: [u32; 8],
    swap_id: [u8; 32],
    maker_owner: AccountId,
    taker_owner: AccountId,
    claim_aggregate_x_only_public_key: [u8; 32],
    refund_aggregate_x_only_public_key: [u8; 32],
    finalized_nonces: M4StageAFinalizedNonces,
}

impl M4StageAFutureMessageInput {
    /// Creates an untrusted future-message input.
    #[allow(clippy::too_many_arguments)]
    #[must_use]
    pub const fn new(
        escrow_program_id: [u32; 8],
        swap_id: [u8; 32],
        maker_owner: AccountId,
        taker_owner: AccountId,
        claim_aggregate_x_only_public_key: [u8; 32],
        refund_aggregate_x_only_public_key: [u8; 32],
        finalized_nonces: M4StageAFinalizedNonces,
    ) -> Self {
        Self {
            escrow_program_id,
            swap_id,
            maker_owner,
            taker_owner,
            claim_aggregate_x_only_public_key,
            refund_aggregate_x_only_public_key,
            finalized_nonces,
        }
    }
}

/// Checked nonce schedule committed by a Stage-A future-message plan.
///
/// The Taker's finalized owner nonce is consumed by generated tag 13
/// `InitializeNativeXmr`; the immediately following `FundNative` consumes
/// the next nonce, and tag 14 `AuthorizeNativeXmrClaim` consumes the nonce
/// after Fund. Claim and refund use independently finalized aggregate-authority
/// nonces. The Maker has no preceding owner-signed happy-path effect, so its
/// finalized owner nonce is used by punishment.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct M4StageAPlannedNonces {
    maker_owner_finalized: u128,
    taker_owner_finalized: u128,
    initialize: u128,
    fund: u128,
    authorize: u128,
    claim: u128,
    refund: u128,
    punish: u128,
}

impl M4StageAPlannedNonces {
    /// Stable Maker owner nonce from the finalized snapshot.
    #[must_use]
    pub const fn maker_owner_finalized(self) -> u128 {
        self.maker_owner_finalized
    }

    /// Stable Taker owner nonce from the finalized snapshot.
    #[must_use]
    pub const fn taker_owner_finalized(self) -> u128 {
        self.taker_owner_finalized
    }

    /// Taker nonce planned for generated tag-13 initialization.
    #[must_use]
    pub const fn initialize(self) -> u128 {
        self.initialize
    }

    /// Taker nonce planned for the immediately following native funding.
    #[must_use]
    pub const fn fund(self) -> u128 {
        self.fund
    }

    /// Taker nonce planned for generated tag-14 authorization.
    #[must_use]
    pub const fn authorize(self) -> u128 {
        self.authorize
    }

    /// Independently finalized claim aggregate-authority nonce.
    #[must_use]
    pub const fn claim(self) -> u128 {
        self.claim
    }

    /// Independently finalized refund aggregate-authority nonce.
    #[must_use]
    pub const fn refund(self) -> u128 {
        self.refund
    }

    /// Maker owner nonce planned for punishment.
    #[must_use]
    pub const fn punish(self) -> u128 {
        self.punish
    }
}

/// Exact generated official NSSA messages and hashes committed into M4 Stage A.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct M4StageAFutureMessagePlan {
    claim_authority: AccountId,
    refund_authority: AccountId,
    nonces: M4StageAPlannedNonces,
    claim_message: Message,
    refund_message: Message,
    punish_message: Message,
}

impl M4StageAFutureMessagePlan {
    /// Official account derived from the claim aggregate x-only key.
    #[must_use]
    pub const fn claim_authority(&self) -> AccountId {
        self.claim_authority
    }

    /// Official account derived from the refund aggregate x-only key.
    #[must_use]
    pub const fn refund_authority(&self) -> AccountId {
        self.refund_authority
    }

    /// Planned predecessor and settlement nonce schedule.
    #[must_use]
    pub const fn nonces(&self) -> M4StageAPlannedNonces {
        self.nonces
    }

    /// Exact unsigned generated tag-15 claim message.
    #[must_use]
    pub const fn claim_message(&self) -> &Message {
        &self.claim_message
    }

    /// Exact unsigned generated tag-16 signed-refund message.
    #[must_use]
    pub const fn refund_message(&self) -> &Message {
        &self.refund_message
    }

    /// Exact unsigned generated tag-17 punishment message.
    #[must_use]
    pub const fn punish_message(&self) -> &Message {
        &self.punish_message
    }

    /// Official NSSA hash committed as the Stage-A claim message.
    #[must_use]
    pub fn claim_hash(&self) -> [u8; 32] {
        self.claim_message.hash()
    }

    /// Official NSSA hash committed as the Stage-A signed-refund message.
    #[must_use]
    pub fn refund_hash(&self) -> [u8; 32] {
        self.refund_message.hash()
    }

    /// Official NSSA hash committed as the Stage-A punishment message.
    #[must_use]
    pub fn punish_hash(&self) -> [u8; 32] {
        self.punish_message.hash()
    }
}

/// Fail-closed errors from deterministic Stage-A future-message planning.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum M4StageAFutureMessagePlanError {
    /// Program, swap, owner, or authority identity is invalid or aliased.
    #[error("invalid or aliased M4 Stage-A identity")]
    InvalidIdentity,
    /// The planned Initialize/Fund/Authorize nonce sequence overflowed.
    #[error("M4 Stage-A owner nonce schedule overflowed")]
    NonceOverflow,
    /// The generated instruction could not form an official NSSA message.
    #[error("M4 Stage-A official message encoding failed")]
    MessageEncoding,
    /// Purpose-separated generated messages produced equal hashes.
    #[error("M4 Stage-A future-message hashes are not distinct")]
    HashCollision,
}

/// Builds generated tag-15/tag-16/tag-17 messages whose official NSSA hashes
/// become M4 Stage-A claim, refund, and punishment messages.
///
/// This pure boundary performs no nonce RPC, signing, persistence, or
/// submission. Callers must obtain all four nonces from one stable finalized
/// view and reserve the resulting accounts before countersigning Stage A.
///
/// # Errors
///
/// Rejects invalid or aliased identities, nonce overflow, official message
/// encoding failure, or a purpose-separation hash collision.
pub fn plan_m4_stage_a_future_messages(
    input: M4StageAFutureMessageInput,
) -> Result<M4StageAFutureMessagePlan, M4StageAFutureMessagePlanError> {
    let (claim_authority, refund_authority) = validate_identities(&input)?;
    let initialize = input.finalized_nonces.taker_owner;
    let fund = initialize
        .checked_add(1)
        .ok_or(M4StageAFutureMessagePlanError::NonceOverflow)?;
    let authorize = fund
        .checked_add(1)
        .ok_or(M4StageAFutureMessagePlanError::NonceOverflow)?;
    let nonces = M4StageAPlannedNonces {
        maker_owner_finalized: input.finalized_nonces.maker_owner,
        taker_owner_finalized: input.finalized_nonces.taker_owner,
        initialize,
        fund,
        authorize,
        claim: input.finalized_nonces.claim_authority,
        refund: input.finalized_nonces.refund_authority,
        punish: input.finalized_nonces.maker_owner,
    };

    let metadata = compute_metadata_pda(&input.escrow_program_id, &input.swap_id);
    let custody = compute_custody_pda(&input.escrow_program_id, &input.swap_id);
    let claim = ClaimNativeXmrAccounts {
        metadata,
        custody,
        claimant: input.maker_owner,
        claim_aggregate_authority: claim_authority,
    };
    let refund = RefundNativeXmrAccounts {
        metadata,
        custody,
        depositor: input.taker_owner,
        refund_aggregate_authority: refund_authority,
    };
    let punish = PunishNativeXmrAccounts {
        metadata,
        custody,
        claimant: input.maker_owner,
    };

    let claim_message = official_message(
        input.escrow_program_id,
        vec![
            claim.metadata,
            claim.custody,
            claim.claimant,
            claim.claim_aggregate_authority,
        ],
        nonces.claim,
        ZecEscrowInstruction::ClaimNativeXmr {
            swap_id: input.swap_id,
        },
    )?;
    let refund_message = official_message(
        input.escrow_program_id,
        vec![
            refund.metadata,
            refund.custody,
            refund.depositor,
            refund.refund_aggregate_authority,
        ],
        nonces.refund,
        ZecEscrowInstruction::RefundNativeXmr {
            swap_id: input.swap_id,
        },
    )?;
    let punish_message = official_message(
        input.escrow_program_id,
        vec![punish.metadata, punish.custody, punish.claimant],
        nonces.punish,
        ZecEscrowInstruction::PunishNativeXmr {
            swap_id: input.swap_id,
        },
    )?;

    let plan = M4StageAFutureMessagePlan {
        claim_authority,
        refund_authority,
        nonces,
        claim_message,
        refund_message,
        punish_message,
    };
    if plan.claim_hash() == plan.refund_hash()
        || plan.claim_hash() == plan.punish_hash()
        || plan.refund_hash() == plan.punish_hash()
    {
        return Err(M4StageAFutureMessagePlanError::HashCollision);
    }
    Ok(plan)
}

fn validate_identities(
    input: &M4StageAFutureMessageInput,
) -> Result<(AccountId, AccountId), M4StageAFutureMessagePlanError> {
    if input.escrow_program_id == [0; 8]
        || input.swap_id == [0; 32]
        || input.maker_owner == AccountId::new([0; 32])
        || input.taker_owner == AccountId::new([0; 32])
        || input.maker_owner == input.taker_owner
    {
        return Err(M4StageAFutureMessagePlanError::InvalidIdentity);
    }
    let claim_key = PublicKey::try_new(input.claim_aggregate_x_only_public_key)
        .map_err(|_| M4StageAFutureMessagePlanError::InvalidIdentity)?;
    let refund_key = PublicKey::try_new(input.refund_aggregate_x_only_public_key)
        .map_err(|_| M4StageAFutureMessagePlanError::InvalidIdentity)?;
    let claim_authority = AccountId::from(&claim_key);
    let refund_authority = AccountId::from(&refund_key);
    if claim_authority == refund_authority
        || [input.maker_owner, input.taker_owner].contains(&claim_authority)
        || [input.maker_owner, input.taker_owner].contains(&refund_authority)
    {
        return Err(M4StageAFutureMessagePlanError::InvalidIdentity);
    }
    Ok((claim_authority, refund_authority))
}
fn official_message(
    program_id: [u32; 8],
    accounts: Vec<AccountId>,
    nonce: u128,
    instruction: ZecEscrowInstruction,
) -> Result<Message, M4StageAFutureMessagePlanError> {
    Message::try_new(program_id, accounts, vec![nonce.into()], instruction)
        .map_err(|_| M4StageAFutureMessagePlanError::MessageEncoding)
}
