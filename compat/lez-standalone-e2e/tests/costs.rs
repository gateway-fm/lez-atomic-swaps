use std::path::PathBuf;

use anyhow::{Context as _, Result, ensure};
use borsh::BorshDeserialize as _;
use lez_zec_escrow_compat::{EscrowMetadata, EscrowStatus, Instruction as EscrowInstruction};
use nssa::{
    AccountId, PrivateKey, ProgramDeploymentTransaction, PublicKey, PublicTransaction, V03State,
    program::Program,
    public_transaction::{Message, WitnessSet},
};
use serde::Serialize;
use sha2::{Digest as _, Sha256};
use spel_framework_core::pda::{compute_pda, seed_from_str};
use token_core::{TokenDefinition, TokenHolding};

fn public_transaction<T: Serialize>(
    state: &V03State,
    program_id: [u32; 8],
    account_ids: Vec<AccountId>,
    signers: &[(AccountId, &PrivateKey)],
    instruction: T,
) -> Result<PublicTransaction> {
    let nonces = signers
        .iter()
        .map(|(id, _)| state.get_account_by_id(*id).nonce)
        .collect();
    let message = Message::try_new(program_id, account_ids, nonces, instruction)
        .context("serialize cost transaction")?;
    let keys = signers.iter().map(|(_, key)| *key).collect::<Vec<_>>();
    Ok(PublicTransaction::new(
        message.clone(),
        WitnessSet::for_message(&message, &keys),
    ))
}

fn transition_costed(
    state: &mut V03State,
    operation: &str,
    transaction: &PublicTransaction,
    block_id: u64,
    timestamp: u64,
) -> Result<()> {
    eprintln!("LEZ_COST_BEGIN {operation}");
    let result = state.transition_from_public_transaction(transaction, block_id, timestamp);
    eprintln!("LEZ_COST_END {operation}");
    result.with_context(|| format!("execute cost operation {operation}"))
}

fn native_ids(program_id: [u32; 8], swap_id: &[u8; 32]) -> (AccountId, AccountId) {
    let metadata = compute_pda(&program_id, &[swap_id]);
    let custody_label = seed_from_str("custody");
    let custody = compute_pda(&program_id, &[&custody_label, swap_id]);
    (metadata, custody)
}

fn escrow_state(state: &V03State, metadata: AccountId) -> Result<EscrowMetadata> {
    let account = state.get_account_by_id(metadata);
    EscrowMetadata::try_from_slice(account.data.as_ref()).context("decode cost escrow metadata")
}

#[derive(Debug, Clone, Copy)]
struct TokenFixture {
    definition: AccountId,
    depositor_ata: AccountId,
    claimant_ata: AccountId,
    amount: u128,
}

fn keyed_account(byte: u8) -> Result<(AccountId, PrivateKey)> {
    let key = PrivateKey::try_new([byte; 32]).context("construct deterministic cost token key")?;
    let id = AccountId::from(&PublicKey::new_from_private_key(&key));
    Ok((id, key))
}

fn associated_token_account(
    ata_program: [u32; 8],
    owner: AccountId,
    definition: AccountId,
) -> AccountId {
    ata_core::get_associated_token_account_id(
        &ata_program,
        &ata_core::compute_ata_seed(owner, definition),
    )
}

fn fungible_holding(
    state: &V03State,
    token_program: [u32; 8],
    holding: AccountId,
) -> Result<(AccountId, u128)> {
    let account = state.get_account_by_id(holding);
    ensure!(
        account.program_owner == token_program,
        "cost holding must be token-program owned"
    );
    match TokenHolding::try_from(&account.data).context("decode cost token holding")? {
        TokenHolding::Fungible {
            definition_id,
            balance,
        } => Ok((definition_id, balance)),
        _ => anyhow::bail!("expected fungible cost holding"),
    }
}

