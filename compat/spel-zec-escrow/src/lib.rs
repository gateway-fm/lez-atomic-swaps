//! Minimal executable SPEL/LEZ compatibility contract for the ZEC escrow.

#![allow(dead_code)]

use nssa_core::{
    account::{Account, AccountId, Data},
    program::{ChainedCall, Claim, PdaSeed, ProgramId, DEFAULT_PROGRAM_ID},
};
use sha2::{Digest, Sha256};
use spel_framework::prelude::*;
use token_core::{Instruction as TokenInstruction, TokenHolding};

#[derive(Debug, Clone, Copy, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum EscrowStatus {
    Empty,
    Funded,
    Claimed,
    Refunded,
}

#[account_type]
#[derive(BorshSerialize, BorshDeserialize)]
pub struct EscrowMetadata {
    pub version: u8,
    pub swap_id: [u8; 32],
    pub terms_hash: [u8; 32],
    pub secret_digest: [u8; 32],
    pub depositor: AccountId,
    pub claimant: AccountId,
    pub custody: AccountId,
    pub asset_program: ProgramId,
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

fn custody_pda_seed(swap_id: &[u8; 32]) -> PdaSeed {
    let label = spel_framework::pda::seed_from_str("custody");
    match AutoClaim::pda_from_seeds(&[&label, swap_id]) {
        AutoClaim::Claimed(Claim::Pda(seed)) => seed,
        _ => unreachable!("multi-seed public PDA always produces a PDA claim"),
    }
}

fn token_transfer(
    asset_program: ProgramId,
    mut sender: AccountWithMetadata,
    mut recipient: AccountWithMetadata,
    amount: u128,
    authorize_custody: bool,
    swap_id: &[u8; 32],
) -> ChainedCall {
    if authorize_custody {
        sender.is_authorized = true;
    } else {
        recipient.is_authorized = true;
    }
    ChainedCall::new(
        asset_program,
        vec![sender, recipient],
        &TokenInstruction::Transfer {
            amount_to_transfer: amount,
        },
    )
    .with_pda_seeds(vec![custody_pda_seed(swap_id)])
}

#[lez_program]
mod zec_escrow {
    #[allow(unused_imports)]
    use super::*;

    #[instruction]
    // The SPEL ABI keeps accounts and each signed swap term explicit in the IDL.
    #[allow(clippy::too_many_arguments)]
    pub fn initialize(
        ctx: ProgramContext,
        #[account(init, pda = arg("swap_id"))] metadata: AccountWithMetadata,
        #[account(mut, signer)] depositor: AccountWithMetadata,
        #[account(mut, pda = [literal("custody"), arg("swap_id")])] custody: AccountWithMetadata,
        claimant: AccountWithMetadata,
        swap_id: [u8; 32],
        terms_hash: [u8; 32],
        secret_digest: [u8; 32],
        amount: u128,
        refund_at: u64,
        asset_program: [u32; 8],
        asset_definition: [u8; 32],
    ) -> SpelResult {
        if amount == 0 || refund_at == 0 || secret_digest == [0; 32] {
            return Err(custom_error(ERROR_INVALID_TERMS, "invalid escrow terms"));
        }

        let is_native = asset_program == DEFAULT_PROGRAM_ID && asset_definition == [0; 32];
        let is_token = asset_program != DEFAULT_PROGRAM_ID && asset_definition != [0; 32];
        if !is_native && !is_token {
            return Err(custom_error(
                ERROR_INVALID_TERMS,
                "asset program and definition disagree",
            ));
        }

        let mut metadata = metadata;
        let mut depositor = depositor;
        let mut custody = custody;
        let mut calls = Vec::new();

        if is_native {
            if depositor.account.program_owner != ctx.self_program_id
                || custody.account.program_owner != ctx.self_program_id
                || claimant.account.program_owner != ctx.self_program_id
            {
                return Err(custom_error(
                    ERROR_ACCOUNT_BINDING,
                    "native LEZ accounts must be escrow-program owned",
                ));
            }
            depositor.account.balance = depositor.account.balance.checked_sub(amount).ok_or(
                SpelError::InsufficientBalance {
                    available: depositor.account.balance,
                    requested: amount,
                },
            )?;
            custody.account.balance =
                custody
                    .account
                    .balance
                    .checked_add(amount)
                    .ok_or_else(|| SpelError::Overflow {
                        operation: "native custody deposit".into(),
                    })?;
        } else {
            if depositor.account.program_owner != asset_program
                || claimant.account.program_owner != asset_program
                || token_definition(&depositor)?.into_value() != asset_definition
                || token_definition(&claimant)?.into_value() != asset_definition
                || custody.account != Account::default()
            {
                return Err(custom_error(
                    ERROR_ACCOUNT_BINDING,
                    "custom-token program, definition, or custody mismatch",
                ));
            }
            calls.push(token_transfer(
                asset_program,
                depositor.clone(),
                custody.clone(),
                amount,
                false,
                &swap_id,
            ));
        }

        let state = EscrowMetadata {
            version: 1,
            swap_id,
            terms_hash,
            secret_digest,
            depositor: depositor.account_id,
            claimant: claimant.account_id,
            custody: custody.account_id,
            asset_program,
            asset_definition,
            amount,
            refund_at,
            status: EscrowStatus::Funded,
        };
        write_metadata(&mut metadata, &state)?;

        Ok(SpelOutput::execute(
            vec![metadata, depositor, custody, claimant],
            calls,
        ))
    }

