use borsh::BorshDeserialize as _;
use lez_zec_escrow_v02::{EscrowMetadata, EscrowStatus, Instruction as EscrowInstruction};
use lez_zec_escrow_v02_methods::{ZEC_ESCROW_V02_ELF, ZEC_ESCROW_V02_ID};
use nssa::{
    Account, AccountId, PrivateKey, PublicKey, PublicTransaction, V03State,
    program::Program,
    public_transaction::{Message, WitnessSet},
};
use serde::Serialize;
use sha2::{Digest as _, Sha256};
use spel_framework_core::pda::{compute_pda, seed_from_str};

const SWAP_ID: [u8; 32] = [11; 32];
const PREIMAGE: [u8; 32] = [12; 32];
const AMOUNT: u128 = 75;
const REFUND_AT: u64 = 1_000;

fn actor(key_byte: u8) -> (AccountId, PrivateKey) {
    let key = PrivateKey::try_new([key_byte; 32]).expect("deterministic test private key");
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
        .expect("serialize exact escrow instruction");
    let keys = signers.iter().map(|(_, key)| *key).collect::<Vec<_>>();
    PublicTransaction::new(message.clone(), WitnessSet::for_message(&message, &keys))
}

fn escrow_ids(program_id: [u32; 8]) -> (AccountId, AccountId) {
    let metadata = compute_pda(&program_id, &[&SWAP_ID]);
    let label = seed_from_str("custody");
    let custody = compute_pda(&program_id, &[&label, &SWAP_ID]);
    (metadata, custody)
}

fn metadata(state: &V03State, account_id: AccountId) -> EscrowMetadata {
    EscrowMetadata::try_from_slice(state.get_account_by_id(account_id).data.as_ref())
        .expect("state stores canonical escrow metadata")
}

fn funded_state(claimant_balance: u128) -> (V03State, AccountId, AccountId, AccountId, PrivateKey) {
    let program = Program::new(ZEC_ESCROW_V02_ELF.into()).expect("checked guest is canonical ELF");
    assert_eq!(program.id(), ZEC_ESCROW_V02_ID);
    let authenticated_transfer = programs::authenticated_transfer();
    let authenticated_transfer_id = authenticated_transfer.id();
    let (depositor, depositor_key) = actor(1);
    let (claimant, claimant_key) = actor(2);
    let initial_accounts = [
        (
            depositor,
            Account {
                program_owner: authenticated_transfer_id,
                balance: 200,
                ..Account::default()
            },
        ),
        (
            claimant,
            Account {
                program_owner: authenticated_transfer_id,
                balance: claimant_balance,
                ..Account::default()
            },
        ),
    ];
    let mut state = V03State::new()
        .with_public_accounts(initial_accounts)
        .with_programs([program, authenticated_transfer]);
    let (metadata_id, custody) = escrow_ids(ZEC_ESCROW_V02_ID);

    let initialize = transaction(
        &state,
        ZEC_ESCROW_V02_ID,
        vec![metadata_id, custody, depositor, claimant],
        &[(depositor, &depositor_key)],
        EscrowInstruction::InitializeNative {
            swap_id: SWAP_ID,
            terms_hash: [31; 32],
            secret_digest: Sha256::digest(PREIMAGE).into(),
            amount: AMOUNT,
            refund_at: REFUND_AT,
            authenticated_transfer_program: authenticated_transfer_id,
        },
    );
    state
        .transition_from_public_transaction(&initialize, 1, 100)
        .expect("checked guest recursively initializes authenticated custody");
    let fund = transaction(
        &state,
        ZEC_ESCROW_V02_ID,
        vec![metadata_id, custody, depositor],
        &[(depositor, &depositor_key)],
        EscrowInstruction::FundNative { swap_id: SWAP_ID },
    );
    state
        .transition_from_public_transaction(&fund, 2, 101)
        .expect("checked guest recursively funds custody");
    assert_eq!(metadata(&state, metadata_id).status, EscrowStatus::Funded);
    assert_eq!(state.get_account_by_id(custody).balance, AMOUNT);
    assert_eq!(state.get_account_by_id(depositor).balance, 200 - AMOUNT);

    (state, metadata_id, custody, claimant, claimant_key)
}

#[test]
fn checked_guest_executes_native_claim_and_permissionless_refund_recursively() {
    let (mut claim_state, claim_metadata, claim_custody, claimant, claimant_key) = funded_state(10);
    let claim = transaction(
        &claim_state,
        ZEC_ESCROW_V02_ID,
        vec![claim_metadata, claim_custody, claimant],
        &[(claimant, &claimant_key)],
        EscrowInstruction::ClaimNative {
            swap_id: SWAP_ID,
            preimage: PREIMAGE,
        },
    );
    claim_state
        .transition_from_public_transaction(&claim, 3, REFUND_AT - 1)
        .expect("valid preimage recursively commits metadata and custody transfer");
    assert_eq!(
        metadata(&claim_state, claim_metadata).status,
        EscrowStatus::Claimed
    );
    assert_eq!(claim_state.get_account_by_id(claim_custody).balance, 0);
    assert_eq!(claim_state.get_account_by_id(claimant).balance, 10 + AMOUNT);

    let (mut refund_state, refund_metadata, refund_custody, _, _) = funded_state(10);
    let depositor = actor(1).0;
    let refund = transaction(
        &refund_state,
        ZEC_ESCROW_V02_ID,
        vec![refund_metadata, refund_custody, depositor],
        &[],
        EscrowInstruction::RefundNative { swap_id: SWAP_ID },
    );
    refund_state
        .transition_from_public_transaction(&refund, 3, REFUND_AT)
        .expect("fixed-destination refund is permissionless at the boundary");
    assert_eq!(
        metadata(&refund_state, refund_metadata).status,
        EscrowStatus::Refunded
    );
    assert_eq!(refund_state.get_account_by_id(refund_custody).balance, 0);
    assert_eq!(refund_state.get_account_by_id(depositor).balance, 200);
}

#[test]
fn child_transfer_failure_rolls_back_terminal_metadata_and_every_account() {
    let (mut state, metadata_id, custody_id, claimant, claimant_key) = funded_state(u128::MAX);
    let before_metadata = state.get_account_by_id(metadata_id);
    let before_custody = state.get_account_by_id(custody_id);
    let before_claimant = state.get_account_by_id(claimant);
    assert_eq!(metadata(&state, metadata_id).status, EscrowStatus::Funded);

    let claim = transaction(
        &state,
        ZEC_ESCROW_V02_ID,
        vec![metadata_id, custody_id, claimant],
        &[(claimant, &claimant_key)],
        EscrowInstruction::ClaimNative {
            swap_id: SWAP_ID,
            preimage: PREIMAGE,
        },
    );
    assert!(
        state
            .transition_from_public_transaction(&claim, 3, REFUND_AT - 1)
            .is_err(),
        "the authenticated-transfer child must reject recipient overflow"
    );

    assert_eq!(state.get_account_by_id(metadata_id), before_metadata);
    assert_eq!(state.get_account_by_id(custody_id), before_custody);
    assert_eq!(state.get_account_by_id(claimant), before_claimant);
    assert_eq!(metadata(&state, metadata_id).status, EscrowStatus::Funded);
    assert_eq!(state.get_account_by_id(custody_id).balance, AMOUNT);
}
