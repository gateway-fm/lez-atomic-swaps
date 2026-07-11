use std::path::PathBuf;

use anyhow::{Context as _, Result, ensure};
use borsh::BorshDeserialize as _;
use lez_zec_escrow_compat::{EscrowMetadata, EscrowStatus, Instruction as EscrowInstruction};
use nssa::{
    AccountId, PrivateKey, ProgramDeploymentTransaction, PublicTransaction, V03State,
    program::Program,
    public_transaction::{Message, WitnessSet},
};
use serde::Serialize;
use sha2::{Digest as _, Sha256};
use spel_framework_core::pda::{compute_pda, seed_from_str};

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

#[test]
#[ignore = "requires exact r0vm and the separately built Risc0 guest ELF"]
fn records_recursive_native_instruction_costs_without_clock_noise() -> Result<()> {
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

    Ok(())
}
