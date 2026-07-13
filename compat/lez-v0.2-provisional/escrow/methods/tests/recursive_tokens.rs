use borsh::BorshDeserialize as _;
use lez_zec_escrow_v02::{EscrowMetadata, EscrowStatus, Instruction as EscrowInstruction};
use lez_zec_escrow_v02_methods::{ZEC_ESCROW_V02_ELF, ZEC_ESCROW_V02_ID};
use nssa::{
    AccountId, PrivateKey, PublicKey, PublicTransaction, V03State,
    program::Program,
    public_transaction::{Message, WitnessSet},
};
use serde::Serialize;
use sha2::{Digest as _, Sha256};
use spel_framework_core::pda::compute_pda;
use token_core::TokenHolding;

const PREIMAGE: [u8; 32] = [12; 32];
const AMOUNT: u128 = 75;
const ACTOR_FUNDS: u128 = 500;
const REFUND_AT: u64 = 10_000;

fn actor(key_byte: u8) -> (AccountId, PrivateKey) {
    let key = PrivateKey::try_new([key_byte; 32]).expect("deterministic token actor key");
    let account_id = AccountId::from(&PublicKey::new_from_private_key(&key));
    (account_id, key)
}

fn transaction<T: Serialize>(
    state: &V03State,
    program_id: [u32; 8],
    account_ids: Vec<AccountId>,
    signers: &[(AccountId, &PrivateKey)],
    instruction: T,
) -> PublicTransaction {
    let nonces = signers
        .iter()
        .map(|(account_id, _)| state.get_account_by_id(*account_id).nonce)
        .collect();
    let message = Message::try_new(program_id, account_ids, nonces, instruction)
        .expect("serialize exact token fixture instruction");
    let keys = signers.iter().map(|(_, key)| *key).collect::<Vec<_>>();
    PublicTransaction::new(message.clone(), WitnessSet::for_message(&message, &keys))
}

fn ata(ata_program: [u32; 8], owner: AccountId, definition: AccountId) -> AccountId {
    ata_core::get_associated_token_account_id(
        &ata_program,
        &ata_core::compute_ata_seed(owner, definition),
    )
}

fn holding_balance(state: &V03State, account_id: AccountId, definition: AccountId) -> u128 {
    match TokenHolding::try_from(&state.get_account_by_id(account_id).data)
        .expect("official token program stores a holding")
    {
        TokenHolding::Fungible {
            definition_id,
            balance,
        } => {
            assert_eq!(definition_id, definition);
            balance
        }
        _ => panic!("escrow accepts only fungible token definitions"),
    }
}

fn escrow_metadata(state: &V03State, metadata: AccountId) -> EscrowMetadata {
    EscrowMetadata::try_from_slice(state.get_account_by_id(metadata).data.as_ref())
        .expect("state stores canonical token escrow metadata")
}

#[allow(clippy::too_many_arguments)]
fn create_funded_definition(
    state: &mut V03State,
    block: &mut u64,
    definition_key_byte: u8,
    supply_key_byte: u8,
    depositor: AccountId,
    depositor_key: &PrivateKey,
    claimant: AccountId,
    claimant_key: &PrivateKey,
) -> (AccountId, AccountId, AccountId) {
    let token_program = programs::token().id();
    let ata_program = programs::ata().id();
    let (definition, definition_key) = actor(definition_key_byte);
    let (supply, supply_key) = actor(supply_key_byte);
    let define = transaction(
        state,
        token_program,
        vec![definition, supply],
        &[(definition, &definition_key), (supply, &supply_key)],
        token_core::Instruction::NewFungibleDefinition {
            name: format!("M2 v0.2 definition {definition_key_byte}"),
            total_supply: 1_000,
        },
    );
    state
        .transition_from_public_transaction(&define, *block, 100)
        .expect("official token program creates the fungible definition");
    *block += 1;

    let depositor_ata = ata(ata_program, depositor, definition);
    let claimant_ata = ata(ata_program, claimant, definition);
    for (owner, key, owner_ata) in [
        (depositor, depositor_key, depositor_ata),
        (claimant, claimant_key, claimant_ata),
    ] {
        let create = transaction(
            state,
            ata_program,
            vec![owner, definition, owner_ata],
            &[(owner, key)],
            ata_core::Instruction::Create {
                ata_program_id: ata_program,
            },
        );
        state
            .transition_from_public_transaction(&create, *block, 100)
            .expect("real owner creates the exact official ATA");
        *block += 1;
    }

    let fund = transaction(
        state,
        token_program,
        vec![supply, depositor_ata],
        &[(supply, &supply_key)],
        token_core::Instruction::Transfer {
            amount_to_transfer: ACTOR_FUNDS,
        },
    );
    state
        .transition_from_public_transaction(&fund, *block, 100)
        .expect("supply owner funds the depositor's exact ATA");
    *block += 1;
    assert_eq!(
        holding_balance(state, depositor_ata, definition),
        ACTOR_FUNDS
    );
    assert_eq!(holding_balance(state, claimant_ata, definition), 0);
    (definition, depositor_ata, claimant_ata)
}