    #[instruction]
    pub fn claim_hashlock(
        ctx: ProgramContext,
        #[account(mut, owner = self_program_id, pda = arg("swap_id"))]
        metadata: AccountWithMetadata,
        #[account(mut, pda = [literal("custody"), arg("swap_id")])] custody: AccountWithMetadata,
        #[account(mut, signer)] claimant: AccountWithMetadata,
        swap_id: [u8; 32],
        preimage: [u8; 32],
    ) -> SpelResult {
        let mut metadata = metadata;
        let mut custody = custody;
        let mut claimant = claimant;
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
            || state.custody != custody.account_id
            || state.claimant != claimant.account_id
        {
            return Err(custom_error(
                ERROR_ACCOUNT_BINDING,
                "claim account binding mismatch",
            ));
        }
        let digest: [u8; 32] = Sha256::digest(preimage).into();
        if digest != state.secret_digest {
            return Err(custom_error(ERROR_WRONG_PREIMAGE, "wrong preimage"));
        }

        let mut calls = Vec::new();
        if state.asset_program == DEFAULT_PROGRAM_ID {
            if custody.account.program_owner != ctx.self_program_id
                || claimant.account.program_owner != ctx.self_program_id
            {
                return Err(custom_error(
                    ERROR_ACCOUNT_BINDING,
                    "native LEZ claim account owner mismatch",
                ));
            }
            custody.account.balance = custody.account.balance.checked_sub(state.amount).ok_or(
                SpelError::InsufficientBalance {
                    available: custody.account.balance,
                    requested: state.amount,
                },
            )?;
            claimant.account.balance = claimant
                .account
                .balance
                .checked_add(state.amount)
                .ok_or_else(|| SpelError::Overflow {
                    operation: "native claim".into(),
                })?;
        } else {
            if custody.account.program_owner != state.asset_program
                || claimant.account.program_owner != state.asset_program
                || token_definition(&custody)?.into_value() != state.asset_definition
                || token_definition(&claimant)?.into_value() != state.asset_definition
            {
                return Err(custom_error(
                    ERROR_ACCOUNT_BINDING,
                    "custom-token claim asset mismatch",
                ));
            }
            calls.push(token_transfer(
                state.asset_program,
                custody.clone(),
                claimant.clone(),
                state.amount,
                true,
                &swap_id,
            ));
        }

        state.status = EscrowStatus::Claimed;
        write_metadata(&mut metadata, &state)?;
        Ok(
            SpelOutput::execute(vec![metadata, custody, claimant], calls)
                .with_timestamp_validity_window(..state.refund_at),
        )
    }