#[allow(clippy::too_many_arguments)]
fn setup_token_fixture(
    state: &mut V03State,
    depositor: AccountId,
    depositor_key: &PrivateKey,
    claimant: AccountId,
    claimant_key: &PrivateKey,
    definition_key_byte: u8,
    supply_key_byte: u8,
    amount: u128,
    first_block_id: u64,
) -> Result<TokenFixture> {
    let token_program = Program::token().id();
    let ata_program = Program::ata().id();
    let (definition, definition_key) = keyed_account(definition_key_byte)?;
    let (supply, supply_key) = keyed_account(supply_key_byte)?;
    let total_supply = 10_000_u128;

    let definition_tx = public_transaction(
        state,
        token_program,
        vec![definition, supply],
        &[(definition, &definition_key), (supply, &supply_key)],
        token_core::Instruction::NewFungibleDefinition {
            name: format!("cost token {definition_key_byte}"),
            total_supply,
        },
    )?;
    state
        .transition_from_public_transaction(&definition_tx, first_block_id, 15_000)
        .context("create cost token definition")?;
    ensure!(
        matches!(
            TokenDefinition::try_from(&state.get_account_by_id(definition).data),
            Ok(TokenDefinition::Fungible {
                total_supply: actual,
                ..
            }) if actual == total_supply
        ),
        "cost definition must be the exact fungible supply"
    );

    let depositor_ata = associated_token_account(ata_program, depositor, definition);
    let claimant_ata = associated_token_account(ata_program, claimant, definition);
    for (offset, owner, key, ata) in [
        (1_u64, depositor, depositor_key, depositor_ata),
        (2_u64, claimant, claimant_key, claimant_ata),
    ] {
        let create = public_transaction(
            state,
            ata_program,
            vec![owner, definition, ata],
            &[(owner, key)],
            ata_core::Instruction::Create {
                ata_program_id: ata_program,
            },
        )?;
        state
            .transition_from_public_transaction(&create, first_block_id + offset, 15_000 + offset)
            .context("create cost actor ATA")?;
    }

    let fund = public_transaction(
        state,
        token_program,
        vec![supply, depositor_ata],
        &[(supply, &supply_key)],
        token_core::Instruction::Transfer {
            amount_to_transfer: amount,
        },
    )?;
    state
        .transition_from_public_transaction(&fund, first_block_id + 3, 15_003)
        .context("fund cost depositor ATA")?;
    ensure!(
        fungible_holding(state, token_program, depositor_ata)? == (definition, amount)
            && fungible_holding(state, token_program, claimant_ata)? == (definition, 0),
        "cost fixture must fund only the exact depositor ATA"
    );

    Ok(TokenFixture {
        definition,
        depositor_ata,
        claimant_ata,
        amount,
    })
}