#[allow(clippy::too_many_arguments)]
fn initialize_and_fund_escrow(
    state: &mut V03State,
    block: &mut u64,
    swap_id: [u8; 32],
    definition: AccountId,
    depositor: AccountId,
    depositor_key: &PrivateKey,
    claimant: AccountId,
    depositor_ata: AccountId,
) -> (AccountId, AccountId) {
    let ata_program = programs::ata().id();
    let metadata = compute_pda(&ZEC_ESCROW_V02_ID, &[&swap_id]);
    let custody = ata(ata_program, metadata, definition);
    let initialize = transaction(
        state,
        ZEC_ESCROW_V02_ID,
        vec![metadata, depositor, claimant, definition],
        &[(depositor, depositor_key)],
        EscrowInstruction::InitializeToken {
            swap_id,
            terms_hash: [31; 32],
            secret_digest: Sha256::digest(PREIMAGE).into(),
            amount: AMOUNT,
            refund_at: REFUND_AT,
            ata_program,
        },
    );
    state
        .transition_from_public_transaction(&initialize, *block, 100)
        .expect("checked guest initializes exact token terms");
    *block += 1;
    let create = transaction(
        state,
        ZEC_ESCROW_V02_ID,
        vec![metadata, definition, custody],
        &[],
        EscrowInstruction::CreateTokenCustody { swap_id },
    );
    state
        .transition_from_public_transaction(&create, *block, 100)
        .expect("permissionless escrow call recursively creates exact custody ATA");
    *block += 1;
    let fund = transaction(
        state,
        ZEC_ESCROW_V02_ID,
        vec![metadata, depositor, depositor_ata, custody],
        &[(depositor, depositor_key)],
        EscrowInstruction::FundToken { swap_id },
    );
    state
        .transition_from_public_transaction(&fund, *block, 100)
        .expect("owner call recursively transfers tokens through ATA and Token programs");
    *block += 1;
    assert_eq!(
        escrow_metadata(state, metadata).status,
        EscrowStatus::Funded
    );
    assert_eq!(holding_balance(state, custody, definition), AMOUNT);
    assert_eq!(
        holding_balance(state, depositor_ata, definition),
        ACTOR_FUNDS - AMOUNT
    );
    (metadata, custody)
}

#[test]
fn checked_guest_executes_two_definition_claim_and_permissionless_refund_through_ata() {
    let escrow = Program::new(ZEC_ESCROW_V02_ELF.into()).expect("checked guest is canonical ELF");
    assert_eq!(escrow.id(), ZEC_ESCROW_V02_ID);
    let mut state = V03State::new().with_programs([escrow, programs::ata(), programs::token()]);
    let (depositor, depositor_key) = actor(1);
    let (claimant, claimant_key) = actor(2);
    let mut block = 1;

    let (claim_definition, claim_depositor_ata, claim_claimant_ata) = create_funded_definition(
        &mut state,
        &mut block,
        41,
        51,
        depositor,
        &depositor_key,
        claimant,
        &claimant_key,
    );
    let (claim_metadata, claim_custody) = initialize_and_fund_escrow(
        &mut state,
        &mut block,
        [71; 32],
        claim_definition,
        depositor,
        &depositor_key,
        claimant,
        claim_depositor_ata,
    );
    let claim = transaction(
        &state,
        ZEC_ESCROW_V02_ID,
        vec![claim_metadata, claim_custody, claimant, claim_claimant_ata],
        &[(claimant, &claimant_key)],
        EscrowInstruction::ClaimToken {
            swap_id: [71; 32],
            preimage: PREIMAGE,
        },
    );
    state
        .transition_from_public_transaction(&claim, block, REFUND_AT - 1)
        .expect("claimant recursively commits metadata, ATA, and Token state");
    block += 1;
    assert_eq!(
        escrow_metadata(&state, claim_metadata).status,
        EscrowStatus::Claimed
    );
    assert_eq!(holding_balance(&state, claim_custody, claim_definition), 0);
    assert_eq!(
        holding_balance(&state, claim_claimant_ata, claim_definition),
        AMOUNT
    );

    let (refund_definition, refund_depositor_ata, _) = create_funded_definition(
        &mut state,
        &mut block,
        42,
        52,
        depositor,
        &depositor_key,
        claimant,
        &claimant_key,
    );
    let (refund_metadata, refund_custody) = initialize_and_fund_escrow(
        &mut state,
        &mut block,
        [72; 32],
        refund_definition,
        depositor,
        &depositor_key,
        claimant,
        refund_depositor_ata,
    );
    assert_ne!(claim_custody, refund_custody);
    let refund = transaction(
        &state,
        ZEC_ESCROW_V02_ID,
        vec![refund_metadata, refund_custody, refund_depositor_ata],
        &[],
        EscrowInstruction::RefundToken { swap_id: [72; 32] },
    );
    state
        .transition_from_public_transaction(&refund, block, REFUND_AT)
        .expect("permissionless refund recursively commits only the fixed depositor ATA");
    assert_eq!(
        escrow_metadata(&state, refund_metadata).status,
        EscrowStatus::Refunded
    );
    assert_eq!(
        holding_balance(&state, refund_custody, refund_definition),
        0
    );
    assert_eq!(
        holding_balance(&state, refund_depositor_ata, refund_definition),
        ACTOR_FUNDS
    );
}
