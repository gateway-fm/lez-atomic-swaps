//! Minimal executable SPEL/LEZ compatibility contract for the ZEC escrow.

#![allow(dead_code)]

use nssa_core::{
    account::{Account, AccountId, Data},
    program::{ChainedCall, Claim, PdaSeed, ProgramId, DEFAULT_PROGRAM_ID},
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
#[derive(Clone, BorshSerialize, BorshDeserialize)]
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

fn token_definition(account: &AccountWithMetadata) -> Result<AccountId, SpelError> {
    TokenHolding::try_from(&account.account.data)
        .map(|holding| holding.definition_id())
        .map_err(|_| custom_error(ERROR_ACCOUNT_BINDING, "invalid token holding"))
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

fn native_initialize_call(
    authenticated_transfer_program: ProgramId,
    mut custody: AccountWithMetadata,
    swap_id: &[u8; 32],
) -> ChainedCall {
    custody.is_authorized = true;
    ChainedCall::new(authenticated_transfer_program, vec![custody], &0_u128)
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
        &amount,
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
        ))
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
        if state.version != 1 {
            return Err(custom_error(
                ERROR_UNSUPPORTED_VERSION,
                "unsupported escrow metadata version",
            ));
        }
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
        Ok(SpelOutput::execute(
            vec![metadata, custody, depositor],
            vec![transfer],
        ))
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
        if state.version != 1 {
            return Err(custom_error(
                ERROR_UNSUPPORTED_VERSION,
                "unsupported escrow metadata version",
            ));
        }
        if state.status != EscrowStatus::Funded {
            return Err(custom_error(ERROR_NOT_FUNDED, "escrow is not funded"));
        }
        if state.swap_id != swap_id
            || state.asset_definition != [0; 32]
            || state.asset_program != state.custody_program
            || state.claimant != claimant.account_id
            || state.custody != custody.account_id
            || claimant.account.program_owner != state.asset_program
            || custody.account.program_owner != state.asset_program
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
        if state.version != 1 {
            return Err(custom_error(
                ERROR_UNSUPPORTED_VERSION,
                "unsupported escrow metadata version",
            ));
        }
        if state.status != EscrowStatus::Funded {
            return Err(custom_error(ERROR_NOT_FUNDED, "escrow is not funded"));
        }
        if state.swap_id != swap_id
            || state.asset_definition != [0; 32]
            || state.asset_program != state.custody_program
            || state.depositor != depositor.account_id
            || state.custody != custody.account_id
            || depositor.account.program_owner != state.asset_program
            || custody.account.program_owner != state.asset_program
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
        ))
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
        if state.version != 1 {
            return Err(custom_error(
                ERROR_UNSUPPORTED_VERSION,
                "unsupported escrow metadata version",
            ));
        }
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
        Ok(SpelOutput::execute(
            vec![metadata, token_definition, custody],
            vec![create],
        ))
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
        if state.version != 1 {
            return Err(custom_error(
                ERROR_UNSUPPORTED_VERSION,
                "unsupported escrow metadata version",
            ));
        }
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
        ))
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
        if state.version != 1 {
            return Err(custom_error(
                ERROR_UNSUPPORTED_VERSION,
                "unsupported escrow metadata version",
            ));
        }
        if state.status != EscrowStatus::Funded {
            return Err(custom_error(ERROR_NOT_FUNDED, "escrow is not funded"));
        }
        if state.swap_id != swap_id
            || state.claimant != claimant_owner.account_id
            || state.claimant_asset != claimant_asset.account_id
            || state.custody != custody.account_id
            || claimant_asset.account.program_owner != state.asset_program
            || custody.account.program_owner != state.asset_program
            || token_definition(&claimant_asset)?.into_value() != state.asset_definition
            || token_definition(&custody)?.into_value() != state.asset_definition
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
        if state.version != 1 {
            return Err(custom_error(
                ERROR_UNSUPPORTED_VERSION,
                "unsupported escrow metadata version",
            ));
        }
        if state.status != EscrowStatus::Funded {
            return Err(custom_error(ERROR_NOT_FUNDED, "escrow is not funded"));
        }
        if state.swap_id != swap_id
            || state.depositor_asset != depositor_asset.account_id
            || state.custody != custody.account_id
            || depositor_asset.account.program_owner != state.asset_program
            || custody.account.program_owner != state.asset_program
            || token_definition(&depositor_asset)?.into_value() != state.asset_definition
            || token_definition(&custody)?.into_value() != state.asset_definition
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
    use nssa_core::{
        account::{Account, AccountId, Data},
        program::{validate_execution, ChainedCall, DEFAULT_PROGRAM_ID},
    };
    use sha2::{Digest, Sha256};
    use token_core::{TokenDefinition, TokenHolding};

    const ESCROW_PROGRAM: ProgramId = [7; 8];
    const SWAP_ID: [u8; 32] = [11; 32];
    const PREIMAGE: [u8; 32] = [12; 32];
    const AMOUNT: u128 = 75;
    const REFUND_AT: u64 = 1_000;

    fn account(
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

    fn empty_account(id: [u8; 32]) -> AccountWithMetadata {
        account(id, DEFAULT_PROGRAM_ID, 0, Data::default(), false)
    }

    fn context() -> ProgramContext {
        ProgramContext::new(ESCROW_PROGRAM, DEFAULT_PROGRAM_ID)
    }

    fn metadata_account() -> AccountWithMetadata {
        let id = spel_framework::pda::compute_pda(&ESCROW_PROGRAM, &[&SWAP_ID]);
        empty_account(id.into_value())
    }

    fn custody_id() -> AccountId {
        let label = spel_framework::pda::seed_from_str("custody");
        spel_framework::pda::compute_pda(&ESCROW_PROGRAM, &[&label, &SWAP_ID])
    }

    fn actual_native_program() -> ProgramId {
        nssa::program::Program::authenticated_transfer_program().id()
    }

    fn actual_ata_program() -> ProgramId {
        nssa::program::Program::ata().id()
    }

    fn actual_token_program() -> ProgramId {
        nssa::program::Program::token().id()
    }

    fn token_holding(id: [u8; 32], definition: AccountId, balance: u128) -> AccountWithMetadata {
        let holding = TokenHolding::Fungible {
            definition_id: definition,
            balance,
        };
        account(id, actual_token_program(), 0, Data::from(&holding), false)
    }

    fn token_definition_account(definition: AccountId) -> AccountWithMetadata {
        let data = Data::from(&TokenDefinition::Fungible {
            name: "M2-test-token".into(),
            total_supply: 1_000,
            metadata_id: None,
        });
        account(
            definition.into_value(),
            actual_token_program(),
            0,
            data,
            false,
        )
    }

    fn actor(id: [u8; 32], signed: bool) -> AccountWithMetadata {
        account(id, DEFAULT_PROGRAM_ID, 0, Data::default(), signed)
    }

    fn exact_ata(owner: AccountId, definition: AccountId) -> AccountId {
        associated_token_account(actual_ata_program(), owner, definition)
    }

    fn metadata_from(output: &SpelOutput) -> EscrowMetadata {
        EscrowMetadata::try_from_slice(output.post_states[0].account().data.as_ref())
            .expect("metadata output must be valid")
    }

    fn committed_metadata(output: &SpelOutput) -> AccountWithMetadata {
        let mut metadata = metadata_account();
        metadata.account = output.post_states[0].account().clone();
        metadata.account.program_owner = ESCROW_PROGRAM;
        metadata
    }

    fn custom_token_initialize(definition: AccountId) -> SpelOutput {
        zec_escrow::initialize_token(
            context(),
            metadata_account(),
            actor([1; 32], true),
            actor([2; 32], false),
            token_definition_account(definition),
            SWAP_ID,
            [31; 32],
            Sha256::digest(PREIMAGE).into(),
            AMOUNT,
            REFUND_AT,
            actual_ata_program(),
        )
        .expect("valid custom-token initialize")
    }

    fn custom_token_fund(initialized: &SpelOutput, definition: AccountId) -> SpelOutput {
        let state = metadata_from(initialized);
        zec_escrow::fund_token(
            context(),
            committed_metadata(initialized),
            actor([1; 32], true),
            token_holding(state.depositor_asset.into_value(), definition, 500),
            token_holding(state.custody.into_value(), definition, 0),
            SWAP_ID,
        )
        .expect("signed token owner funds the exact custody ATA")
    }

    fn execute_ata_transfer(call: &ChainedCall, amount: u128) -> (u128, u128) {
        let (ata_posts, nested) = ata_program::transfer::transfer_from_associated_token_account(
            call.pre_states[0].clone(),
            call.pre_states[1].clone(),
            call.pre_states[2].clone(),
            actual_ata_program(),
            amount,
        );
        validate_execution(&call.pre_states, &ata_posts, call.program_id)
            .expect("official ATA outer transfer must satisfy LEZ validation");
        assert_eq!(nested.len(), 1);
        let token_call = &nested[0];
        let token_posts = token_program::transfer::transfer(
            token_call.pre_states[0].clone(),
            token_call.pre_states[1].clone(),
            amount,
        );
        validate_execution(&token_call.pre_states, &token_posts, token_call.program_id)
            .expect("ATA-delegated token transfer must satisfy LEZ validation");
        let balance =
            |index: usize| match TokenHolding::try_from(&token_posts[index].account().data)
                .expect("official token program must emit a holding")
            {
                TokenHolding::Fungible { balance, .. } => balance,
                _ => panic!("escrow fixture only accepts fungible custom tokens"),
            };
        (balance(0), balance(1))
    }

    fn native_initialize() -> SpelOutput {
        let native_program = actual_native_program();
        zec_escrow::initialize_native(
            context(),
            metadata_account(),
            empty_account(custody_id().into_value()),
            account([1; 32], native_program, 200, Data::default(), true),
            account([2; 32], native_program, 10, Data::default(), false),
            SWAP_ID,
            [31; 32],
            Sha256::digest(PREIMAGE).into(),
            AMOUNT,
            REFUND_AT,
            native_program,
        )
        .expect("valid native custody initialization")
    }

    fn native_fund(initialized: &SpelOutput) -> SpelOutput {
        let native_program = actual_native_program();
        zec_escrow::fund_native(
            context(),
            committed_metadata(initialized),
            account(
                custody_id().into_value(),
                native_program,
                0,
                Data::default(),
                false,
            ),
            account([1; 32], native_program, 200, Data::default(), true),
            SWAP_ID,
        )
        .expect("signed native depositor funds initialized custody")
    }

    #[test]
    fn generated_idl_has_the_contractual_zec_escrow_surface() {
        let idl = __program_idl();
        let instruction_names = idl
            .instructions
            .iter()
            .map(|instruction| instruction.name.as_str())
            .collect::<Vec<_>>();

        assert_eq!(idl.name, "zec_escrow");
        assert_eq!(
            instruction_names,
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
        assert!(idl
            .accounts
            .iter()
            .any(|account| account.name == "EscrowMetadata"));
    }

    #[test]
    fn idl_json_is_generated_by_spel_not_maintained_by_hand() {
        assert!(PROGRAM_IDL_JSON.contains("claim_token"));
        assert!(PROGRAM_IDL_JSON.contains("create_token_custody"));
        assert!(PROGRAM_IDL_JSON.contains("fund_native"));
        assert!(PROGRAM_IDL_JSON.contains("EscrowMetadata"));
        assert!(PROGRAM_IDL_JSON.contains("refund_at"));
    }

    #[test]
    fn native_lez_initializes_then_funds_through_authenticated_transfer() {
        let native_program = actual_native_program();
        let initialized = native_initialize();
        let state = metadata_from(&initialized);
        assert!(matches!(state.status, EscrowStatus::Empty));
        assert_eq!(state.depositor, AccountId::new([1; 32]));
        assert_eq!(state.claimant, AccountId::new([2; 32]));
        assert_eq!(state.custody, custody_id());
        assert_eq!(state.asset_program, native_program);
        assert_eq!(state.custody_program, native_program);
        assert_eq!(state.asset_definition, [0; 32]);

        let initialize_call = &initialized.chained_calls[0];
        assert_eq!(initialize_call.program_id, native_program);
        assert_eq!(initialize_call.pre_states.len(), 1);
        assert_eq!(initialize_call.pre_states[0].account_id, custody_id());
        assert!(initialize_call.pre_states[0].is_authorized);
        assert_eq!(
            initialize_call.instruction_data,
            ChainedCall::new(native_program, vec![], &0_u128).instruction_data
        );
        assert_eq!(initialize_call.pda_seeds, vec![custody_pda_seed(&SWAP_ID)]);

        let funded = native_fund(&initialized);
        assert!(matches!(
            metadata_from(&funded).status,
            EscrowStatus::Funded
        ));
        assert_eq!(funded.post_states[1].account().balance, 0);
        assert_eq!(funded.post_states[2].account().balance, 200);
        let funding_call = &funded.chained_calls[0];
        assert_eq!(funding_call.program_id, native_program);
        assert_eq!(
            funding_call.pre_states[0].account_id,
            AccountId::new([1; 32])
        );
        assert_eq!(funding_call.pre_states[1].account_id, custody_id());
        assert!(funding_call.pre_states[0].is_authorized);
        assert!(funding_call.pda_seeds.is_empty());
        assert_eq!(
            funding_call.instruction_data,
            ChainedCall::new(native_program, vec![], &AMOUNT).instruction_data
        );
    }

    #[test]
    fn custom_custody_is_exact_official_ata_and_executes_nested_token_calls() {
        let definition = AccountId::new([41; 32]);
        let ata_program = actual_ata_program();
        let token_program = actual_token_program();
        let expected_custody = exact_ata(metadata_account().account_id, definition);
        assert_ne!(expected_custody, custody_id());

        let initialized = custom_token_initialize(definition);
        let state = metadata_from(&initialized);
        assert_eq!(state.custody, expected_custody);
        assert_eq!(state.asset_program, token_program);
        assert_eq!(state.custody_program, ata_program);
        assert_eq!(
            state.depositor_asset,
            exact_ata(state.depositor, definition)
        );
        assert_eq!(state.claimant_asset, exact_ata(state.claimant, definition));
        assert!(initialized.chained_calls.is_empty());

        let created = zec_escrow::create_token_custody(
            context(),
            committed_metadata(&initialized),
            token_definition_account(definition),
            empty_account(expected_custody.into_value()),
            SWAP_ID,
        )
        .expect("any user can create the exact escrow custody ATA");
        let create_call = &created.chained_calls[0];
        assert_eq!(create_call.program_id, ata_program);
        assert!(create_call.pda_seeds.is_empty());
        let (ata_posts, nested) = ata_program::create::create_associated_token_account(
            create_call.pre_states[0].clone(),
            create_call.pre_states[1].clone(),
            create_call.pre_states[2].clone(),
            ata_program,
        );
        validate_execution(&create_call.pre_states, &ata_posts, create_call.program_id)
            .expect("official ATA create outer call must validate");
        assert_eq!(nested.len(), 1);
        assert_eq!(nested[0].program_id, token_program);
        let token_posts = token_program::initialize::initialize_account(
            nested[0].pre_states[0].clone(),
            nested[0].pre_states[1].clone(),
        );
        validate_execution(&nested[0].pre_states, &token_posts, nested[0].program_id)
            .expect("ATA-delegated token initialization must validate");
        assert_eq!(
            TokenHolding::try_from(&token_posts[1].account().data)
                .expect("custody becomes a token holding")
                .definition_id(),
            definition
        );

        let funded = custom_token_fund(&initialized, definition);
        assert_eq!(funded.chained_calls[0].program_id, ata_program);
        assert_eq!(
            execute_ata_transfer(&funded.chained_calls[0], AMOUNT),
            (425, AMOUNT)
        );
        assert!(matches!(
            metadata_from(&funded).status,
            EscrowStatus::Funded
        ));
    }

    #[test]
    fn two_token_definitions_have_independent_exact_custody_and_funding() {
        let mut custody_ids = Vec::new();
        for definition_bytes in [[41; 32], [42; 32]] {
            let definition = AccountId::new(definition_bytes);
            let initialized = custom_token_initialize(definition);
            let state = metadata_from(&initialized);
            assert_eq!(state.asset_definition, definition_bytes);
            assert_eq!(
                state.custody,
                exact_ata(metadata_account().account_id, definition)
            );
            let funded = custom_token_fund(&initialized, definition);
            assert_eq!(
                execute_ata_transfer(&funded.chained_calls[0], AMOUNT),
                (425, AMOUNT)
            );
            custody_ids.push(state.custody);
        }
        assert_ne!(custody_ids[0], custody_ids[1]);
    }

    #[test]
    fn token_initialization_and_custody_reject_definition_and_ata_substitution() {
        let definition = AccountId::new([41; 32]);
        let invalid_definition = account(
            definition.into_value(),
            actual_token_program(),
            0,
            Data::default(),
            false,
        );
        let invalid = zec_escrow::initialize_token(
            context(),
            metadata_account(),
            actor([1; 32], true),
            actor([2; 32], false),
            invalid_definition,
            SWAP_ID,
            [31; 32],
            Sha256::digest(PREIMAGE).into(),
            AMOUNT,
            REFUND_AT,
            actual_ata_program(),
        )
        .unwrap_err();
        assert_eq!(invalid.error_code(), 6003);

        let initialized = custom_token_initialize(definition);
        let substituted = zec_escrow::create_token_custody(
            context(),
            committed_metadata(&initialized),
            token_definition_account(definition),
            empty_account([99; 32]),
            SWAP_ID,
        )
        .unwrap_err();
        assert_eq!(substituted.error_code(), 6003);
    }

    #[test]
    fn token_claim_requires_real_claimant_and_delegates_metadata_then_ata() {
        let definition = AccountId::new([41; 32]);
        let initialized = custom_token_initialize(definition);
        let funded = custom_token_fund(&initialized, definition);
        let state = metadata_from(&funded);

        let wrong_preimage = zec_escrow::claim_token(
            context(),
            committed_metadata(&funded),
            token_holding(state.custody.into_value(), definition, AMOUNT),
            actor([2; 32], true),
            token_holding(state.claimant_asset.into_value(), definition, 0),
            SWAP_ID,
            [99; 32],
        )
        .unwrap_err();
        assert_eq!(wrong_preimage.error_code(), 6004);

        let mut unsupported_state = state.clone();
        unsupported_state.version = 2;
        let mut unsupported_metadata = committed_metadata(&funded);
        write_metadata(&mut unsupported_metadata, &unsupported_state)
            .expect("encode unsupported test version");
        let unsupported = zec_escrow::claim_token(
            context(),
            unsupported_metadata,
            token_holding(state.custody.into_value(), definition, AMOUNT),
            actor([2; 32], true),
            token_holding(state.claimant_asset.into_value(), definition, 0),
            SWAP_ID,
            PREIMAGE,
        )
        .unwrap_err();
        assert_eq!(unsupported.error_code(), 6005);

        let wrong_actor = zec_escrow::claim_token(
            context(),
            committed_metadata(&funded),
            token_holding(state.custody.into_value(), definition, AMOUNT),
            actor([3; 32], true),
            token_holding(state.claimant_asset.into_value(), definition, 0),
            SWAP_ID,
            PREIMAGE,
        )
        .unwrap_err();
        assert_eq!(wrong_actor.error_code(), 6003);

        let claimed = zec_escrow::claim_token(
            context(),
            committed_metadata(&funded),
            token_holding(state.custody.into_value(), definition, AMOUNT),
            actor([2; 32], true),
            token_holding(state.claimant_asset.into_value(), definition, 0),
            SWAP_ID,
            PREIMAGE,
        )
        .expect("real claimant claims to the fixed claimant ATA");
        assert!(matches!(
            metadata_from(&claimed).status,
            EscrowStatus::Claimed
        ));
        let call = &claimed.chained_calls[0];
        assert_eq!(call.program_id, actual_ata_program());
        assert_eq!(call.pda_seeds, vec![metadata_pda_seed(&SWAP_ID)]);
        assert!(call.pre_states[0].is_authorized);
        assert!(claimed
            .timestamp_validity_window
            .is_valid_for(REFUND_AT - 1));
        assert!(!claimed.timestamp_validity_window.is_valid_for(REFUND_AT));
        assert_eq!(execute_ata_transfer(call, AMOUNT), (0, AMOUNT));

        let replay = zec_escrow::claim_token(
            context(),
            committed_metadata(&claimed),
            token_holding(state.custody.into_value(), definition, AMOUNT),
            actor([2; 32], true),
            token_holding(state.claimant_asset.into_value(), definition, 0),
            SWAP_ID,
            PREIMAGE,
        )
        .unwrap_err();
        assert_eq!(replay.error_code(), 6002);
    }

    #[test]
    fn token_refund_is_permissionless_and_fixed_to_depositor_ata() {
        let definition = AccountId::new([41; 32]);
        let initialized = custom_token_initialize(definition);
        let funded = custom_token_fund(&initialized, definition);
        let state = metadata_from(&funded);
        let custody = token_holding(state.custody.into_value(), definition, AMOUNT);

        let wrong_destination = zec_escrow::refund_token(
            context(),
            committed_metadata(&funded),
            custody.clone(),
            token_holding([99; 32], definition, 0),
            SWAP_ID,
        )
        .unwrap_err();
        assert_eq!(wrong_destination.error_code(), 6003);

        let refunded = zec_escrow::refund_token(
            context(),
            committed_metadata(&funded),
            custody,
            token_holding(state.depositor_asset.into_value(), definition, 425),
            SWAP_ID,
        )
        .expect("any submitter can refund only to the immutable depositor ATA");
        assert!(matches!(
            metadata_from(&refunded).status,
            EscrowStatus::Refunded
        ));
        assert!(!refunded
            .timestamp_validity_window
            .is_valid_for(REFUND_AT - 1));
        assert!(refunded.timestamp_validity_window.is_valid_for(REFUND_AT));
        assert_eq!(
            execute_ata_transfer(&refunded.chained_calls[0], AMOUNT),
            (0, 500)
        );
    }

    #[test]
    fn native_lez_claim_and_refund_delegate_custody_at_disjoint_boundaries() {
        let native_program = actual_native_program();
        let initialized = native_initialize();
        let funded = native_fund(&initialized);
        let claim = zec_escrow::claim_native(
            context(),
            committed_metadata(&funded),
            account(
                custody_id().into_value(),
                native_program,
                AMOUNT,
                Data::default(),
                false,
            ),
            account([2; 32], native_program, 10, Data::default(), true),
            SWAP_ID,
            PREIMAGE,
        )
        .expect("native claim");
        assert!(matches!(
            metadata_from(&claim).status,
            EscrowStatus::Claimed
        ));
        assert_eq!(claim.chained_calls[0].program_id, native_program);
        assert!(claim.chained_calls[0].pre_states[0].is_authorized);
        assert_eq!(
            claim.chained_calls[0].pda_seeds,
            vec![custody_pda_seed(&SWAP_ID)]
        );
        assert!(claim.timestamp_validity_window.is_valid_for(REFUND_AT - 1));
        assert!(!claim.timestamp_validity_window.is_valid_for(REFUND_AT));

        let initialized = native_initialize();
        let funded = native_fund(&initialized);
        let refund = zec_escrow::refund_native(
            context(),
            committed_metadata(&funded),
            account(
                custody_id().into_value(),
                native_program,
                AMOUNT,
                Data::default(),
                false,
            ),
            account([1; 32], native_program, 125, Data::default(), true),
            SWAP_ID,
        )
        .expect("native refund");
        assert!(matches!(
            metadata_from(&refund).status,
            EscrowStatus::Refunded
        ));
        assert_eq!(refund.chained_calls[0].program_id, native_program);
        assert!(refund.chained_calls[0].pre_states[0].is_authorized);
        assert_eq!(
            refund.chained_calls[0].pda_seeds,
            vec![custody_pda_seed(&SWAP_ID)]
        );
        assert!(!refund.timestamp_validity_window.is_valid_for(REFUND_AT - 1));
        assert!(refund.timestamp_validity_window.is_valid_for(REFUND_AT));
    }

    #[test]
    fn native_lez_rejects_foreign_owned_actor() {
        let native_program = actual_native_program();
        let err = zec_escrow::initialize_native(
            context(),
            metadata_account(),
            empty_account(custody_id().into_value()),
            account([1; 32], [88; 8], 200, Data::default(), true),
            account([2; 32], native_program, 0, Data::default(), false),
            SWAP_ID,
            [31; 32],
            Sha256::digest(PREIMAGE).into(),
            AMOUNT,
            REFUND_AT,
            native_program,
        )
        .unwrap_err();
        assert_eq!(err.error_code(), 6003);
    }
}