#[test]
#[ignore = "requires exact r0vm and the separately built Risc0 guest ELF"]
fn records_recursive_escrow_instruction_costs_without_clock_noise() -> Result<()> {
    let elf_path = std::env::var_os("LEZ_ESCROW_GUEST_ELF")
        .map(PathBuf::from)
        .context("LEZ_ESCROW_GUEST_ELF must name the checked guest ELF")?;
    let elf = std::fs::read(&elf_path)
        .with_context(|| format!("read guest ELF at {}", elf_path.display()))?;
    ensure!(!elf.is_empty(), "guest ELF must not be empty");
    let program = Program::new(elf.clone()).context("guest must be a canonical LEZ program")?;

    let mut state = testnet_initial_state::initial_state();
    let deployment =
        ProgramDeploymentTransaction::new(nssa::program_deployment_transaction::Message::new(elf));
    state
        .transition_from_program_deployment_transaction(&deployment)
        .context("deploy checked guest into production in-memory state")?;

    let actors = testnet_initial_state::initial_pub_accounts_private_keys();
    let depositor = &actors[0];
    let claimant = &actors[1];
    let native_program = Program::authenticated_transfer_program().id();
    for actor in [depositor, claimant] {
        let account = state.get_account_by_id(actor.account_id);
        ensure!(
            account.program_owner == native_program && account.balance > 0,
            "cost actor must be a funded authenticated-transfer genesis account"
        );
    }

    let preimage = [42; 32];
    let secret_digest: [u8; 32] = Sha256::digest(preimage).into();
    let claim_swap_id = [71; 32];
    let claim_amount = 700_u128;
    let (claim_metadata, claim_custody) = native_ids(program.id(), &claim_swap_id);

    let initialize_claim = public_transaction(
        &state,
        program.id(),
        vec![
            claim_metadata,
            claim_custody,
            depositor.account_id,
            claimant.account_id,
        ],
        &[(depositor.account_id, &depositor.pub_sign_key)],
        EscrowInstruction::InitializeNative {
            swap_id: claim_swap_id,
            terms_hash: [81; 32],
            secret_digest,
            amount: claim_amount,
            refund_at: 10_000,
            authenticated_transfer_program: native_program,
        },
    )?;
    transition_costed(
        &mut state,
        "initialize_native_claim",
        &initialize_claim,
        1,
        1_000,
    )?;

    let fund_claim = public_transaction(
        &state,
        program.id(),
        vec![claim_metadata, claim_custody, depositor.account_id],
        &[(depositor.account_id, &depositor.pub_sign_key)],
        EscrowInstruction::FundNative {
            swap_id: claim_swap_id,
        },
    )?;
    transition_costed(&mut state, "fund_native_claim", &fund_claim, 2, 1_001)?;

    let claim = public_transaction(
        &state,
        program.id(),
        vec![claim_metadata, claim_custody, claimant.account_id],
        &[(claimant.account_id, &claimant.pub_sign_key)],
        EscrowInstruction::ClaimNative {
            swap_id: claim_swap_id,
            preimage,
        },
    )?;
    transition_costed(&mut state, "claim_native", &claim, 3, 1_002)?;
    ensure!(
        escrow_state(&state, claim_metadata)?.status == EscrowStatus::Claimed,
        "cost claim scenario must reach claimed state"
    );

    let refund_swap_id = [72; 32];
    let refund_amount = 900_u128;
    let (refund_metadata, refund_custody) = native_ids(program.id(), &refund_swap_id);
    let initialize_refund = public_transaction(
        &state,
        program.id(),
        vec![
            refund_metadata,
            refund_custody,
            depositor.account_id,
            claimant.account_id,
        ],
        &[(depositor.account_id, &depositor.pub_sign_key)],
        EscrowInstruction::InitializeNative {
            swap_id: refund_swap_id,
            terms_hash: [82; 32],
            secret_digest,
            amount: refund_amount,
            refund_at: 12_000,
            authenticated_transfer_program: native_program,
        },
    )?;
    transition_costed(
        &mut state,
        "initialize_native_refund",
        &initialize_refund,
        4,
        11_000,
    )?;

    let fund_refund = public_transaction(
        &state,
        program.id(),
        vec![refund_metadata, refund_custody, depositor.account_id],
        &[(depositor.account_id, &depositor.pub_sign_key)],
        EscrowInstruction::FundNative {
            swap_id: refund_swap_id,
        },
    )?;
    transition_costed(&mut state, "fund_native_refund", &fund_refund, 5, 11_001)?;

    let refund = public_transaction(
        &state,
        program.id(),
        vec![refund_metadata, refund_custody, depositor.account_id],
        &[],
        EscrowInstruction::RefundNative {
            swap_id: refund_swap_id,
        },
    )?;
    transition_costed(&mut state, "refund_native", &refund, 6, 12_000)?;
    ensure!(
        escrow_state(&state, refund_metadata)?.status == EscrowStatus::Refunded,
        "cost refund scenario must reach refunded state"
    );

    let token_program = Program::token().id();
    let ata_program = Program::ata().id();
    let claim_token_fixture = setup_token_fixture(
        &mut state,
        depositor.account_id,
        &depositor.pub_sign_key,
        claimant.account_id,
        &claimant.pub_sign_key,
        31,
        32,
        1_200,
        20,
    )?;
    let refund_token_fixture = setup_token_fixture(
        &mut state,
        depositor.account_id,
        &depositor.pub_sign_key,
        claimant.account_id,
        &claimant.pub_sign_key,
        33,
        34,
        1_400,
        24,
    )?;

    let token_claim_swap_id = [73; 32];
    let token_claim_metadata = compute_pda(&program.id(), &[&token_claim_swap_id]);
    let token_claim_custody = associated_token_account(
        ata_program,
        token_claim_metadata,
        claim_token_fixture.definition,
    );
    let initialize_token_claim = public_transaction(
        &state,
        program.id(),
        vec![
            token_claim_metadata,
            depositor.account_id,
            claimant.account_id,
            claim_token_fixture.definition,
        ],
        &[(depositor.account_id, &depositor.pub_sign_key)],
        EscrowInstruction::InitializeToken {
            swap_id: token_claim_swap_id,
            terms_hash: [83; 32],
            secret_digest,
            amount: claim_token_fixture.amount,
            refund_at: 30_000,
            ata_program,
        },
    )?;
    transition_costed(
        &mut state,
        "initialize_token_claim",
        &initialize_token_claim,
        30,
        20_000,
    )?;

    let create_token_claim_custody = public_transaction(
        &state,
        program.id(),
        vec![
            token_claim_metadata,
            claim_token_fixture.definition,
            token_claim_custody,
        ],
        &[],
        EscrowInstruction::CreateTokenCustody {
            swap_id: token_claim_swap_id,
        },
    )?;
    transition_costed(
        &mut state,
        "create_token_custody_claim",
        &create_token_claim_custody,
        31,
        20_001,
    )?;

    let fund_token_claim = public_transaction(
        &state,
        program.id(),
        vec![
            token_claim_metadata,
            depositor.account_id,
            claim_token_fixture.depositor_ata,
            token_claim_custody,
        ],
        &[(depositor.account_id, &depositor.pub_sign_key)],
        EscrowInstruction::FundToken {
            swap_id: token_claim_swap_id,
        },
    )?;
    transition_costed(
        &mut state,
        "fund_token_claim",
        &fund_token_claim,
        32,
        20_002,
    )?;

    let claim_token = public_transaction(
        &state,
        program.id(),
        vec![
            token_claim_metadata,
            token_claim_custody,
            claimant.account_id,
            claim_token_fixture.claimant_ata,
        ],
        &[(claimant.account_id, &claimant.pub_sign_key)],
        EscrowInstruction::ClaimToken {
            swap_id: token_claim_swap_id,
            preimage,
        },
    )?;
    transition_costed(&mut state, "claim_token", &claim_token, 33, 20_003)?;
    ensure!(
        escrow_state(&state, token_claim_metadata)?.status == EscrowStatus::Claimed
            && fungible_holding(&state, token_program, claim_token_fixture.claimant_ata)?
                == (claim_token_fixture.definition, claim_token_fixture.amount),
        "cost token claim must reach the correct definition-bound claimant ATA"
    );

    let token_refund_swap_id = [74; 32];
    let token_refund_metadata = compute_pda(&program.id(), &[&token_refund_swap_id]);
    let token_refund_custody = associated_token_account(
        ata_program,
        token_refund_metadata,
        refund_token_fixture.definition,
    );
    let initialize_token_refund = public_transaction(
        &state,
        program.id(),
        vec![
            token_refund_metadata,
            depositor.account_id,
            claimant.account_id,
            refund_token_fixture.definition,
        ],
        &[(depositor.account_id, &depositor.pub_sign_key)],
        EscrowInstruction::InitializeToken {
            swap_id: token_refund_swap_id,
            terms_hash: [84; 32],
            secret_digest,
            amount: refund_token_fixture.amount,
            refund_at: 40_000,
            ata_program,
        },
    )?;
    transition_costed(
        &mut state,
        "initialize_token_refund",
        &initialize_token_refund,
        34,
        31_000,
    )?;

    let create_token_refund_custody = public_transaction(
        &state,
        program.id(),
        vec![
            token_refund_metadata,
            refund_token_fixture.definition,
            token_refund_custody,
        ],
        &[],
        EscrowInstruction::CreateTokenCustody {
            swap_id: token_refund_swap_id,
        },
    )?;
    transition_costed(
        &mut state,
        "create_token_custody_refund",
        &create_token_refund_custody,
        35,
        31_001,
    )?;

    let fund_token_refund = public_transaction(
        &state,
        program.id(),
        vec![
            token_refund_metadata,
            depositor.account_id,
            refund_token_fixture.depositor_ata,
            token_refund_custody,
        ],
        &[(depositor.account_id, &depositor.pub_sign_key)],
        EscrowInstruction::FundToken {
            swap_id: token_refund_swap_id,
        },
    )?;
    transition_costed(
        &mut state,
        "fund_token_refund",
        &fund_token_refund,
        36,
        31_002,
    )?;

    let refund_token = public_transaction(
        &state,
        program.id(),
        vec![
            token_refund_metadata,
            token_refund_custody,
            refund_token_fixture.depositor_ata,
        ],
        &[],
        EscrowInstruction::RefundToken {
            swap_id: token_refund_swap_id,
        },
    )?;
    transition_costed(&mut state, "refund_token", &refund_token, 37, 40_000)?;
    ensure!(
        escrow_state(&state, token_refund_metadata)?.status == EscrowStatus::Refunded
            && fungible_holding(&state, token_program, refund_token_fixture.depositor_ata)?
                == (refund_token_fixture.definition, refund_token_fixture.amount),
        "cost token refund must restore the correct definition-bound depositor ATA"
    );

    Ok(())
}
