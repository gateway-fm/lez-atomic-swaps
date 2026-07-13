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
}

#[account_type]
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct EscrowMetadata {
    pub version: u8,
    pub swap_id: [u8; 32],
    pub terms_hash: [u8; 32],
    pub secret_digest: [u8; 32],
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
const ERROR_WRONG_PREIMAGE: u32 = 4;
const ERROR_UNSUPPORTED_VERSION: u32 = 5;

fn custom_error(code: u32, message: impl Into<String>) -> SpelError {
    SpelError::custom(code, message)
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
    if state.version != 1 {
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
        if amount == 0
            || refund_at == 0
            || terms_hash == [0; 32]
            || secret_digest == [0; 32]
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

        let mut metadata = metadata;
        let state = EscrowMetadata {
            version: 1,
            swap_id,
            terms_hash,
            secret_digest,
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
        };
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
        let mut metadata = metadata;
        let mut state = read_metadata(&metadata)?;
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
        let digest: [u8; 32] = Sha256::digest(preimage).into();
        if digest != state.secret_digest {
            return Err(custom_error(ERROR_WRONG_PREIMAGE, "wrong preimage"));
        }

        let transfer = native_transfer_call(
            state.asset_program,
            custody.clone(),
            claimant.clone(),
            state.amount,
            true,
            &swap_id,
        );
        state.status = EscrowStatus::Claimed;
        write_metadata(&mut metadata, &state)?;
        Ok(
            SpelOutput::execute(vec![metadata, custody, claimant], vec![transfer])
                .with_timestamp_validity_window(..state.refund_at),
        )
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
        let token_program = token_definition.account.program_owner;
        if amount == 0
            || refund_at == 0
            || terms_hash == [0; 32]
            || secret_digest == [0; 32]
            || token_program == DEFAULT_PROGRAM_ID
            || token_program == ctx.self_program_id
            || ata_program == DEFAULT_PROGRAM_ID
            || ata_program == ctx.self_program_id
            || ata_program == token_program
        {
            return Err(custom_error(ERROR_INVALID_TERMS, "invalid token terms"));
        }
        require_fungible_definition(&token_definition, token_program)?;

        let mut metadata = metadata;
        let definition = token_definition.account_id;
        let custody = associated_token_account(ata_program, metadata.account_id, definition);
        let depositor_asset =
            associated_token_account(ata_program, depositor_owner.account_id, definition);
        let claimant_asset =
            associated_token_account(ata_program, claimant_owner.account_id, definition);
        let state = EscrowMetadata {
            version: 1,
            swap_id,
            terms_hash,
            secret_digest,
            depositor: depositor_owner.account_id,
            depositor_asset,
            claimant: claimant_owner.account_id,
            claimant_asset,
            custody,
            asset_program: token_program,
            custody_program: ata_program,
            asset_definition: definition.into_value(),
            amount,
            refund_at,
            status: EscrowStatus::Empty,
        };
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
        let mut metadata = metadata;
        let mut state = read_metadata(&metadata)?;
        require_funded(&state)?;
        if state.swap_id != swap_id
            || state.claimant != claimant_owner.account_id
            || state.claimant_asset != claimant_asset.account_id
            || state.custody != custody.account_id
            || claimant_asset.account.program_owner != state.asset_program
            || custody.account.program_owner != state.asset_program
            || token_definition(&claimant_asset)?.into_value() != state.asset_definition
            || token_definition(&custody)?.into_value() != state.asset_definition
            || fungible_balance(&custody)? != state.amount
        {
            return Err(custom_error(
                ERROR_ACCOUNT_BINDING,
                "token claim account binding mismatch",
            ));
        }
        let digest: [u8; 32] = Sha256::digest(preimage).into();
        if digest != state.secret_digest {
            return Err(custom_error(ERROR_WRONG_PREIMAGE, "wrong preimage"));
        }

        state.status = EscrowStatus::Claimed;
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

    fn assert_timestamp_only_window(output: &SpelOutput, start: Option<u64>, end: Option<u64>) {
        assert_eq!(output.block_validity_window.start(), None);
        assert_eq!(output.block_validity_window.end(), None);
        assert_eq!(output.timestamp_validity_window.start(), start);
        assert_eq!(output.timestamp_validity_window.end(), end);
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
                "fund_native",
                "claim_native",
                "refund_native",
                "initialize_token",
                "create_token_custody",
                "fund_token",
                "claim_token",
                "refund_token",
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
