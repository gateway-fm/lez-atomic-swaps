//! Native SHA-256 HTLC escrow port for LEZ v0.2.0.
//!
//! Each state transition and its authenticated-transfer chained call are one
//! [`SpelOutput`]. LEZ validates the complete recursive execution before
//! committing it, so metadata cannot become terminal unless the exact custody
//! transfer also succeeds.

#![allow(dead_code)]

use authenticated_transfer_core::Instruction as AuthenticatedTransferInstruction;
use nssa_core::{
    account::{Account, AccountId, Data},
    program::{ChainedCall, Claim, DEFAULT_PROGRAM_ID, PdaSeed, ProgramId},
};
use sha2::{Digest, Sha256};
use spel_framework::prelude::*;
use token_core::{TokenDefinition, TokenHolding};

#[derive(Debug, Clone, Copy, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum EscrowStatus {
    Empty,
    Funded,
    Claimed,
    Refunded,
    /// M4: the Taker published the agreement-bound claim partial on LEZ.
    XmrClaimAuthorized,
}

#[account_type]
/// Immutable authority required to claim a funded escrow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum ClaimAuthority {
    /// Existing ZEC path: reveal the SHA-256 preimage in the instruction.
    Sha256Preimage { secret_digest: [u8; 32] },
    /// M3 BTC path: authorize the exact transaction as one aggregate LEZ account.
    AggregateWitness {
        x_only_public_key: [u8; 32],
        account_id: AccountId,
    },
    /// M4 XMR path: distinct aggregate witnesses reveal opposite spend shares.
    XmrDualAdaptor {
        claim_aggregate_x_only_public_key: [u8; 32],
        claim_aggregate_account_id: AccountId,
        refund_aggregate_x_only_public_key: [u8; 32],
        refund_aggregate_account_id: AccountId,
        maker_dleq_transcript_commitment: [u8; 32],
        taker_dleq_transcript_commitment: [u8; 32],
        claim_partial_commitment: [u8; 32],
        punish_at: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct EscrowMetadata {
    pub version: u8,
    pub swap_id: [u8; 32],
    pub terms_hash: [u8; 32],
    pub claim_authority: ClaimAuthority,
    pub depositor: AccountId,
    pub depositor_asset: AccountId,
    pub claimant: AccountId,
    pub claimant_asset: AccountId,
    pub custody: AccountId,
    pub asset_program: ProgramId,
    pub custody_program: ProgramId,
    pub asset_definition: [u8; 32],
    pub amount: u128,
    /// Exclusive claim/funding boundary and inclusive refund boundary, in LEZ
    /// Unix timestamp milliseconds.
    pub refund_at: u64,
    pub status: EscrowStatus,
}

const ERROR_INVALID_TERMS: u32 = 1;
const ERROR_NOT_FUNDED: u32 = 2;
const ERROR_ACCOUNT_BINDING: u32 = 3;
const ESCROW_METADATA_VERSION: u8 = 2;
const XMR_ESCROW_METADATA_VERSION: u8 = 3;
const ERROR_WRONG_PREIMAGE: u32 = 4;
const ERROR_UNSUPPORTED_VERSION: u32 = 5;
const ERROR_WRONG_CLAIM_AUTHORITY: u32 = 6;
const XMR_CLAIM_PARTIAL_DOMAIN: &[u8] = b"logos.gateway.lez-xmr.claim-partial.v1\0";
// lee_core exposes AccountId but not the public signing-key type. Pulling the
// full host-oriented lee state machine into the guest solely for this hash
// would expand the on-chain graph. Keep the exact mapping pinned to LEZ v0.2.0
// commit a58fbce2ff48c58b7bb5001b1a27e64b9596ee3a:
// lee/state_machine/src/signature/public_key.rs. The recursive witnessed-claim
// test cross-checks it against lee::AccountId::from(&lee::PublicKey).
const PUBLIC_ACCOUNT_ID_PREFIX: &[u8; 32] = b"/LEE/v0.3/AccountId/Public/\x00\x00\x00\x00\x00";

fn custom_error(code: u32, message: impl Into<String>) -> SpelError {
    SpelError::custom(code, message)
}

fn witnessed_account_id(x_only_public_key: &[u8; 32]) -> AccountId {
    let mut hasher = Sha256::new();
    hasher.update(PUBLIC_ACCOUNT_ID_PREFIX);
    hasher.update(x_only_public_key);
    AccountId::new(hasher.finalize().into())
}

fn valid_claim_authority(authority: ClaimAuthority, claimant: AccountId) -> bool {
    match authority {
        ClaimAuthority::Sha256Preimage { secret_digest } => secret_digest != [0; 32],
        ClaimAuthority::AggregateWitness {
            x_only_public_key,
            account_id,
        } => {
            x_only_public_key != [0; 32]
                && witnessed_account_id(&x_only_public_key) == account_id
                && account_id != claimant
        }
        ClaimAuthority::XmrDualAdaptor {
            claim_aggregate_x_only_public_key,
            claim_aggregate_account_id,
            refund_aggregate_x_only_public_key,
            refund_aggregate_account_id,
            maker_dleq_transcript_commitment,
            taker_dleq_transcript_commitment,
            claim_partial_commitment,
            punish_at,
        } => {
            claim_aggregate_x_only_public_key != [0; 32]
                && refund_aggregate_x_only_public_key != [0; 32]
                && claim_aggregate_x_only_public_key != refund_aggregate_x_only_public_key
                && witnessed_account_id(&claim_aggregate_x_only_public_key)
                    == claim_aggregate_account_id
                && witnessed_account_id(&refund_aggregate_x_only_public_key)
                    == refund_aggregate_account_id
                && claim_aggregate_account_id != refund_aggregate_account_id
                && claim_aggregate_account_id != claimant
                && refund_aggregate_account_id != claimant
                && maker_dleq_transcript_commitment != [0; 32]
                && taker_dleq_transcript_commitment != [0; 32]
                && maker_dleq_transcript_commitment != taker_dleq_transcript_commitment
                && claim_partial_commitment != [0; 32]
                && punish_at != 0
        }
    }
}

fn xmr_claim_partial_commitment(claim_partial: [u8; 32]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(XMR_CLAIM_PARTIAL_DOMAIN);
    hasher.update(claim_partial);
    hasher.finalize().into()
}

fn require_preimage_authority(state: &EscrowMetadata, preimage: [u8; 32]) -> Result<(), SpelError> {
    let ClaimAuthority::Sha256Preimage { secret_digest } = state.claim_authority else {
        return Err(custom_error(
            ERROR_WRONG_CLAIM_AUTHORITY,
            "escrow requires an aggregate transaction witness",
        ));
    };
    let digest: [u8; 32] = Sha256::digest(preimage).into();
    if digest != secret_digest {
        return Err(custom_error(ERROR_WRONG_PREIMAGE, "wrong preimage"));
    }
    Ok(())
}

fn require_aggregate_witness_authority(
    state: &EscrowMetadata,
    aggregate_authority: AccountId,
) -> Result<(), SpelError> {
    let ClaimAuthority::AggregateWitness {
        x_only_public_key,
        account_id,
    } = state.claim_authority
    else {
        return Err(custom_error(
            ERROR_WRONG_CLAIM_AUTHORITY,
            "escrow requires a SHA-256 preimage",
        ));
    };
    if witnessed_account_id(&x_only_public_key) != account_id
        || account_id != aggregate_authority
        || account_id == state.claimant
    {
        return Err(custom_error(
            ERROR_ACCOUNT_BINDING,
            "aggregate witness account binding mismatch",
        ));
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum XmrWitnessPurpose {
    Claim,
    Refund,
}

fn require_xmr_witness_authority(
    state: &EscrowMetadata,
    purpose: XmrWitnessPurpose,
    aggregate_authority: AccountId,
) -> Result<u64, SpelError> {
    let ClaimAuthority::XmrDualAdaptor {
        claim_aggregate_x_only_public_key,
        claim_aggregate_account_id,
        refund_aggregate_x_only_public_key,
        refund_aggregate_account_id,
        punish_at,
        ..
    } = state.claim_authority
    else {
        return Err(custom_error(
            ERROR_WRONG_CLAIM_AUTHORITY,
            "escrow is not an XMR dual-adaptor escrow",
        ));
    };
    let (public_key, account_id) = match purpose {
        XmrWitnessPurpose::Claim => (
            claim_aggregate_x_only_public_key,
            claim_aggregate_account_id,
        ),
        XmrWitnessPurpose::Refund => (
            refund_aggregate_x_only_public_key,
            refund_aggregate_account_id,
        ),
    };
    if witnessed_account_id(&public_key) != account_id
        || account_id != aggregate_authority
        || account_id == state.depositor
        || account_id == state.claimant
    {
        return Err(custom_error(
            ERROR_ACCOUNT_BINDING,
            "XMR aggregate witness account binding mismatch",
        ));
    }
    Ok(punish_at)
}

fn write_metadata(
    account: &mut AccountWithMetadata,
    metadata: &EscrowMetadata,
) -> Result<(), SpelError> {
    let bytes = borsh::to_vec(metadata).map_err(|error| SpelError::SerializationError {
        message: error.to_string(),
    })?;
    account.account.data =
        Data::try_from(bytes).map_err(|error| SpelError::SerializationError {
            message: error.to_string(),
        })?;
    Ok(())
}

fn read_metadata(account: &AccountWithMetadata) -> Result<EscrowMetadata, SpelError> {
    EscrowMetadata::try_from_slice(account.account.data.as_ref()).map_err(|error| {
        SpelError::DeserializationError {
            account_index: 0,
            message: error.to_string(),
        }
    })
}

fn custody_pda_seed(swap_id: &[u8; 32]) -> PdaSeed {
    let label = spel_framework::pda::seed_from_str("custody");
    match AutoClaim::pda_from_seeds(&[&label, swap_id]) {
        AutoClaim::Claimed(Claim::Pda(seed)) => seed,
        _ => unreachable!("multi-seed public PDA always produces a PDA claim"),
    }
}

fn metadata_pda_seed(swap_id: &[u8; 32]) -> PdaSeed {
    match AutoClaim::pda_from_seeds(&[swap_id]) {
        AutoClaim::Claimed(Claim::Pda(seed)) => seed,
        _ => unreachable!("public metadata PDA always produces a PDA claim"),
    }
}

fn associated_token_account(
    ata_program: ProgramId,
    owner: AccountId,
    definition: AccountId,
) -> AccountId {
    ata_core::get_associated_token_account_id(
        &ata_program,
        &ata_core::compute_ata_seed(owner, definition),
    )
}

fn token_holding(account: &AccountWithMetadata) -> Result<TokenHolding, SpelError> {
    TokenHolding::try_from(&account.account.data)
        .map_err(|_| custom_error(ERROR_ACCOUNT_BINDING, "invalid token holding"))
}

fn token_definition(account: &AccountWithMetadata) -> Result<AccountId, SpelError> {
    token_holding(account).map(|holding| holding.definition_id())
}

fn fungible_balance(account: &AccountWithMetadata) -> Result<u128, SpelError> {
    match token_holding(account)? {
        TokenHolding::Fungible { balance, .. } => Ok(balance),
        _ => Err(custom_error(
            ERROR_ACCOUNT_BINDING,
            "token holding must be fungible",
        )),
    }
}

fn require_fungible_definition(
    account: &AccountWithMetadata,
    token_program: ProgramId,
) -> Result<(), SpelError> {
    if account.account.program_owner != token_program
        || !matches!(
            TokenDefinition::try_from(&account.account.data),
            Ok(TokenDefinition::Fungible { .. })
        )
    {
        return Err(custom_error(
            ERROR_ACCOUNT_BINDING,
            "token definition must be fungible and token-program owned",
        ));
    }
    Ok(())
}

fn native_initialize_call(
    authenticated_transfer_program: ProgramId,
    mut custody: AccountWithMetadata,
    swap_id: &[u8; 32],
) -> ChainedCall {
    custody.is_authorized = true;
    ChainedCall::new(
        authenticated_transfer_program,
        vec![custody],
        &AuthenticatedTransferInstruction::Initialize,
    )
    .with_pda_seeds(vec![custody_pda_seed(swap_id)])
}

#[allow(clippy::too_many_arguments)]
fn native_initial_state(
    ctx: &ProgramContext,
    custody: &AccountWithMetadata,
    depositor: &AccountWithMetadata,
    claimant: &AccountWithMetadata,
    swap_id: [u8; 32],
    terms_hash: [u8; 32],
    claim_authority: ClaimAuthority,
    amount: u128,
    refund_at: u64,
    authenticated_transfer_program: ProgramId,
) -> Result<EscrowMetadata, SpelError> {
    if amount == 0
        || refund_at == 0
        || terms_hash == [0; 32]
        || !valid_claim_authority(claim_authority, claimant.account_id)
        || authenticated_transfer_program == DEFAULT_PROGRAM_ID
        || authenticated_transfer_program == ctx.self_program_id
    {
        return Err(custom_error(ERROR_INVALID_TERMS, "invalid native terms"));
    }
    if custody.account != Account::default()
        || depositor.account.program_owner != authenticated_transfer_program
        || claimant.account.program_owner != authenticated_transfer_program
    {
        return Err(custom_error(
            ERROR_ACCOUNT_BINDING,
            "native custody or actor owner mismatch",
        ));
    }

    Ok(EscrowMetadata {
        version: ESCROW_METADATA_VERSION,
        swap_id,
        terms_hash,
        claim_authority,
        depositor: depositor.account_id,
        depositor_asset: depositor.account_id,
        claimant: claimant.account_id,
        claimant_asset: claimant.account_id,
        custody: custody.account_id,
        asset_program: authenticated_transfer_program,
        custody_program: authenticated_transfer_program,
        asset_definition: [0; 32],
        amount,
        refund_at,
        status: EscrowStatus::Empty,
    })
}

#[allow(clippy::too_many_arguments)]
fn native_xmr_initial_state(
    ctx: &ProgramContext,
    custody: &AccountWithMetadata,
    depositor: &AccountWithMetadata,
    claimant: &AccountWithMetadata,
    claim_aggregate_authority: &AccountWithMetadata,
    refund_aggregate_authority: &AccountWithMetadata,
    swap_id: [u8; 32],
    terms_hash: [u8; 32],
    claim_aggregate_x_only_public_key: [u8; 32],
    refund_aggregate_x_only_public_key: [u8; 32],
    maker_dleq_transcript_commitment: [u8; 32],
    taker_dleq_transcript_commitment: [u8; 32],
    claim_partial_commitment: [u8; 32],
    amount: u128,
    refund_at: u64,
    punish_at: u64,
    authenticated_transfer_program: ProgramId,
) -> Result<EscrowMetadata, SpelError> {
    if punish_at <= refund_at
        || depositor.account_id == claimant.account_id
        || depositor.account_id == claim_aggregate_authority.account_id
        || depositor.account_id == refund_aggregate_authority.account_id
    {
        return Err(custom_error(
            ERROR_INVALID_TERMS,
            "invalid XMR native terms",
        ));
    }
    let authority = ClaimAuthority::XmrDualAdaptor {
        claim_aggregate_x_only_public_key,
        claim_aggregate_account_id: claim_aggregate_authority.account_id,
        refund_aggregate_x_only_public_key,
        refund_aggregate_account_id: refund_aggregate_authority.account_id,
        maker_dleq_transcript_commitment,
        taker_dleq_transcript_commitment,
        claim_partial_commitment,
        punish_at,
    };
    let mut state = native_initial_state(
        ctx,
        custody,
        depositor,
        claimant,
        swap_id,
        terms_hash,
        authority,
        amount,
        refund_at,
        authenticated_transfer_program,
    )?;
    state.version = XMR_ESCROW_METADATA_VERSION;
    Ok(state)
}

enum ClaimProof {
    Sha256Preimage([u8; 32]),
    AggregateWitness(AccountId),
}

fn validated_native_claim_state(
    metadata: &AccountWithMetadata,
    custody: &AccountWithMetadata,
    claimant: &AccountWithMetadata,
    swap_id: [u8; 32],
    proof: ClaimProof,
) -> Result<EscrowMetadata, SpelError> {
    let mut state = read_metadata(metadata)?;
    require_funded(&state)?;
    if state.swap_id != swap_id
        || state.asset_definition != [0; 32]
        || state.asset_program != state.custody_program
        || state.claimant != claimant.account_id
        || state.custody != custody.account_id
        || claimant.account.program_owner != state.asset_program
        || custody.account.program_owner != state.asset_program
        || custody.account.balance != state.amount
    {
        return Err(custom_error(
            ERROR_ACCOUNT_BINDING,
            "native claim account binding mismatch",
        ));
    }
    match proof {
        ClaimProof::Sha256Preimage(preimage) => {
            require_preimage_authority(&state, preimage)?;
        }
        ClaimProof::AggregateWitness(aggregate_authority) => {
            require_aggregate_witness_authority(&state, aggregate_authority)?;
        }
    }
    state.status = EscrowStatus::Claimed;
    Ok(state)
}

fn validated_xmr_native_witness_state(
    metadata: &AccountWithMetadata,
    custody: &AccountWithMetadata,
    recipient: &AccountWithMetadata,
    aggregate_authority: &AccountWithMetadata,
    swap_id: [u8; 32],
    purpose: XmrWitnessPurpose,
) -> Result<(EscrowMetadata, u64), SpelError> {
    let mut state = read_metadata(metadata)?;
    let status_allowed = match purpose {
        XmrWitnessPurpose::Claim => state.status == EscrowStatus::XmrClaimAuthorized,
        XmrWitnessPurpose::Refund => matches!(
            state.status,
            EscrowStatus::Funded | EscrowStatus::XmrClaimAuthorized
        ),
    };
    if !status_allowed {
        return Err(custom_error(
            ERROR_NOT_FUNDED,
            "XMR escrow is not in the required branch state",
        ));
    }
    let expected_recipient = match purpose {
        XmrWitnessPurpose::Claim => state.claimant,
        XmrWitnessPurpose::Refund => state.depositor,
    };
    if state.swap_id != swap_id
        || state.version != XMR_ESCROW_METADATA_VERSION
        || state.asset_definition != [0; 32]
        || state.asset_program != state.custody_program
        || expected_recipient != recipient.account_id
        || state.custody != custody.account_id
        || recipient.account.program_owner != state.asset_program
        || custody.account.program_owner != state.asset_program
        || custody.account.balance != state.amount
    {
        return Err(custom_error(
            ERROR_ACCOUNT_BINDING,
            "XMR native witness account binding mismatch",
        ));
    }
    let punish_at = require_xmr_witness_authority(&state, purpose, aggregate_authority.account_id)?;
    state.status = match purpose {
        XmrWitnessPurpose::Claim => EscrowStatus::Claimed,
        XmrWitnessPurpose::Refund => EscrowStatus::Refunded,
    };
    Ok((state, punish_at))
}

fn validated_xmr_claim_authorization_state(
    metadata: &AccountWithMetadata,
    depositor: &AccountWithMetadata,
    swap_id: [u8; 32],
    claim_partial: [u8; 32],
) -> Result<EscrowMetadata, SpelError> {
    let mut state = read_metadata(metadata)?;
    require_funded(&state)?;
    let ClaimAuthority::XmrDualAdaptor {
        claim_partial_commitment,
        ..
    } = state.claim_authority
    else {
        return Err(custom_error(
            ERROR_WRONG_CLAIM_AUTHORITY,
            "escrow is not an XMR dual-adaptor escrow",
        ));
    };
    if state.swap_id != swap_id
        || state.version != XMR_ESCROW_METADATA_VERSION
        || state.depositor != depositor.account_id
        || depositor.account.program_owner != state.asset_program
        || claim_partial == [0; 32]
        || xmr_claim_partial_commitment(claim_partial) != claim_partial_commitment
    {
        return Err(custom_error(
            ERROR_ACCOUNT_BINDING,
            "XMR on-chain claim-partial authorization mismatch",
        ));
    }
    state.status = EscrowStatus::XmrClaimAuthorized;
    Ok(state)
}

fn validated_xmr_native_punish_state(
    metadata: &AccountWithMetadata,
    custody: &AccountWithMetadata,
    claimant: &AccountWithMetadata,
    swap_id: [u8; 32],
) -> Result<(EscrowMetadata, u64), SpelError> {
    let mut state = read_metadata(metadata)?;
    if !matches!(
        state.status,
        EscrowStatus::Funded | EscrowStatus::XmrClaimAuthorized
    ) {
        return Err(custom_error(ERROR_NOT_FUNDED, "escrow is not funded"));
    }
    if state.swap_id != swap_id
        || state.version != XMR_ESCROW_METADATA_VERSION
        || state.asset_definition != [0; 32]
        || state.asset_program != state.custody_program
        || state.claimant != claimant.account_id
        || state.custody != custody.account_id
        || claimant.account.program_owner != state.asset_program
        || custody.account.program_owner != state.asset_program
        || custody.account.balance != state.amount
    {
        return Err(custom_error(
            ERROR_ACCOUNT_BINDING,
            "XMR native punishment account binding mismatch",
        ));
    }
    let ClaimAuthority::XmrDualAdaptor { punish_at, .. } = state.claim_authority else {
        return Err(custom_error(
            ERROR_WRONG_CLAIM_AUTHORITY,
            "escrow is not an XMR dual-adaptor escrow",
        ));
    };
    state.status = EscrowStatus::Claimed;
    Ok((state, punish_at))
}

#[allow(clippy::too_many_arguments)]
fn token_initial_state(
    ctx: &ProgramContext,
    metadata: &AccountWithMetadata,
    depositor_owner: &AccountWithMetadata,
    claimant_owner: &AccountWithMetadata,
    token_definition: &AccountWithMetadata,
    swap_id: [u8; 32],
    terms_hash: [u8; 32],
    claim_authority: ClaimAuthority,
    amount: u128,
    refund_at: u64,
    ata_program: ProgramId,
) -> Result<EscrowMetadata, SpelError> {
    let token_program = token_definition.account.program_owner;
    if amount == 0
        || refund_at == 0
        || terms_hash == [0; 32]
        || !valid_claim_authority(claim_authority, claimant_owner.account_id)
        || token_program == DEFAULT_PROGRAM_ID
        || token_program == ctx.self_program_id
        || ata_program == DEFAULT_PROGRAM_ID
        || ata_program == ctx.self_program_id
        || ata_program == token_program
    {
        return Err(custom_error(ERROR_INVALID_TERMS, "invalid token terms"));
    }
    require_fungible_definition(token_definition, token_program)?;

    let definition = token_definition.account_id;
    Ok(EscrowMetadata {
        version: ESCROW_METADATA_VERSION,
        swap_id,
        terms_hash,
        claim_authority,
        depositor: depositor_owner.account_id,
        depositor_asset: associated_token_account(
            ata_program,
            depositor_owner.account_id,
            definition,
        ),
        claimant: claimant_owner.account_id,
        claimant_asset: associated_token_account(
            ata_program,
            claimant_owner.account_id,
            definition,
        ),
        custody: associated_token_account(ata_program, metadata.account_id, definition),
        asset_program: token_program,
        custody_program: ata_program,
        asset_definition: definition.into_value(),
        amount,
        refund_at,
        status: EscrowStatus::Empty,
    })
}

fn validated_token_claim_state(
    metadata: &AccountWithMetadata,
    custody: &AccountWithMetadata,
    claimant_owner: &AccountWithMetadata,
    claimant_asset: &AccountWithMetadata,
    swap_id: [u8; 32],
    proof: ClaimProof,
) -> Result<EscrowMetadata, SpelError> {
    let mut state = read_metadata(metadata)?;
    require_funded(&state)?;
    if state.swap_id != swap_id
        || state.claimant != claimant_owner.account_id
        || state.claimant_asset != claimant_asset.account_id
        || state.custody != custody.account_id
        || claimant_asset.account.program_owner != state.asset_program
        || custody.account.program_owner != state.asset_program
        || token_definition(claimant_asset)?.into_value() != state.asset_definition
        || token_definition(custody)?.into_value() != state.asset_definition
        || fungible_balance(custody)? != state.amount
    {
        return Err(custom_error(
            ERROR_ACCOUNT_BINDING,
            "token claim account binding mismatch",
        ));
    }
    match proof {
        ClaimProof::Sha256Preimage(preimage) => require_preimage_authority(&state, preimage)?,
        ClaimProof::AggregateWitness(aggregate_authority) => {
            require_aggregate_witness_authority(&state, aggregate_authority)?;
        }
    }
    state.status = EscrowStatus::Claimed;
    Ok(state)
}

fn native_transfer_call(
    authenticated_transfer_program: ProgramId,
    mut sender: AccountWithMetadata,
    recipient: AccountWithMetadata,
    amount: u128,
    authorize_custody: bool,
    swap_id: &[u8; 32],
) -> ChainedCall {
    if authorize_custody {
        sender.is_authorized = true;
    }
    let call = ChainedCall::new(
        authenticated_transfer_program,
        vec![sender, recipient],
        &AuthenticatedTransferInstruction::Transfer { amount },
    );
    if authorize_custody {
        call.with_pda_seeds(vec![custody_pda_seed(swap_id)])
    } else {
        call
    }
}

fn ata_transfer_call(
    ata_program: ProgramId,
    mut owner: AccountWithMetadata,
    sender: AccountWithMetadata,
    recipient: AccountWithMetadata,
    amount: u128,
    authorize_metadata: bool,
    swap_id: &[u8; 32],
) -> ChainedCall {
    if authorize_metadata {
        owner.is_authorized = true;
    }
    let call = ChainedCall::new(
        ata_program,
        vec![owner, sender, recipient],
        &ata_core::Instruction::Transfer {
            ata_program_id: ata_program,
            amount,
        },
    );
    if authorize_metadata {
        call.with_pda_seeds(vec![metadata_pda_seed(swap_id)])
    } else {
        call
    }
}

fn require_supported(state: &EscrowMetadata) -> Result<(), SpelError> {
    let supported = match state.claim_authority {
        ClaimAuthority::XmrDualAdaptor { .. } => state.version == XMR_ESCROW_METADATA_VERSION,
        _ => state.version == ESCROW_METADATA_VERSION,
    };
    if !supported {
        return Err(custom_error(
            ERROR_UNSUPPORTED_VERSION,
            "unsupported escrow metadata version",
        ));
    }
    Ok(())
}

fn require_funded(state: &EscrowMetadata) -> Result<(), SpelError> {
    require_supported(state)?;
    if state.status != EscrowStatus::Funded {
        return Err(custom_error(ERROR_NOT_FUNDED, "escrow is not funded"));
    }
    Ok(())
}

#[lez_program]
mod zec_escrow {
    #[allow(unused_imports)]
    use super::*;

    #[instruction]
    #[allow(clippy::too_many_arguments)]
    pub fn initialize_native(
        ctx: ProgramContext,
        #[account(init, pda = arg("swap_id"))] metadata: AccountWithMetadata,
        #[account(mut, pda = [literal("custody"), arg("swap_id")])] custody: AccountWithMetadata,
        #[account(signer)] depositor: AccountWithMetadata,
        claimant: AccountWithMetadata,
        swap_id: [u8; 32],
        terms_hash: [u8; 32],
        secret_digest: [u8; 32],
        amount: u128,
        refund_at: u64,
        authenticated_transfer_program: [u32; 8],
    ) -> SpelResult {
        let state = native_initial_state(
            &ctx,
            &custody,
            &depositor,
            &claimant,
            swap_id,
            terms_hash,
            ClaimAuthority::Sha256Preimage { secret_digest },
            amount,
            refund_at,
            authenticated_transfer_program,
        )?;
        let mut metadata = metadata;
        write_metadata(&mut metadata, &state)?;
        let initialize =
            native_initialize_call(authenticated_transfer_program, custody.clone(), &swap_id);
        Ok(SpelOutput::execute(
            vec![metadata, custody, depositor, claimant],
            vec![initialize],
        )
        .with_timestamp_validity_window(..refund_at))
    }

    #[instruction]
    #[allow(clippy::too_many_arguments)]
    pub fn initialize_native_witnessed(
        ctx: ProgramContext,
        #[account(init, pda = arg("swap_id"))] metadata: AccountWithMetadata,
        #[account(mut, pda = [literal("custody"), arg("swap_id")])] custody: AccountWithMetadata,
        #[account(signer)] depositor: AccountWithMetadata,
        claimant: AccountWithMetadata,
        aggregate_authority: AccountWithMetadata,
        swap_id: [u8; 32],
        terms_hash: [u8; 32],
        aggregate_x_only_public_key: [u8; 32],
        amount: u128,
        refund_at: u64,
        authenticated_transfer_program: [u32; 8],
    ) -> SpelResult {
        let state = native_initial_state(
            &ctx,
            &custody,
            &depositor,
            &claimant,
            swap_id,
            terms_hash,
            ClaimAuthority::AggregateWitness {
                x_only_public_key: aggregate_x_only_public_key,
                account_id: aggregate_authority.account_id,
            },
            amount,
            refund_at,
            authenticated_transfer_program,
        )?;
        let mut metadata = metadata;
        write_metadata(&mut metadata, &state)?;
        let initialize =
            native_initialize_call(authenticated_transfer_program, custody.clone(), &swap_id);
        Ok(SpelOutput::execute(
            vec![metadata, custody, depositor, claimant, aggregate_authority],
            vec![initialize],
        )
        .with_timestamp_validity_window(..refund_at))
    }

    #[instruction]
    pub fn fund_native(
        _ctx: ProgramContext,
        #[account(mut, owner = self_program_id, pda = arg("swap_id"))]
        metadata: AccountWithMetadata,
        #[account(mut, pda = [literal("custody"), arg("swap_id")])] custody: AccountWithMetadata,
        #[account(mut, signer)] depositor: AccountWithMetadata,
        swap_id: [u8; 32],
    ) -> SpelResult {
        let mut metadata = metadata;
        let mut state = read_metadata(&metadata)?;
        require_supported(&state)?;
        if state.status != EscrowStatus::Empty {
            return Err(custom_error(ERROR_NOT_FUNDED, "escrow is not empty"));
        }
        if state.swap_id != swap_id
            || state.asset_definition != [0; 32]
            || state.asset_program != state.custody_program
            || state.depositor != depositor.account_id
            || state.custody != custody.account_id
            || depositor.account.program_owner != state.asset_program
            || custody.account.program_owner != state.asset_program
            || custody.account.balance != 0
            || depositor.account.balance < state.amount
        {
            return Err(custom_error(
                ERROR_ACCOUNT_BINDING,
                "native funding account binding mismatch",
            ));
        }

        let transfer = native_transfer_call(
            state.asset_program,
            depositor.clone(),
            custody.clone(),
            state.amount,
            false,
            &swap_id,
        );
        state.status = EscrowStatus::Funded;
        write_metadata(&mut metadata, &state)?;
        Ok(
            SpelOutput::execute(vec![metadata, custody, depositor], vec![transfer])
                .with_timestamp_validity_window(..state.refund_at),
        )
    }

    #[instruction]
    pub fn claim_native(
        _ctx: ProgramContext,
        #[account(mut, owner = self_program_id, pda = arg("swap_id"))]
        metadata: AccountWithMetadata,
        #[account(mut, pda = [literal("custody"), arg("swap_id")])] custody: AccountWithMetadata,
        #[account(mut, signer)] claimant: AccountWithMetadata,
        swap_id: [u8; 32],
        preimage: [u8; 32],
    ) -> SpelResult {
        let state = validated_native_claim_state(
            &metadata,
            &custody,
            &claimant,
            swap_id,
            ClaimProof::Sha256Preimage(preimage),
        )?;
        let transfer = native_transfer_call(
            state.asset_program,
            custody.clone(),
            claimant.clone(),
            state.amount,
            true,
            &swap_id,
        );
        let mut metadata = metadata;
        write_metadata(&mut metadata, &state)?;
        Ok(
            SpelOutput::execute(vec![metadata, custody, claimant], vec![transfer])
                .with_timestamp_validity_window(..state.refund_at),
        )
    }

    #[instruction]
    pub fn claim_native_witnessed(
        _ctx: ProgramContext,
        #[account(mut, owner = self_program_id, pda = arg("swap_id"))]
        metadata: AccountWithMetadata,
        #[account(mut, pda = [literal("custody"), arg("swap_id")])] custody: AccountWithMetadata,
        #[account(mut)] claimant: AccountWithMetadata,
        #[account(signer)] aggregate_authority: AccountWithMetadata,
        swap_id: [u8; 32],
    ) -> SpelResult {
        let state = validated_native_claim_state(
            &metadata,
            &custody,
            &claimant,
            swap_id,
            ClaimProof::AggregateWitness(aggregate_authority.account_id),
        )?;
        let transfer = native_transfer_call(
            state.asset_program,
            custody.clone(),
            claimant.clone(),
            state.amount,
            true,
            &swap_id,
        );
        let mut metadata = metadata;
        write_metadata(&mut metadata, &state)?;
        Ok(SpelOutput::execute(
            vec![metadata, custody, claimant, aggregate_authority],
            vec![transfer],
        )
        .with_timestamp_validity_window(..state.refund_at))
    }

    #[instruction]
    pub fn refund_native(
        _ctx: ProgramContext,
        #[account(mut, owner = self_program_id, pda = arg("swap_id"))]
        metadata: AccountWithMetadata,
        #[account(mut, pda = [literal("custody"), arg("swap_id")])] custody: AccountWithMetadata,
        #[account(mut)] depositor: AccountWithMetadata,
        swap_id: [u8; 32],
    ) -> SpelResult {
        let mut metadata = metadata;
        let mut state = read_metadata(&metadata)?;
        require_funded(&state)?;
        if matches!(state.claim_authority, ClaimAuthority::XmrDualAdaptor { .. }) {
            return Err(custom_error(
                ERROR_WRONG_CLAIM_AUTHORITY,
                "XMR escrow requires the signed refund instruction",
            ));
        }
        if state.swap_id != swap_id
            || state.asset_definition != [0; 32]
            || state.asset_program != state.custody_program
            || state.depositor != depositor.account_id
            || state.custody != custody.account_id
            || depositor.account.program_owner != state.asset_program
            || custody.account.program_owner != state.asset_program
            || custody.account.balance != state.amount
        {
            return Err(custom_error(
                ERROR_ACCOUNT_BINDING,
                "native refund account binding mismatch",
            ));
        }

        let transfer = native_transfer_call(
            state.asset_program,
            custody.clone(),
            depositor.clone(),
            state.amount,
            true,
            &swap_id,
        );
        state.status = EscrowStatus::Refunded;
        write_metadata(&mut metadata, &state)?;
        Ok(
            SpelOutput::execute(vec![metadata, custody, depositor], vec![transfer])
                .with_timestamp_validity_window(state.refund_at..),
        )
    }

    #[instruction]
    #[allow(clippy::too_many_arguments)]
    pub fn initialize_token(
        ctx: ProgramContext,
        #[account(init, pda = arg("swap_id"))] metadata: AccountWithMetadata,
        #[account(signer)] depositor_owner: AccountWithMetadata,
        claimant_owner: AccountWithMetadata,
        token_definition: AccountWithMetadata,
        swap_id: [u8; 32],
        terms_hash: [u8; 32],
        secret_digest: [u8; 32],
        amount: u128,
        refund_at: u64,
        ata_program: [u32; 8],
    ) -> SpelResult {
        let mut metadata = metadata;
        let state = token_initial_state(
            &ctx,
            &metadata,
            &depositor_owner,
            &claimant_owner,
            &token_definition,
            swap_id,
            terms_hash,
            ClaimAuthority::Sha256Preimage { secret_digest },
            amount,
            refund_at,
            ata_program,
        )?;
        write_metadata(&mut metadata, &state)?;
        Ok(SpelOutput::execute(
            vec![metadata, depositor_owner, claimant_owner, token_definition],
            vec![],
        )
        .with_timestamp_validity_window(..refund_at))
    }

    #[instruction]
    pub fn create_token_custody(
        _ctx: ProgramContext,
        #[account(owner = self_program_id, pda = arg("swap_id"))] metadata: AccountWithMetadata,
        token_definition: AccountWithMetadata,
        #[account(mut)] custody: AccountWithMetadata,
        swap_id: [u8; 32],
    ) -> SpelResult {
        let state = read_metadata(&metadata)?;
        require_supported(&state)?;
        if state.status != EscrowStatus::Empty {
            return Err(custom_error(ERROR_NOT_FUNDED, "escrow is not empty"));
        }
        if state.swap_id != swap_id
            || state.asset_definition != token_definition.account_id.into_value()
            || state.custody != custody.account_id
            || state.custody
                != associated_token_account(
                    state.custody_program,
                    metadata.account_id,
                    token_definition.account_id,
                )
            || custody.account != Account::default()
        {
            return Err(custom_error(
                ERROR_ACCOUNT_BINDING,
                "token custody derivation mismatch",
            ));
        }
        require_fungible_definition(&token_definition, state.asset_program)?;
        let create = ChainedCall::new(
            state.custody_program,
            vec![metadata.clone(), token_definition.clone(), custody.clone()],
            &ata_core::Instruction::Create {
                ata_program_id: state.custody_program,
            },
        );
        Ok(
            SpelOutput::execute(vec![metadata, token_definition, custody], vec![create])
                .with_timestamp_validity_window(..state.refund_at),
        )
    }

    #[instruction]
    pub fn fund_token(
        _ctx: ProgramContext,
        #[account(mut, owner = self_program_id, pda = arg("swap_id"))]
        metadata: AccountWithMetadata,
        #[account(signer)] depositor_owner: AccountWithMetadata,
        #[account(mut)] depositor_asset: AccountWithMetadata,
        #[account(mut)] custody: AccountWithMetadata,
        swap_id: [u8; 32],
    ) -> SpelResult {
        let mut metadata = metadata;
        let mut state = read_metadata(&metadata)?;
        require_supported(&state)?;
        if state.status != EscrowStatus::Empty {
            return Err(custom_error(ERROR_NOT_FUNDED, "escrow is not empty"));
        }
        if state.swap_id != swap_id
            || state.depositor != depositor_owner.account_id
            || state.depositor_asset != depositor_asset.account_id
            || state.custody != custody.account_id
            || depositor_asset.account.program_owner != state.asset_program
            || custody.account.program_owner != state.asset_program
            || token_definition(&depositor_asset)?.into_value() != state.asset_definition
            || token_definition(&custody)?.into_value() != state.asset_definition
            || fungible_balance(&depositor_asset)? < state.amount
            || fungible_balance(&custody)? != 0
        {
            return Err(custom_error(
                ERROR_ACCOUNT_BINDING,
                "token funding account binding mismatch",
            ));
        }
        let transfer = ata_transfer_call(
            state.custody_program,
            depositor_owner.clone(),
            depositor_asset.clone(),
            custody.clone(),
            state.amount,
            false,
            &swap_id,
        );
        state.status = EscrowStatus::Funded;
        write_metadata(&mut metadata, &state)?;
        Ok(SpelOutput::execute(
            vec![metadata, depositor_owner, depositor_asset, custody],
            vec![transfer],
        )
        .with_timestamp_validity_window(..state.refund_at))
    }

    #[instruction]
    pub fn claim_token(
        _ctx: ProgramContext,
        #[account(mut, owner = self_program_id, pda = arg("swap_id"))]
        metadata: AccountWithMetadata,
        #[account(mut)] custody: AccountWithMetadata,
        #[account(signer)] claimant_owner: AccountWithMetadata,
        #[account(mut)] claimant_asset: AccountWithMetadata,
        swap_id: [u8; 32],
        preimage: [u8; 32],
    ) -> SpelResult {
        let state = validated_token_claim_state(
            &metadata,
            &custody,
            &claimant_owner,
            &claimant_asset,
            swap_id,
            ClaimProof::Sha256Preimage(preimage),
        )?;
        let mut metadata = metadata;
        write_metadata(&mut metadata, &state)?;
        let transfer = ata_transfer_call(
            state.custody_program,
            metadata.clone(),
            custody.clone(),
            claimant_asset.clone(),
            state.amount,
            true,
            &swap_id,
        );
        Ok(SpelOutput::execute(
            vec![metadata, custody, claimant_owner, claimant_asset],
            vec![transfer],
        )
        .with_timestamp_validity_window(..state.refund_at))
    }

    #[instruction]
    pub fn refund_token(
        _ctx: ProgramContext,
        #[account(mut, owner = self_program_id, pda = arg("swap_id"))]
        metadata: AccountWithMetadata,
        #[account(mut)] custody: AccountWithMetadata,
        #[account(mut)] depositor_asset: AccountWithMetadata,
        swap_id: [u8; 32],
    ) -> SpelResult {
        let mut metadata = metadata;
        let mut state = read_metadata(&metadata)?;
        require_funded(&state)?;
        if state.swap_id != swap_id
            || state.depositor_asset != depositor_asset.account_id
            || state.custody != custody.account_id
            || depositor_asset.account.program_owner != state.asset_program
            || custody.account.program_owner != state.asset_program
            || token_definition(&depositor_asset)?.into_value() != state.asset_definition
            || token_definition(&custody)?.into_value() != state.asset_definition
            || fungible_balance(&custody)? != state.amount
        {
            return Err(custom_error(
                ERROR_ACCOUNT_BINDING,
                "token refund account binding mismatch",
            ));
        }

        state.status = EscrowStatus::Refunded;
        write_metadata(&mut metadata, &state)?;
        let transfer = ata_transfer_call(
            state.custody_program,
            metadata.clone(),
            custody.clone(),
            depositor_asset.clone(),
            state.amount,
            true,
            &swap_id,
        );
        Ok(
            SpelOutput::execute(vec![metadata, custody, depositor_asset], vec![transfer])
                .with_timestamp_validity_window(state.refund_at..),
        )
    }

    // These witnessed-token instructions are intentionally appended after every
    // pre-existing instruction. The program macro derives the public instruction
    // discriminants from declaration order, so appending preserves the v0.2
    // wire encoding of the native and SHA-256 token surfaces.
    #[instruction]
    #[allow(clippy::too_many_arguments)]
    pub fn initialize_token_witnessed(
        ctx: ProgramContext,
        #[account(init, pda = arg("swap_id"))] metadata: AccountWithMetadata,
        #[account(signer)] depositor_owner: AccountWithMetadata,
        claimant_owner: AccountWithMetadata,
        token_definition: AccountWithMetadata,
        aggregate_authority: AccountWithMetadata,
        swap_id: [u8; 32],
        terms_hash: [u8; 32],
        aggregate_x_only_public_key: [u8; 32],
        amount: u128,
        refund_at: u64,
        ata_program: [u32; 8],
    ) -> SpelResult {
        let mut metadata = metadata;
        let state = token_initial_state(
            &ctx,
            &metadata,
            &depositor_owner,
            &claimant_owner,
            &token_definition,
            swap_id,
            terms_hash,
            ClaimAuthority::AggregateWitness {
                x_only_public_key: aggregate_x_only_public_key,
                account_id: aggregate_authority.account_id,
            },
            amount,
            refund_at,
            ata_program,
        )?;
        write_metadata(&mut metadata, &state)?;
        Ok(SpelOutput::execute(
            vec![
                metadata,
                depositor_owner,
                claimant_owner,
                token_definition,
                aggregate_authority,
            ],
            vec![],
        )
        .with_timestamp_validity_window(..refund_at))
    }

    #[instruction]
    pub fn claim_token_witnessed(
        _ctx: ProgramContext,
        #[account(mut, owner = self_program_id, pda = arg("swap_id"))]
        metadata: AccountWithMetadata,
        #[account(mut)] custody: AccountWithMetadata,
        claimant_owner: AccountWithMetadata,
        #[account(mut)] claimant_asset: AccountWithMetadata,
        #[account(signer)] aggregate_authority: AccountWithMetadata,
        swap_id: [u8; 32],
    ) -> SpelResult {
        let state = validated_token_claim_state(
            &metadata,
            &custody,
            &claimant_owner,
            &claimant_asset,
            swap_id,
            ClaimProof::AggregateWitness(aggregate_authority.account_id),
        )?;
        let mut metadata = metadata;
        write_metadata(&mut metadata, &state)?;
        let transfer = ata_transfer_call(
            state.custody_program,
            metadata.clone(),
            custody.clone(),
            claimant_asset.clone(),
            state.amount,
            true,
            &swap_id,
        );
        Ok(SpelOutput::execute(
            vec![
                metadata,
                custody,
                claimant_owner,
                claimant_asset,
                aggregate_authority,
            ],
            vec![transfer],
        )
        .with_timestamp_validity_window(..state.refund_at))
    }

    // XMR-native instructions are intentionally appended after tags 0 through
    // 12 so every existing v0.2 instruction discriminant remains byte-identical.
    #[instruction]
    #[allow(clippy::too_many_arguments)]
    pub fn initialize_native_xmr(
        ctx: ProgramContext,
        #[account(init, pda = arg("swap_id"))] metadata: AccountWithMetadata,
        #[account(mut, pda = [literal("custody"), arg("swap_id")])] custody: AccountWithMetadata,
        #[account(signer)] depositor: AccountWithMetadata,
        claimant: AccountWithMetadata,
        claim_aggregate_authority: AccountWithMetadata,
        refund_aggregate_authority: AccountWithMetadata,
        swap_id: [u8; 32],
        terms_hash: [u8; 32],
        claim_aggregate_x_only_public_key: [u8; 32],
        refund_aggregate_x_only_public_key: [u8; 32],
        maker_dleq_transcript_commitment: [u8; 32],
        taker_dleq_transcript_commitment: [u8; 32],
        claim_partial_commitment: [u8; 32],
        amount: u128,
        refund_at: u64,
        punish_at: u64,
        authenticated_transfer_program: [u32; 8],
    ) -> SpelResult {
        let state = native_xmr_initial_state(
            &ctx,
            &custody,
            &depositor,
            &claimant,
            &claim_aggregate_authority,
            &refund_aggregate_authority,
            swap_id,
            terms_hash,
            claim_aggregate_x_only_public_key,
            refund_aggregate_x_only_public_key,
            maker_dleq_transcript_commitment,
            taker_dleq_transcript_commitment,
            claim_partial_commitment,
            amount,
            refund_at,
            punish_at,
            authenticated_transfer_program,
        )?;
        let mut metadata = metadata;
        write_metadata(&mut metadata, &state)?;
        let initialize =
            native_initialize_call(authenticated_transfer_program, custody.clone(), &swap_id);
        Ok(SpelOutput::execute(
            vec![
                metadata,
                custody,
                depositor,
                claimant,
                claim_aggregate_authority,
                refund_aggregate_authority,
            ],
            vec![initialize],
        )
        .with_timestamp_validity_window(..refund_at))
    }

    #[instruction]
    pub fn authorize_native_xmr_claim(
        _ctx: ProgramContext,
        #[account(mut, owner = self_program_id, pda = arg("swap_id"))]
        metadata: AccountWithMetadata,
        #[account(signer)] depositor: AccountWithMetadata,
        swap_id: [u8; 32],
        claim_partial: [u8; 32],
    ) -> SpelResult {
        let state =
            validated_xmr_claim_authorization_state(&metadata, &depositor, swap_id, claim_partial)?;
        let mut metadata = metadata;
        write_metadata(&mut metadata, &state)?;
        Ok(SpelOutput::execute(vec![metadata, depositor], vec![])
            .with_timestamp_validity_window(..state.refund_at))
    }

    #[instruction]
    pub fn claim_native_xmr(
        _ctx: ProgramContext,
        #[account(mut, owner = self_program_id, pda = arg("swap_id"))]
        metadata: AccountWithMetadata,
        #[account(mut, pda = [literal("custody"), arg("swap_id")])] custody: AccountWithMetadata,
        #[account(mut)] claimant: AccountWithMetadata,
        #[account(signer)] claim_aggregate_authority: AccountWithMetadata,
        swap_id: [u8; 32],
    ) -> SpelResult {
        let (state, _) = validated_xmr_native_witness_state(
            &metadata,
            &custody,
            &claimant,
            &claim_aggregate_authority,
            swap_id,
            XmrWitnessPurpose::Claim,
        )?;
        let transfer = native_transfer_call(
            state.asset_program,
            custody.clone(),
            claimant.clone(),
            state.amount,
            true,
            &swap_id,
        );
        let mut metadata = metadata;
        write_metadata(&mut metadata, &state)?;
        Ok(SpelOutput::execute(
            vec![metadata, custody, claimant, claim_aggregate_authority],
            vec![transfer],
        )
        .with_timestamp_validity_window(..state.refund_at))
    }

    #[instruction]
    pub fn refund_native_xmr(
        _ctx: ProgramContext,
        #[account(mut, owner = self_program_id, pda = arg("swap_id"))]
        metadata: AccountWithMetadata,
        #[account(mut, pda = [literal("custody"), arg("swap_id")])] custody: AccountWithMetadata,
        #[account(mut)] depositor: AccountWithMetadata,
        #[account(signer)] refund_aggregate_authority: AccountWithMetadata,
        swap_id: [u8; 32],
    ) -> SpelResult {
        let (state, punish_at) = validated_xmr_native_witness_state(
            &metadata,
            &custody,
            &depositor,
            &refund_aggregate_authority,
            swap_id,
            XmrWitnessPurpose::Refund,
        )?;
        let transfer = native_transfer_call(
            state.asset_program,
            custody.clone(),
            depositor.clone(),
            state.amount,
            true,
            &swap_id,
        );
        let mut metadata = metadata;
        write_metadata(&mut metadata, &state)?;
        SpelOutput::execute(
            vec![metadata, custody, depositor, refund_aggregate_authority],
            vec![transfer],
        )
        .try_with_timestamp_validity_window(state.refund_at..punish_at)
        .map_err(|_| {
            custom_error(
                ERROR_INVALID_TERMS,
                "invalid XMR signed refund validity window",
            )
        })
    }

    #[instruction]
    pub fn punish_native_xmr(
        _ctx: ProgramContext,
        #[account(mut, owner = self_program_id, pda = arg("swap_id"))]
        metadata: AccountWithMetadata,
        #[account(mut, pda = [literal("custody"), arg("swap_id")])] custody: AccountWithMetadata,
        #[account(mut, signer)] claimant: AccountWithMetadata,
        swap_id: [u8; 32],
    ) -> SpelResult {
        let (state, punish_at) =
            validated_xmr_native_punish_state(&metadata, &custody, &claimant, swap_id)?;
        let transfer = native_transfer_call(
            state.asset_program,
            custody.clone(),
            claimant.clone(),
            state.amount,
            true,
            &swap_id,
        );
        let mut metadata = metadata;
        write_metadata(&mut metadata, &state)?;
        Ok(
            SpelOutput::execute(vec![metadata, custody, claimant], vec![transfer])
                .with_timestamp_validity_window(punish_at..),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ESCROW_PROGRAM: ProgramId = [7; 8];
    const AUTHENTICATED_TRANSFER: ProgramId = [9; 8];
    const ATA_PROGRAM: ProgramId = [10; 8];
    const TOKEN_PROGRAM: ProgramId = [13; 8];
    const SWAP_ID: [u8; 32] = [11; 32];
    const PREIMAGE: [u8; 32] = [12; 32];
    const AMOUNT: u128 = 75;
    const REFUND_AT: u64 = 1_000;
    const PUNISH_AT: u64 = 2_000;
    const XMR_CLAIM_KEY: [u8; 32] = [44; 32];
    const XMR_REFUND_KEY: [u8; 32] = [45; 32];
    const MAKER_DLEQ_COMMITMENT: [u8; 32] = [46; 32];
    const TAKER_DLEQ_COMMITMENT: [u8; 32] = [47; 32];
    const XMR_CLAIM_PARTIAL: [u8; 32] = [48; 32];

    fn account(
        id: [u8; 32],
        owner: ProgramId,
        balance: u128,
        authorized: bool,
    ) -> AccountWithMetadata {
        account_with_data(id, owner, balance, Data::default(), authorized)
    }

    fn account_with_data(
        id: [u8; 32],
        owner: ProgramId,
        balance: u128,
        data: Data,
        authorized: bool,
    ) -> AccountWithMetadata {
        AccountWithMetadata::new(
            Account {
                program_owner: owner,
                balance,
                data,
                ..Account::default()
            },
            authorized,
            AccountId::new(id),
        )
    }

    fn empty_account(id: AccountId) -> AccountWithMetadata {
        account(id.into_value(), DEFAULT_PROGRAM_ID, 0, false)
    }

    fn actor(id: [u8; 32], signer: bool) -> AccountWithMetadata {
        account(id, DEFAULT_PROGRAM_ID, 0, signer)
    }

    fn definition_account(definition: AccountId) -> AccountWithMetadata {
        account_with_data(
            definition.into_value(),
            TOKEN_PROGRAM,
            0,
            Data::from(&TokenDefinition::Fungible {
                name: "M2-v0.2-token".into(),
                total_supply: 1_000,
                metadata_id: None,
            }),
            false,
        )
    }

    fn holding(id: AccountId, definition: AccountId, balance: u128) -> AccountWithMetadata {
        account_with_data(
            id.into_value(),
            TOKEN_PROGRAM,
            0,
            Data::from(&TokenHolding::Fungible {
                definition_id: definition,
                balance,
            }),
            false,
        )
    }

    fn exact_ata(owner: AccountId, definition: AccountId) -> AccountId {
        associated_token_account(ATA_PROGRAM, owner, definition)
    }

    fn context() -> ProgramContext {
        ProgramContext::new(ESCROW_PROGRAM, DEFAULT_PROGRAM_ID)
    }

    fn metadata_id() -> AccountId {
        spel_framework::pda::compute_pda(&ESCROW_PROGRAM, &[&SWAP_ID])
    }

    fn custody_id() -> AccountId {
        let label = spel_framework::pda::seed_from_str("custody");
        spel_framework::pda::compute_pda(&ESCROW_PROGRAM, &[&label, &SWAP_ID])
    }

    fn metadata_from(output: &SpelOutput) -> EscrowMetadata {
        EscrowMetadata::try_from_slice(output.post_states[0].account().data.as_ref())
            .expect("metadata output must be canonical borsh")
    }

    fn committed_metadata(output: &SpelOutput) -> AccountWithMetadata {
        let mut metadata = empty_account(metadata_id());
        metadata.account = output.post_states[0].account().clone();
        metadata.account.program_owner = ESCROW_PROGRAM;
        metadata
    }

    fn initialized() -> SpelOutput {
        zec_escrow::initialize_native(
            context(),
            empty_account(metadata_id()),
            empty_account(custody_id()),
            account([1; 32], AUTHENTICATED_TRANSFER, 200, true),
            account([2; 32], AUTHENTICATED_TRANSFER, 10, false),
            SWAP_ID,
            [31; 32],
            Sha256::digest(PREIMAGE).into(),
            AMOUNT,
            REFUND_AT,
            AUTHENTICATED_TRANSFER,
        )
        .expect("valid native escrow initialize")
    }

    fn funded() -> SpelOutput {
        let initialized = initialized();
        zec_escrow::fund_native(
            context(),
            committed_metadata(&initialized),
            account(custody_id().into_value(), AUTHENTICATED_TRANSFER, 0, false),
            account([1; 32], AUTHENTICATED_TRANSFER, 200, true),
            SWAP_ID,
        )
        .expect("valid exact native escrow funding")
    }

    fn funded_metadata() -> AccountWithMetadata {
        committed_metadata(&funded())
    }

    fn funded_custody() -> AccountWithMetadata {
        account(
            custody_id().into_value(),
            AUTHENTICATED_TRANSFER,
            AMOUNT,
            false,
        )
    }

    fn xmr_initialized() -> SpelOutput {
        let claim_authority = witnessed_account_id(&XMR_CLAIM_KEY);
        let refund_authority = witnessed_account_id(&XMR_REFUND_KEY);
        zec_escrow::initialize_native_xmr(
            context(),
            empty_account(metadata_id()),
            empty_account(custody_id()),
            account([1; 32], AUTHENTICATED_TRANSFER, 200, true),
            account([2; 32], AUTHENTICATED_TRANSFER, 10, false),
            actor(claim_authority.into_value(), false),
            actor(refund_authority.into_value(), false),
            SWAP_ID,
            [31; 32],
            XMR_CLAIM_KEY,
            XMR_REFUND_KEY,
            MAKER_DLEQ_COMMITMENT,
            TAKER_DLEQ_COMMITMENT,
            xmr_claim_partial_commitment(XMR_CLAIM_PARTIAL),
            AMOUNT,
            REFUND_AT,
            PUNISH_AT,
            AUTHENTICATED_TRANSFER,
        )
        .expect("valid XMR native escrow initialize")
    }

    fn xmr_funded() -> SpelOutput {
        let initialized = xmr_initialized();
        zec_escrow::fund_native(
            context(),
            committed_metadata(&initialized),
            account(custody_id().into_value(), AUTHENTICATED_TRANSFER, 0, false),
            account([1; 32], AUTHENTICATED_TRANSFER, 200, true),
            SWAP_ID,
        )
        .expect("valid exact XMR native escrow funding")
    }

    fn witnessed_initialized() -> SpelOutput {
        let aggregate_authority = witnessed_account_id(&XMR_CLAIM_KEY);
        zec_escrow::initialize_native_witnessed(
            context(),
            empty_account(metadata_id()),
            empty_account(custody_id()),
            account([1; 32], AUTHENTICATED_TRANSFER, 200, true),
            account([2; 32], AUTHENTICATED_TRANSFER, 10, false),
            actor(aggregate_authority.into_value(), false),
            SWAP_ID,
            [31; 32],
            XMR_CLAIM_KEY,
            AMOUNT,
            REFUND_AT,
            AUTHENTICATED_TRANSFER,
        )
        .expect("valid legacy witnessed native escrow initialize")
    }

    fn token_initialized(definition: AccountId) -> SpelOutput {
        zec_escrow::initialize_token(
            context(),
            empty_account(metadata_id()),
            actor([1; 32], true),
            actor([2; 32], false),
            definition_account(definition),
            SWAP_ID,
            [31; 32],
            Sha256::digest(PREIMAGE).into(),
            AMOUNT,
            REFUND_AT,
            ATA_PROGRAM,
        )
        .expect("valid custom-token escrow initialize")
    }

    fn token_funded(definition: AccountId) -> SpelOutput {
        let initialized = token_initialized(definition);
        let state = metadata_from(&initialized);
        zec_escrow::fund_token(
            context(),
            committed_metadata(&initialized),
            actor([1; 32], true),
            holding(state.depositor_asset, definition, 500),
            holding(state.custody, definition, 0),
            SWAP_ID,
        )
        .expect("signed owner funds the exact custody ATA")
    }

    fn token_initialized_witnessed(
        definition: AccountId,
        aggregate_x_only_public_key: [u8; 32],
    ) -> SpelOutput {
        let aggregate_authority = witnessed_account_id(&aggregate_x_only_public_key);
        zec_escrow::initialize_token_witnessed(
            context(),
            empty_account(metadata_id()),
            actor([1; 32], true),
            actor([2; 32], false),
            definition_account(definition),
            actor(aggregate_authority.into_value(), false),
            SWAP_ID,
            [31; 32],
            aggregate_x_only_public_key,
            AMOUNT,
            REFUND_AT,
            ATA_PROGRAM,
        )
        .expect("valid witnessed custom-token escrow initialize")
    }

    fn token_funded_witnessed(
        definition: AccountId,
        aggregate_x_only_public_key: [u8; 32],
    ) -> SpelOutput {
        let initialized = token_initialized_witnessed(definition, aggregate_x_only_public_key);
        let state = metadata_from(&initialized);
        zec_escrow::fund_token(
            context(),
            committed_metadata(&initialized),
            actor([1; 32], true),
            holding(state.depositor_asset, definition, 500),
            holding(state.custody, definition, 0),
            SWAP_ID,
        )
        .expect("signed owner funds the exact witnessed custody ATA")
    }

    fn assert_timestamp_only_window(output: &SpelOutput, start: Option<u64>, end: Option<u64>) {
        assert_eq!(output.block_validity_window.start(), None);
        assert_eq!(output.block_validity_window.end(), None);
        assert_eq!(output.timestamp_validity_window.start(), start);
        assert_eq!(output.timestamp_validity_window.end(), end);
    }

    #[test]
    fn witnessed_initialization_rejects_an_account_not_derived_from_the_aggregate_key() {
        let aggregate_x_only_public_key = [44; 32];
        let aggregate_authority = witnessed_account_id(&aggregate_x_only_public_key);
        let claimant = account([2; 32], AUTHENTICATED_TRANSFER, 10, false);
        assert_ne!(aggregate_authority, claimant.account_id);

        let result = zec_escrow::initialize_native_witnessed(
            context(),
            empty_account(metadata_id()),
            empty_account(custody_id()),
            account([1; 32], AUTHENTICATED_TRANSFER, 200, true),
            claimant,
            account([99; 32], DEFAULT_PROGRAM_ID, 0, false),
            SWAP_ID,
            [31; 32],
            aggregate_x_only_public_key,
            AMOUNT,
            REFUND_AT,
            AUTHENTICATED_TRANSFER,
        );
        assert!(
            result.is_err(),
            "mismatched aggregate authority must be rejected"
        );
    }

    #[test]
    fn lee_pdas_and_official_authenticated_transfer_abi_are_exact() {
        let official_program_id: ProgramId = AUTHENTICATED_TRANSFER;
        let public_abi_words: [u32; 8] = official_program_id;
        assert_eq!(public_abi_words, AUTHENTICATED_TRANSFER);
        let initialized = initialized();
        let state = metadata_from(&initialized);
        assert_eq!(state.custody, custody_id());
        assert_eq!(state.depositor_asset, state.depositor);
        assert_eq!(state.claimant_asset, state.claimant);
        assert_eq!(state.asset_program, AUTHENTICATED_TRANSFER);
        assert_eq!(state.custody_program, AUTHENTICATED_TRANSFER);
        assert_eq!(state.asset_definition, [0; 32]);
        assert_eq!(state.status, EscrowStatus::Empty);
        assert!(matches!(
            state.claim_authority,
            ClaimAuthority::Sha256Preimage { .. }
        ));
        assert_eq!(
            metadata_id(),
            AccountId::for_public_pda(&ESCROW_PROGRAM, &PdaSeed::new(SWAP_ID))
        );
        let initialize = &initialized.chained_calls[0];
        assert_eq!(initialize.program_id, AUTHENTICATED_TRANSFER);
        assert_eq!(initialize.pre_states[0].account_id, custody_id());
        assert!(initialize.pre_states[0].is_authorized);
        assert_eq!(
            initialize.instruction_data,
            ChainedCall::new(
                AUTHENTICATED_TRANSFER,
                vec![],
                &AuthenticatedTransferInstruction::Initialize,
            )
            .instruction_data
        );
        assert_eq!(initialize.pda_seeds, vec![custody_pda_seed(&SWAP_ID)]);
        assert_timestamp_only_window(&initialized, None, Some(REFUND_AT));

        let funded = funded();
        assert_eq!(metadata_from(&funded).status, EscrowStatus::Funded);
        let transfer = &funded.chained_calls[0];
        assert_eq!(transfer.program_id, AUTHENTICATED_TRANSFER);
        assert_eq!(
            transfer.instruction_data,
            ChainedCall::new(
                AUTHENTICATED_TRANSFER,
                vec![],
                &AuthenticatedTransferInstruction::Transfer { amount: AMOUNT },
            )
            .instruction_data
        );
        assert!(transfer.pda_seeds.is_empty());
        assert_timestamp_only_window(&funded, None, Some(REFUND_AT));
    }

    #[test]
    fn hashlock_claim_and_permissionless_refund_are_disjoint_and_atomic() {
        let claim = zec_escrow::claim_native(
            context(),
            funded_metadata(),
            funded_custody(),
            account([2; 32], AUTHENTICATED_TRANSFER, 10, true),
            SWAP_ID,
            PREIMAGE,
        )
        .expect("correct SHA-256 preimage claims before the boundary");
        assert_eq!(metadata_from(&claim).status, EscrowStatus::Claimed);
        assert_eq!(claim.chained_calls.len(), 1);
        assert_timestamp_only_window(&claim, None, Some(REFUND_AT));

        let refund = zec_escrow::refund_native(
            context(),
            funded_metadata(),
            funded_custody(),
            account([1; 32], AUTHENTICATED_TRANSFER, 125, false),
            SWAP_ID,
        )
        .expect("refund is permissionless and pays the immutable depositor");
        assert_eq!(metadata_from(&refund).status, EscrowStatus::Refunded);
        assert_eq!(refund.chained_calls.len(), 1);
        assert_eq!(
            refund.chained_calls[0].pre_states[1].account_id,
            AccountId::new([1; 32])
        );
        assert_timestamp_only_window(&refund, Some(REFUND_AT), None);

        assert!(
            zec_escrow::claim_native(
                context(),
                funded_metadata(),
                funded_custody(),
                account([2; 32], AUTHENTICATED_TRANSFER, 10, true),
                SWAP_ID,
                [99; 32],
            )
            .is_err(),
            "wrong preimage must produce no terminal output or transfer"
        );
        assert!(
            zec_escrow::refund_native(
                context(),
                funded_metadata(),
                account(
                    custody_id().into_value(),
                    AUTHENTICATED_TRANSFER,
                    AMOUNT - 1,
                    false,
                ),
                account([1; 32], AUTHENTICATED_TRANSFER, 125, false),
                SWAP_ID,
            )
            .is_err(),
            "partial custody can never be marked refunded"
        );
    }

    #[test]
    fn generated_idl_has_native_and_exact_ata_surfaces_with_permissionless_refunds() {
        let idl = __program_idl();
        let names = idl
            .instructions
            .iter()
            .map(|instruction| instruction.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            [
                "initialize_native",
                "initialize_native_witnessed",
                "fund_native",
                "claim_native",
                "claim_native_witnessed",
                "refund_native",
                "initialize_token",
                "create_token_custody",
                "fund_token",
                "claim_token",
                "refund_token",
                "initialize_token_witnessed",
                "claim_token_witnessed",
                "initialize_native_xmr",
                "authorize_native_xmr_claim",
                "claim_native_xmr",
                "refund_native_xmr",
                "punish_native_xmr",
            ]
        );
        for instruction_name in ["refund_native", "create_token_custody", "refund_token"] {
            let instruction = idl
                .instructions
                .iter()
                .find(|instruction| instruction.name == instruction_name)
                .expect("permissionless instruction in generated IDL");
            assert!(instruction.accounts.iter().all(|account| !account.signer));
        }
        for instruction_name in ["initialize_token", "fund_token", "claim_token"] {
            let instruction = idl
                .instructions
                .iter()
                .find(|instruction| instruction.name == instruction_name)
                .expect("owner-authorized instruction in generated IDL");
            assert!(instruction.accounts.iter().any(|account| account.signer));
        }
        for instruction_name in ["initialize_token_witnessed", "claim_token_witnessed"] {
            let instruction = idl
                .instructions
                .iter()
                .find(|instruction| instruction.name == instruction_name)
                .expect("aggregate-authorized token instruction in generated IDL");
            assert!(instruction.accounts.iter().any(|account| account.signer));
        }
        for instruction_name in [
            "initialize_native_xmr",
            "authorize_native_xmr_claim",
            "claim_native_xmr",
            "refund_native_xmr",
            "punish_native_xmr",
        ] {
            let instruction = idl
                .instructions
                .iter()
                .find(|instruction| instruction.name == instruction_name)
                .expect("authority-bearing XMR instruction in generated IDL");
            assert!(instruction.accounts.iter().any(|account| account.signer));
        }
    }

    #[test]
    fn every_legacy_and_xmr_variant_is_append_only_on_the_public_wire() {
        let tag = |instruction: Instruction| {
            ChainedCall::new(ESCROW_PROGRAM, vec![], &instruction).instruction_data[0]
        };
        assert_eq!(
            tag(Instruction::InitializeNative {
                swap_id: SWAP_ID,
                terms_hash: [31; 32],
                secret_digest: Sha256::digest(PREIMAGE).into(),
                amount: AMOUNT,
                refund_at: REFUND_AT,
                authenticated_transfer_program: AUTHENTICATED_TRANSFER,
            }),
            0,
        );
        assert_eq!(
            tag(Instruction::InitializeNativeWitnessed {
                swap_id: SWAP_ID,
                terms_hash: [31; 32],
                aggregate_x_only_public_key: [44; 32],
                amount: AMOUNT,
                refund_at: REFUND_AT,
                authenticated_transfer_program: AUTHENTICATED_TRANSFER,
            }),
            1,
        );
        assert_eq!(tag(Instruction::FundNative { swap_id: SWAP_ID }), 2);
        assert_eq!(
            tag(Instruction::ClaimNative {
                swap_id: SWAP_ID,
                preimage: PREIMAGE,
            }),
            3,
        );
        assert_eq!(
            tag(Instruction::ClaimNativeWitnessed { swap_id: SWAP_ID }),
            4,
        );
        assert_eq!(tag(Instruction::RefundNative { swap_id: SWAP_ID }), 5);
        assert_eq!(
            tag(Instruction::InitializeToken {
                swap_id: SWAP_ID,
                terms_hash: [31; 32],
                secret_digest: Sha256::digest(PREIMAGE).into(),
                amount: AMOUNT,
                refund_at: REFUND_AT,
                ata_program: ATA_PROGRAM,
            }),
            6,
        );
        assert_eq!(tag(Instruction::CreateTokenCustody { swap_id: SWAP_ID }), 7,);
        assert_eq!(tag(Instruction::FundToken { swap_id: SWAP_ID }), 8);
        assert_eq!(
            tag(Instruction::ClaimToken {
                swap_id: SWAP_ID,
                preimage: PREIMAGE,
            }),
            9,
        );
        assert_eq!(tag(Instruction::RefundToken { swap_id: SWAP_ID }), 10);
        assert_eq!(
            tag(Instruction::InitializeTokenWitnessed {
                swap_id: SWAP_ID,
                terms_hash: [31; 32],
                aggregate_x_only_public_key: [44; 32],
                amount: AMOUNT,
                refund_at: REFUND_AT,
                ata_program: ATA_PROGRAM,
            }),
            11,
        );
        assert_eq!(
            tag(Instruction::ClaimTokenWitnessed { swap_id: SWAP_ID }),
            12,
        );
        assert_eq!(
            tag(Instruction::InitializeNativeXmr {
                swap_id: SWAP_ID,
                terms_hash: [31; 32],
                claim_aggregate_x_only_public_key: XMR_CLAIM_KEY,
                refund_aggregate_x_only_public_key: XMR_REFUND_KEY,
                maker_dleq_transcript_commitment: MAKER_DLEQ_COMMITMENT,
                taker_dleq_transcript_commitment: TAKER_DLEQ_COMMITMENT,
                claim_partial_commitment: xmr_claim_partial_commitment(XMR_CLAIM_PARTIAL),
                amount: AMOUNT,
                refund_at: REFUND_AT,
                punish_at: PUNISH_AT,
                authenticated_transfer_program: AUTHENTICATED_TRANSFER,
            }),
            13,
        );
        assert_eq!(
            tag(Instruction::AuthorizeNativeXmrClaim {
                swap_id: SWAP_ID,
                claim_partial: XMR_CLAIM_PARTIAL,
            }),
            14,
        );
        assert_eq!(tag(Instruction::ClaimNativeXmr { swap_id: SWAP_ID }), 15,);
        assert_eq!(tag(Instruction::RefundNativeXmr { swap_id: SWAP_ID }), 16,);
        assert_eq!(tag(Instruction::PunishNativeXmr { swap_id: SWAP_ID }), 17,);
    }

    #[test]
    fn legacy_metadata_encodings_remain_byte_exact() {
        let preimage_metadata = metadata_from(&initialized());
        let witnessed_metadata = metadata_from(&witnessed_initialized());
        let preimage_digest: [u8; 32] = Sha256::digest(
            borsh::to_vec(&preimage_metadata).expect("legacy preimage metadata serializes"),
        )
        .into();
        let witnessed_digest: [u8; 32] = Sha256::digest(
            borsh::to_vec(&witnessed_metadata).expect("legacy witnessed metadata serializes"),
        )
        .into();

        assert_eq!(
            preimage_digest,
            [
                219, 57, 173, 104, 233, 3, 91, 12, 195, 91, 238, 63, 134, 116, 221, 16, 253, 108,
                92, 83, 141, 106, 103, 81, 208, 116, 168, 243, 93, 41, 76, 111,
            ]
        );
        assert_eq!(
            witnessed_digest,
            [
                87, 124, 100, 28, 61, 59, 141, 60, 96, 188, 109, 135, 135, 19, 193, 160, 231, 183,
                153, 92, 53, 244, 186, 242, 107, 217, 40, 249, 48, 238, 153, 80,
            ]
        );
    }

    #[test]
    fn xmr_initialization_rejects_aliased_authorities_and_invalid_windows() {
        let claim_authority = witnessed_account_id(&XMR_CLAIM_KEY);
        let refund_authority = witnessed_account_id(&XMR_REFUND_KEY);
        let initialize = |depositor: AccountWithMetadata,
                          claim: AccountWithMetadata,
                          refund: AccountWithMetadata,
                          punish_at| {
            zec_escrow::initialize_native_xmr(
                context(),
                empty_account(metadata_id()),
                empty_account(custody_id()),
                depositor,
                account([2; 32], AUTHENTICATED_TRANSFER, 10, false),
                claim,
                refund,
                SWAP_ID,
                [31; 32],
                XMR_CLAIM_KEY,
                XMR_REFUND_KEY,
                MAKER_DLEQ_COMMITMENT,
                TAKER_DLEQ_COMMITMENT,
                xmr_claim_partial_commitment(XMR_CLAIM_PARTIAL),
                AMOUNT,
                REFUND_AT,
                punish_at,
                AUTHENTICATED_TRANSFER,
            )
        };

        assert!(
            initialize(
                account([1; 32], AUTHENTICATED_TRANSFER, 200, true),
                actor(claim_authority.into_value(), false),
                actor(refund_authority.into_value(), false),
                REFUND_AT,
            )
            .is_err()
        );
        assert!(
            initialize(
                account(
                    claim_authority.into_value(),
                    AUTHENTICATED_TRANSFER,
                    200,
                    true
                ),
                actor(claim_authority.into_value(), false),
                actor(refund_authority.into_value(), false),
                PUNISH_AT,
            )
            .is_err()
        );
        assert!(
            initialize(
                account([1; 32], AUTHENTICATED_TRANSFER, 200, true),
                actor(refund_authority.into_value(), false),
                actor(claim_authority.into_value(), false),
                PUNISH_AT,
            )
            .is_err()
        );
    }

    #[test]
    fn xmr_escrow_rejects_legacy_claims_wrong_authorities_and_destinations() {
        let funded = xmr_funded();
        let claim_authority = witnessed_account_id(&XMR_CLAIM_KEY);
        let refund_authority = witnessed_account_id(&XMR_REFUND_KEY);

        assert!(
            zec_escrow::authorize_native_xmr_claim(
                context(),
                committed_metadata(&funded),
                account([1; 32], AUTHENTICATED_TRANSFER, 125, true),
                SWAP_ID,
                [49; 32],
            )
            .is_err()
        );
        assert!(
            zec_escrow::authorize_native_xmr_claim(
                context(),
                committed_metadata(&funded),
                account([3; 32], AUTHENTICATED_TRANSFER, 125, true),
                SWAP_ID,
                XMR_CLAIM_PARTIAL,
            )
            .is_err()
        );
        assert!(
            zec_escrow::authorize_native_xmr_claim(
                context(),
                committed_metadata(&funded),
                actor([1; 32], true),
                SWAP_ID,
                XMR_CLAIM_PARTIAL,
            )
            .is_err()
        );
        assert!(
            zec_escrow::claim_native(
                context(),
                committed_metadata(&funded),
                funded_custody(),
                account([2; 32], AUTHENTICATED_TRANSFER, 10, true),
                SWAP_ID,
                PREIMAGE,
            )
            .is_err()
        );
        assert!(
            zec_escrow::claim_native_witnessed(
                context(),
                committed_metadata(&funded),
                funded_custody(),
                account([2; 32], AUTHENTICATED_TRANSFER, 10, false),
                actor(claim_authority.into_value(), true),
                SWAP_ID,
            )
            .is_err()
        );
        assert!(
            zec_escrow::claim_native_xmr(
                context(),
                committed_metadata(&funded),
                funded_custody(),
                account([2; 32], AUTHENTICATED_TRANSFER, 10, false),
                actor(refund_authority.into_value(), true),
                SWAP_ID,
            )
            .is_err()
        );
        assert!(
            zec_escrow::refund_native_xmr(
                context(),
                committed_metadata(&funded),
                funded_custody(),
                account([1; 32], AUTHENTICATED_TRANSFER, 125, false),
                actor(claim_authority.into_value(), true),
                SWAP_ID,
            )
            .is_err()
        );
        assert!(
            zec_escrow::claim_native_xmr(
                context(),
                committed_metadata(&funded),
                funded_custody(),
                account([3; 32], AUTHENTICATED_TRANSFER, 10, false),
                actor(claim_authority.into_value(), true),
                SWAP_ID,
            )
            .is_err()
        );
        assert!(
            zec_escrow::refund_native_xmr(
                context(),
                committed_metadata(&funded),
                funded_custody(),
                account([3; 32], AUTHENTICATED_TRANSFER, 125, false),
                actor(refund_authority.into_value(), true),
                SWAP_ID,
            )
            .is_err()
        );
        assert!(
            zec_escrow::punish_native_xmr(
                context(),
                committed_metadata(&funded),
                funded_custody(),
                account([3; 32], AUTHENTICATED_TRANSFER, 10, true),
                SWAP_ID,
            )
            .is_err()
        );
    }

    #[test]
    fn xmr_native_claim_refund_and_punish_are_atomic_and_disjoint() {
        let initialized = xmr_initialized();
        let initialized_state = metadata_from(&initialized);
        assert_eq!(initialized_state.version, XMR_ESCROW_METADATA_VERSION);
        assert_timestamp_only_window(&initialized, None, Some(REFUND_AT));

        let funded = xmr_funded();
        let funded_state = metadata_from(&funded);
        assert_eq!(funded_state.status, EscrowStatus::Funded);
        assert_timestamp_only_window(&funded, None, Some(REFUND_AT));

        assert!(
            zec_escrow::refund_native(
                context(),
                committed_metadata(&funded),
                funded_custody(),
                account([1; 32], AUTHENTICATED_TRANSFER, 125, false),
                SWAP_ID,
            )
            .is_err(),
            "generic unsigned refund must never bypass the s_b reveal"
        );

        let claim_authority = witnessed_account_id(&XMR_CLAIM_KEY);
        assert!(
            zec_escrow::claim_native_xmr(
                context(),
                committed_metadata(&funded),
                funded_custody(),
                account([2; 32], AUTHENTICATED_TRANSFER, 10, false),
                actor(claim_authority.into_value(), true),
                SWAP_ID,
            )
            .is_err(),
            "Maker cannot claim before the Taker publishes the bound partial on LEZ"
        );
        let authorization = zec_escrow::authorize_native_xmr_claim(
            context(),
            committed_metadata(&funded),
            account([1; 32], AUTHENTICATED_TRANSFER, 125, true),
            SWAP_ID,
            XMR_CLAIM_PARTIAL,
        )
        .expect("Taker publishes the exact agreement-bound partial on LEZ");
        assert_eq!(
            metadata_from(&authorization).status,
            EscrowStatus::XmrClaimAuthorized
        );
        assert!(authorization.chained_calls.is_empty());
        assert_timestamp_only_window(&authorization, None, Some(REFUND_AT));

        let claim = zec_escrow::claim_native_xmr(
            context(),
            committed_metadata(&authorization),
            funded_custody(),
            account([2; 32], AUTHENTICATED_TRANSFER, 10, false),
            actor(claim_authority.into_value(), true),
            SWAP_ID,
        )
        .expect("claim aggregate witness releases fixed custody to Maker");
        assert_eq!(metadata_from(&claim).status, EscrowStatus::Claimed);
        assert_timestamp_only_window(&claim, None, Some(REFUND_AT));
        assert_eq!(
            claim.chained_calls[0].pre_states[1].account_id,
            AccountId::new([2; 32])
        );

        let refund_authority = witnessed_account_id(&XMR_REFUND_KEY);
        let refund = zec_escrow::refund_native_xmr(
            context(),
            committed_metadata(&xmr_funded()),
            funded_custody(),
            account([1; 32], AUTHENTICATED_TRANSFER, 125, false),
            actor(refund_authority.into_value(), true),
            SWAP_ID,
        )
        .expect("refund aggregate witness releases fixed custody to Taker");
        assert_eq!(metadata_from(&refund).status, EscrowStatus::Refunded);
        assert_timestamp_only_window(&refund, Some(REFUND_AT), Some(PUNISH_AT));
        assert_eq!(
            refund.chained_calls[0].pre_states[1].account_id,
            AccountId::new([1; 32])
        );

        let punish = zec_escrow::punish_native_xmr(
            context(),
            committed_metadata(&xmr_funded()),
            funded_custody(),
            account([2; 32], AUTHENTICATED_TRANSFER, 10, true),
            SWAP_ID,
        )
        .expect("Maker claimant can punish after the Taker abandons its window");
        assert_eq!(metadata_from(&punish).status, EscrowStatus::Claimed);
        assert_timestamp_only_window(&punish, Some(PUNISH_AT), None);
        assert_eq!(
            punish.chained_calls[0].pre_states[1].account_id,
            AccountId::new([2; 32])
        );
    }

    #[test]
    fn witnessed_token_claims_bind_two_definitions_exact_atas_and_aggregate_authority() {
        let aggregate_x_only_public_key = [44; 32];
        let aggregate_authority = witnessed_account_id(&aggregate_x_only_public_key);
        let mut custody_ids = Vec::new();

        for definition_bytes in [[41; 32], [42; 32]] {
            let definition = AccountId::new(definition_bytes);
            let funded = token_funded_witnessed(definition, aggregate_x_only_public_key);
            let state = metadata_from(&funded);
            assert_eq!(state.asset_definition, definition_bytes);
            assert_eq!(state.custody, exact_ata(metadata_id(), definition));
            assert_eq!(state.claimant_asset, exact_ata(state.claimant, definition));
            assert_eq!(
                state.claim_authority,
                ClaimAuthority::AggregateWitness {
                    x_only_public_key: aggregate_x_only_public_key,
                    account_id: aggregate_authority,
                }
            );

            let claim = zec_escrow::claim_token_witnessed(
                context(),
                committed_metadata(&funded),
                holding(state.custody, definition, AMOUNT),
                actor([2; 32], false),
                holding(state.claimant_asset, definition, 0),
                actor(aggregate_authority.into_value(), true),
                SWAP_ID,
            )
            .expect("aggregate witness claims to the fixed claimant ATA");
            assert_eq!(metadata_from(&claim).status, EscrowStatus::Claimed);
            assert_eq!(
                claim.chained_calls[0].pre_states[2].account_id,
                state.claimant_asset
            );
            assert_timestamp_only_window(&claim, None, Some(REFUND_AT));
            custody_ids.push(state.custody);
        }

        assert_ne!(custody_ids[0], custody_ids[1]);
    }

    #[test]
    fn witnessed_token_paths_reject_wrong_definition_ata_and_aggregate_authority() {
        let definition = AccountId::new([41; 32]);
        let other_definition = AccountId::new([42; 32]);
        let aggregate_x_only_public_key = [44; 32];
        let aggregate_authority = witnessed_account_id(&aggregate_x_only_public_key);

        assert!(
            zec_escrow::initialize_token_witnessed(
                context(),
                empty_account(metadata_id()),
                actor([1; 32], true),
                actor([2; 32], false),
                definition_account(definition),
                actor([99; 32], false),
                SWAP_ID,
                [31; 32],
                aggregate_x_only_public_key,
                AMOUNT,
                REFUND_AT,
                ATA_PROGRAM,
            )
            .is_err(),
            "initialize must bind the aggregate account to its exact BIP-340 key"
        );

        let funded = token_funded_witnessed(definition, aggregate_x_only_public_key);
        let state = metadata_from(&funded);
        let claim = |custody, claimant_asset, authority| {
            zec_escrow::claim_token_witnessed(
                context(),
                committed_metadata(&funded),
                custody,
                actor([2; 32], false),
                claimant_asset,
                authority,
                SWAP_ID,
            )
        };
        assert!(
            claim(
                holding(state.custody, other_definition, AMOUNT),
                holding(state.claimant_asset, definition, 0),
                actor(aggregate_authority.into_value(), true),
            )
            .is_err(),
            "custody for another definition must fail closed"
        );
        assert!(
            claim(
                holding(state.custody, definition, AMOUNT),
                holding(AccountId::new([98; 32]), definition, 0),
                actor(aggregate_authority.into_value(), true),
            )
            .is_err(),
            "a non-canonical claimant ATA must fail closed"
        );
        assert!(
            claim(
                holding(state.custody, definition, AMOUNT),
                holding(state.claimant_asset, definition, 0),
                actor([99; 32], true),
            )
            .is_err(),
            "an unrelated signer must not replace the aggregate authority"
        );
    }

    #[test]
    fn two_token_definitions_use_distinct_exact_ata_custody_and_official_calls() {
        let official_program_id: ProgramId = ATA_PROGRAM;
        let public_abi_words: [u32; 8] = official_program_id;
        assert_eq!(public_abi_words, ATA_PROGRAM);
        let mut custody_ids = Vec::new();
        for definition_bytes in [[41; 32], [42; 32]] {
            let definition = AccountId::new(definition_bytes);
            let initialized = token_initialized(definition);
            let state = metadata_from(&initialized);
            let expected_custody = exact_ata(metadata_id(), definition);
            assert_eq!(state.asset_definition, definition_bytes);
            assert_eq!(state.asset_program, TOKEN_PROGRAM);
            assert_eq!(state.custody_program, ATA_PROGRAM);
            assert_eq!(state.custody, expected_custody);
            assert_eq!(
                state.depositor_asset,
                exact_ata(state.depositor, definition)
            );
            assert_eq!(state.claimant_asset, exact_ata(state.claimant, definition));
            assert_ne!(state.custody, custody_id());
            assert!(initialized.chained_calls.is_empty());
            assert_timestamp_only_window(&initialized, None, Some(REFUND_AT));

            let created = zec_escrow::create_token_custody(
                context(),
                committed_metadata(&initialized),
                definition_account(definition),
                empty_account(expected_custody),
                SWAP_ID,
            )
            .expect("permissionless exact custody ATA creation");
            let create = &created.chained_calls[0];
            assert_eq!(create.program_id, ATA_PROGRAM);
            assert_eq!(
                create.instruction_data,
                ChainedCall::new(
                    ATA_PROGRAM,
                    vec![],
                    &ata_core::Instruction::Create {
                        ata_program_id: ATA_PROGRAM,
                    },
                )
                .instruction_data
            );
            assert!(create.pda_seeds.is_empty());

            let funded = token_funded(definition);
            let transfer = &funded.chained_calls[0];
            assert_eq!(metadata_from(&funded).status, EscrowStatus::Funded);
            assert_eq!(transfer.program_id, ATA_PROGRAM);
            assert_eq!(transfer.pre_states[0].account_id, state.depositor);
            assert_eq!(transfer.pre_states[1].account_id, state.depositor_asset);
            assert_eq!(transfer.pre_states[2].account_id, state.custody);
            assert!(transfer.pre_states[0].is_authorized);
            assert!(transfer.pda_seeds.is_empty());
            assert_eq!(
                transfer.instruction_data,
                ChainedCall::new(
                    ATA_PROGRAM,
                    vec![],
                    &ata_core::Instruction::Transfer {
                        ata_program_id: ATA_PROGRAM,
                        amount: AMOUNT,
                    },
                )
                .instruction_data
            );
            custody_ids.push(state.custody);
        }
        assert_ne!(custody_ids[0], custody_ids[1]);
    }

    #[test]
    fn token_claim_and_permissionless_refund_are_fixed_atomic_and_disjoint() {
        let definition = AccountId::new([41; 32]);
        let funded = token_funded(definition);
        let state = metadata_from(&funded);

        assert!(
            zec_escrow::claim_token(
                context(),
                committed_metadata(&funded),
                holding(state.custody, definition, AMOUNT),
                actor([2; 32], true),
                holding(state.claimant_asset, definition, 0),
                SWAP_ID,
                [99; 32],
            )
            .is_err(),
            "wrong SHA-256 preimage must produce no terminal output"
        );

        let claim = zec_escrow::claim_token(
            context(),
            committed_metadata(&funded),
            holding(state.custody, definition, AMOUNT),
            actor([2; 32], true),
            holding(state.claimant_asset, definition, 0),
            SWAP_ID,
            PREIMAGE,
        )
        .expect("fixed claimant owner claims to the exact claimant ATA");
        assert_eq!(metadata_from(&claim).status, EscrowStatus::Claimed);
        assert_timestamp_only_window(&claim, None, Some(REFUND_AT));
        let claim_transfer = &claim.chained_calls[0];
        assert_eq!(claim_transfer.program_id, ATA_PROGRAM);
        assert_eq!(claim_transfer.pre_states[0].account_id, metadata_id());
        assert!(claim_transfer.pre_states[0].is_authorized);
        assert_eq!(
            claim_transfer.pre_states[2].account_id,
            state.claimant_asset
        );
        assert_eq!(claim_transfer.pda_seeds, vec![metadata_pda_seed(&SWAP_ID)]);

        assert!(
            zec_escrow::refund_token(
                context(),
                committed_metadata(&funded),
                holding(state.custody, definition, AMOUNT),
                holding(AccountId::new([99; 32]), definition, 0),
                SWAP_ID,
            )
            .is_err(),
            "refund can never redirect away from the immutable depositor ATA"
        );
        let refund = zec_escrow::refund_token(
            context(),
            committed_metadata(&funded),
            holding(state.custody, definition, AMOUNT),
            holding(state.depositor_asset, definition, 425),
            SWAP_ID,
        )
        .expect("any submitter can refund to the immutable depositor ATA");
        assert_eq!(metadata_from(&refund).status, EscrowStatus::Refunded);
        assert_timestamp_only_window(&refund, Some(REFUND_AT), None);
        assert_eq!(
            refund.chained_calls[0].pre_states[2].account_id,
            state.depositor_asset
        );
        assert_eq!(
            refund.chained_calls[0].pda_seeds,
            vec![metadata_pda_seed(&SWAP_ID)]
        );
    }

    #[test]
    fn token_paths_reject_wrong_definition_partial_custody_and_prefunding() {
        let definition = AccountId::new([41; 32]);
        let other_definition = AccountId::new([42; 32]);
        let initialized = token_initialized(definition);
        let state = metadata_from(&initialized);

        assert!(
            zec_escrow::create_token_custody(
                context(),
                committed_metadata(&initialized),
                definition_account(other_definition),
                empty_account(state.custody),
                SWAP_ID,
            )
            .is_err()
        );
        assert!(
            zec_escrow::fund_token(
                context(),
                committed_metadata(&initialized),
                actor([1; 32], true),
                holding(state.depositor_asset, definition, AMOUNT - 1),
                holding(state.custody, definition, 0),
                SWAP_ID,
            )
            .is_err()
        );
        assert!(
            zec_escrow::fund_token(
                context(),
                committed_metadata(&initialized),
                actor([1; 32], true),
                holding(state.depositor_asset, definition, 500),
                holding(state.custody, definition, 1),
                SWAP_ID,
            )
            .is_err(),
            "pre-funded custody would violate exact escrow accounting"
        );

        let funded = token_funded(definition);
        let funded_state = metadata_from(&funded);
        assert!(
            zec_escrow::claim_token(
                context(),
                committed_metadata(&funded),
                holding(funded_state.custody, definition, AMOUNT - 1),
                actor([2; 32], true),
                holding(funded_state.claimant_asset, definition, 0),
                SWAP_ID,
                PREIMAGE,
            )
            .is_err(),
            "partial custody can never become terminal metadata"
        );
    }
}