    #[instruction]
    pub fn refund(
        ctx: ProgramContext,
        #[account(mut, owner = self_program_id, pda = arg("swap_id"))]
        metadata: AccountWithMetadata,
        #[account(mut, pda = [literal("custody"), arg("swap_id")])] custody: AccountWithMetadata,
        #[account(mut, signer)] depositor: AccountWithMetadata,
        swap_id: [u8; 32],
    ) -> SpelResult {
        let mut metadata = metadata;
        let mut custody = custody;
        let mut depositor = depositor;
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
            || state.custody != custody.account_id
            || state.depositor != depositor.account_id
        {
            return Err(custom_error(
                ERROR_ACCOUNT_BINDING,
                "refund account binding mismatch",
            ));
        }

        let mut calls = Vec::new();
        if state.asset_program == DEFAULT_PROGRAM_ID {
            if custody.account.program_owner != ctx.self_program_id
                || depositor.account.program_owner != ctx.self_program_id
            {
                return Err(custom_error(
                    ERROR_ACCOUNT_BINDING,
                    "native LEZ refund account owner mismatch",
                ));
            }
            custody.account.balance = custody.account.balance.checked_sub(state.amount).ok_or(
                SpelError::InsufficientBalance {
                    available: custody.account.balance,
                    requested: state.amount,
                },
            )?;
            depositor.account.balance = depositor
                .account
                .balance
                .checked_add(state.amount)
                .ok_or_else(|| SpelError::Overflow {
                    operation: "native refund".into(),
                })?;
        } else {
            if custody.account.program_owner != state.asset_program
                || depositor.account.program_owner != state.asset_program
                || token_definition(&custody)?.into_value() != state.asset_definition
                || token_definition(&depositor)?.into_value() != state.asset_definition
            {
                return Err(custom_error(
                    ERROR_ACCOUNT_BINDING,
                    "custom-token refund asset mismatch",
                ));
            }
            calls.push(token_transfer(
                state.asset_program,
                custody.clone(),
                depositor.clone(),
                state.amount,
                true,
                &swap_id,
            ));
        }

        state.status = EscrowStatus::Refunded;
        write_metadata(&mut metadata, &state)?;
        Ok(
            SpelOutput::execute(vec![metadata, custody, depositor], calls)
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
    use token_core::{Instruction as TokenInstruction, TokenHolding};

    const ESCROW_PROGRAM: ProgramId = [7; 8];
    const TOKEN_PROGRAM: ProgramId = [9; 8];
    const OTHER_TOKEN_PROGRAM: ProgramId = [10; 8];
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

    fn token_holding(id: [u8; 32], definition: AccountId, balance: u128) -> AccountWithMetadata {
        let holding = TokenHolding::Fungible {
            definition_id: definition,
            balance,
        };
        account(id, TOKEN_PROGRAM, 0, Data::from(&holding), true)
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

    fn custom_token_initialize(definition: AccountId, claimant: AccountWithMetadata) -> SpelOutput {
        zec_escrow::initialize(
            context(),
            metadata_account(),
            token_holding([21; 32], definition, 500),
            empty_account(custody_id().into_value()),
            claimant,
            SWAP_ID,
            [31; 32],
            Sha256::digest(PREIMAGE).into(),
            AMOUNT,
            REFUND_AT,
            TOKEN_PROGRAM,
            definition.into_value(),
        )
        .expect("valid custom-token initialize")
    }

    fn expected_token_transfer(
        sender: AccountWithMetadata,
        recipient: AccountWithMetadata,
        amount: u128,
    ) -> ChainedCall {
        ChainedCall::new(
            TOKEN_PROGRAM,
            vec![sender, recipient],
            &TokenInstruction::Transfer {
                amount_to_transfer: amount,
            },
        )
    }

    fn execute_token_transfer(call: &ChainedCall, amount: u128) -> (u128, u128) {
        let post_states = token_program::transfer::transfer(
            call.pre_states[0].clone(),
            call.pre_states[1].clone(),
            amount,
        );
        validate_execution(&call.pre_states, &post_states, call.program_id)
            .expect("official token transfer must satisfy LEZ execution validation");
        let balance =
            |index: usize| match TokenHolding::try_from(&post_states[index].account().data)
                .expect("official token program must emit a holding")
            {
                TokenHolding::Fungible { balance, .. } => balance,
                _ => panic!("escrow fixture only accepts fungible custom tokens"),
            };
        (balance(0), balance(1))
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
            ["initialize", "claim_hashlock", "refund"]
        );
        assert!(idl
            .accounts
            .iter()
            .any(|account| account.name == "EscrowMetadata"));
    }

    #[test]
    fn idl_json_is_generated_by_spel_not_maintained_by_hand() {
        assert!(PROGRAM_IDL_JSON.contains("claim_hashlock"));
        assert!(PROGRAM_IDL_JSON.contains("EscrowMetadata"));
        assert!(PROGRAM_IDL_JSON.contains("refund_at"));
    }

    #[test]
    fn native_lez_initialize_locks_value_and_binds_every_actor() {
        let depositor = account([1; 32], ESCROW_PROGRAM, 200, Data::default(), true);
        let custody = account(
            custody_id().into_value(),
            ESCROW_PROGRAM,
            0,
            Data::default(),
            false,
        );
        let claimant = account([2; 32], ESCROW_PROGRAM, 10, Data::default(), false);

        let pre_states = vec![
            metadata_account(),
            depositor.clone(),
            custody.clone(),
            claimant.clone(),
        ];
        let output = zec_escrow::initialize(
            context(),
            metadata_account(),
            depositor,
            custody,
            claimant,
            SWAP_ID,
            [31; 32],
            Sha256::digest(PREIMAGE).into(),
            AMOUNT,
            REFUND_AT,
            DEFAULT_PROGRAM_ID,
            [0; 32],
        )
        .expect("valid native LEZ initialize");

        validate_execution(&pre_states, &output.post_states, ESCROW_PROGRAM)
            .expect("native custody output must satisfy LEZ execution validation");

        assert_eq!(output.post_states[1].account().balance, 125);
        assert_eq!(output.post_states[2].account().balance, AMOUNT);
        assert!(output.chained_calls.is_empty());
        let metadata = metadata_from(&output);
        assert_eq!(metadata.depositor, AccountId::new([1; 32]));
        assert_eq!(metadata.claimant, AccountId::new([2; 32]));
        assert_eq!(metadata.custody, custody_id());
        assert_eq!(metadata.asset_program, DEFAULT_PROGRAM_ID);
        assert_eq!(metadata.asset_definition, [0; 32]);
        assert!(matches!(metadata.status, EscrowStatus::Funded));
    }

    #[test]
    fn native_lez_claim_and_refund_move_exact_value_at_disjoint_boundaries() {
        let initialize = || {
            zec_escrow::initialize(
                context(),
                metadata_account(),
                account([1; 32], ESCROW_PROGRAM, 200, Data::default(), true),
                account(
                    custody_id().into_value(),
                    ESCROW_PROGRAM,
                    0,
                    Data::default(),
                    false,
                ),
                account([2; 32], ESCROW_PROGRAM, 10, Data::default(), true),
                SWAP_ID,
                [31; 32],
                Sha256::digest(PREIMAGE).into(),
                AMOUNT,
                REFUND_AT,
                DEFAULT_PROGRAM_ID,
                [0; 32],
            )
            .expect("native initialize")
        };

        let initialized = initialize();
        let metadata = committed_metadata(&initialized);
        let claim = zec_escrow::claim_hashlock(
            context(),
            metadata,
            account(
                custody_id().into_value(),
                ESCROW_PROGRAM,
                AMOUNT,
                Data::default(),
                false,
            ),
            account([2; 32], ESCROW_PROGRAM, 10, Data::default(), true),
            SWAP_ID,
            PREIMAGE,
        )
        .expect("native claim");
        assert_eq!(claim.post_states[1].account().balance, 0);
        assert_eq!(claim.post_states[2].account().balance, 85);
        assert!(claim.timestamp_validity_window.is_valid_for(REFUND_AT - 1));
        assert!(!claim.timestamp_validity_window.is_valid_for(REFUND_AT));

        let initialized = initialize();
        let metadata = committed_metadata(&initialized);
        let refund = zec_escrow::refund(
            context(),
            metadata,
            account(
                custody_id().into_value(),
                ESCROW_PROGRAM,
                AMOUNT,
                Data::default(),
                false,
            ),
            account([1; 32], ESCROW_PROGRAM, 125, Data::default(), true),
            SWAP_ID,
        )
        .expect("native refund");
        assert_eq!(refund.post_states[1].account().balance, 0);
        assert_eq!(refund.post_states[2].account().balance, 200);
        assert!(!refund.timestamp_validity_window.is_valid_for(REFUND_AT - 1));
        assert!(refund.timestamp_validity_window.is_valid_for(REFUND_AT));
    }

    #[test]
    fn native_lez_initialize_rejects_foreign_owned_value_accounts() {
        let err = zec_escrow::initialize(
            context(),
            metadata_account(),
            account([1; 32], [88; 8], 200, Data::default(), true),
            account(
                custody_id().into_value(),
                ESCROW_PROGRAM,
                0,
                Data::default(),
                false,
            ),
            account([2; 32], ESCROW_PROGRAM, 0, Data::default(), false),
            SWAP_ID,
            [31; 32],
            Sha256::digest(PREIMAGE).into(),
            AMOUNT,
            REFUND_AT,
            DEFAULT_PROGRAM_ID,
            [0; 32],
        )
        .unwrap_err();
        assert_eq!(err.error_code(), 6003);
    }

    #[test]
    fn custom_token_initialize_uses_official_token_program_for_two_definitions() {
        for definition_bytes in [[41; 32], [42; 32]] {
            let definition = AccountId::new(definition_bytes);
            let claimant = token_holding([2; 32], definition, 0);
            let output = custom_token_initialize(definition, claimant);
            let call = &output.chained_calls[0];

            assert_eq!(call.program_id, TOKEN_PROGRAM);
            assert_eq!(call.pre_states[0].account_id, AccountId::new([21; 32]));
            assert_eq!(call.pre_states[1].account_id, custody_id());
            let expected = expected_token_transfer(
                call.pre_states[0].clone(),
                call.pre_states[1].clone(),
                AMOUNT,
            );
            assert_eq!(call.instruction_data, expected.instruction_data);
            assert_eq!(execute_token_transfer(call, AMOUNT), (425, AMOUNT));
            assert_eq!(metadata_from(&output).asset_definition, definition_bytes);
        }
    }

    #[test]
    fn custom_token_initialize_rejects_definition_and_program_substitution() {
        let definition_a = AccountId::new([41; 32]);
        let definition_b = AccountId::new([42; 32]);
        let claimant_b = token_holding([2; 32], definition_b, 0);
        let err = zec_escrow::initialize(
            context(),
            metadata_account(),
            token_holding([21; 32], definition_a, 500),
            empty_account(custody_id().into_value()),
            claimant_b,
            SWAP_ID,
            [31; 32],
            Sha256::digest(PREIMAGE).into(),
            AMOUNT,
            REFUND_AT,
            TOKEN_PROGRAM,
            definition_a.into_value(),
        )
        .unwrap_err();
        assert_eq!(err.error_code(), 6003);

        let mut foreign_program_depositor = token_holding([21; 32], definition_a, 500);
        foreign_program_depositor.account.program_owner = OTHER_TOKEN_PROGRAM;
        let err = zec_escrow::initialize(
            context(),
            metadata_account(),
            foreign_program_depositor,
            empty_account(custody_id().into_value()),
            token_holding([2; 32], definition_a, 0),
            SWAP_ID,
            [31; 32],
            Sha256::digest(PREIMAGE).into(),
            AMOUNT,
            REFUND_AT,
            TOKEN_PROGRAM,
            definition_a.into_value(),
        )
        .unwrap_err();
        assert_eq!(err.error_code(), 6003);
    }

    #[test]
    fn claim_rejects_wrong_preimage_actor_and_custody_substitution() {
        let definition = AccountId::new([41; 32]);
        let claimant = token_holding([2; 32], definition, 0);
        let initialized = custom_token_initialize(definition, claimant.clone());
        let metadata = committed_metadata(&initialized);
        let custody = token_holding(custody_id().into_value(), definition, AMOUNT);

        let wrong_preimage = zec_escrow::claim_hashlock(
            context(),
            metadata.clone(),
            custody.clone(),
            claimant.clone(),
            SWAP_ID,
            [99; 32],
        )
        .unwrap_err();
        assert_eq!(wrong_preimage.error_code(), 6004);

        let wrong_actor = zec_escrow::claim_hashlock(
            context(),
            metadata.clone(),
            custody.clone(),
            token_holding([3; 32], definition, 0),
            SWAP_ID,
            PREIMAGE,
        )
        .unwrap_err();
        assert_eq!(wrong_actor.error_code(), 6003);

        let mut substituted = custody;
        substituted.account_id = AccountId::new([88; 32]);
        let wrong_custody = zec_escrow::claim_hashlock(
            context(),
            metadata,
            substituted,
            claimant,
            SWAP_ID,
            PREIMAGE,
        )
        .unwrap_err();
        assert_eq!(wrong_custody.error_code(), 6003);
    }

    #[test]
    fn claim_is_before_refund_boundary_and_replay_is_rejected() {
        let definition = AccountId::new([41; 32]);
        let claimant = token_holding([2; 32], definition, 0);
        let initialized = custom_token_initialize(definition, claimant.clone());
        let metadata = committed_metadata(&initialized);
        let custody = token_holding(custody_id().into_value(), definition, AMOUNT);

        let output = zec_escrow::claim_hashlock(
            context(),
            metadata.clone(),
            custody.clone(),
            claimant.clone(),
            SWAP_ID,
            PREIMAGE,
        )
        .expect("claim before refund boundary");
        assert!(output.timestamp_validity_window.is_valid_for(REFUND_AT - 1));
        assert!(!output.timestamp_validity_window.is_valid_for(REFUND_AT));
        assert!(matches!(
            metadata_from(&output).status,
            EscrowStatus::Claimed
        ));
        assert_eq!(
            execute_token_transfer(&output.chained_calls[0], AMOUNT),
            (0, AMOUNT)
        );

        let mut claimed_metadata = metadata;
        claimed_metadata.account = output.post_states[0].account().clone();
        let replay = zec_escrow::claim_hashlock(
            context(),
            claimed_metadata,
            custody,
            claimant,
            SWAP_ID,
            PREIMAGE,
        )
        .unwrap_err();
        assert_eq!(replay.error_code(), 6002);
    }

    #[test]
    fn refund_is_at_or_after_boundary_and_only_original_depositor_can_receive() {
        let definition = AccountId::new([41; 32]);
        let claimant = token_holding([2; 32], definition, 0);
        let initialized = custom_token_initialize(definition, claimant);
        let metadata = committed_metadata(&initialized);
        let custody = token_holding(custody_id().into_value(), definition, AMOUNT);
        let depositor = token_holding([21; 32], definition, 425);

        let wrong_depositor = zec_escrow::refund(
            context(),
            metadata.clone(),
            custody.clone(),
            token_holding([22; 32], definition, 0),
            SWAP_ID,
        )
        .unwrap_err();
        assert_eq!(wrong_depositor.error_code(), 6003);

        let output = zec_escrow::refund(context(), metadata, custody, depositor, SWAP_ID)
            .expect("refund at boundary");
        assert!(!output.timestamp_validity_window.is_valid_for(REFUND_AT - 1));
        assert!(output.timestamp_validity_window.is_valid_for(REFUND_AT));
        assert!(matches!(
            metadata_from(&output).status,
            EscrowStatus::Refunded
        ));
        assert_eq!(
            execute_token_transfer(&output.chained_calls[0], AMOUNT),
            (0, 500)
        );

        let refunded_metadata = committed_metadata(&output);
        let replay = zec_escrow::refund(
            context(),
            refunded_metadata,
            token_holding(custody_id().into_value(), definition, AMOUNT),
            token_holding([21; 32], definition, 425),
            SWAP_ID,
        )
        .unwrap_err();
        assert_eq!(replay.error_code(), 6002);
    }

    #[test]
    fn unsupported_metadata_version_is_rejected_before_release() {
        let definition = AccountId::new([41; 32]);
        let claimant = token_holding([2; 32], definition, 0);
        let initialized = custom_token_initialize(definition, claimant.clone());
        let mut state = metadata_from(&initialized);
        state.version = 2;
        let mut metadata = committed_metadata(&initialized);
        write_metadata(&mut metadata, &state).expect("encode unsupported test version");

        let err = zec_escrow::claim_hashlock(
            context(),
            metadata,
            token_holding(custody_id().into_value(), definition, AMOUNT),
            claimant,
            SWAP_ID,
            PREIMAGE,
        )
        .unwrap_err();
        assert_eq!(err.error_code(), 6005);
    }
}
