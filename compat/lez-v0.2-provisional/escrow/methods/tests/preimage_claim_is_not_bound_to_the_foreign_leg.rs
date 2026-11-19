//! What the escrow enforces for a preimage claim, and what it does not.
//!
//! `claim_native` pays the custody to the claimant stored at initialization
//! when that claimant signs and presents a preimage of the stored digest,
//! before `refund_at`. Nothing in it refers to the other leg of the swap: LEZ
//! cannot observe Zcash, so the escrow cannot require that a Zcash HTLC exists
//! or was funded. The escrow therefore protects the depositor only together
//! with the protocol rule that the depositor (the first locker, the Taker)
//! holds the secret until the claimant's leg is funded: whoever holds the
//! preimage and the claimant's key can take the custody.
//!
//! Before ADR 0216 a ZEC `TakerSellsLez` Maker (the LEZ
//! claimant) held the preimage from activation, so the claim below was open to
//! it right after the Taker's LEZ lock, with no Zcash leg. Since ADR 0216 the
//! Taker holds the preimage in both directions and the Maker learns it only
//! from the Taker's Zcash claim; the protocol-level proof is
//! `maker_cannot_claim_the_takers_lez_before_the_takers_zcash_claim_reveals_the_preimage`
//! in the ZEC SDK lifecycle tests. This file keeps the escrow's side of the
//! argument explicit. Adapted from an independent reproduction of that flaw.

use borsh::BorshDeserialize as _;
use lez_zec_escrow_v02::{EscrowMetadata, EscrowStatus, Instruction as EscrowInstruction};
use lez_zec_escrow_v02_methods::ZEC_ESCROW_V02_ELF;
use nssa::{
    Account, AccountId, PrivateKey, PublicKey, PublicTransaction, V03State,
    program::Program,
    public_transaction::{Message, WitnessSet},
};
use serde::Serialize;
use sha2::{Digest as _, Sha256};
use spel_framework_core::pda::{compute_pda, seed_from_str};

const SWAP_ID: [u8; 32] = [21; 32];
const PREIMAGE: [u8; 32] = [22; 32];
const AMOUNT: u128 = 1_000;
const REFUND_AT: u64 = 1_000_000;
/// Stored at initialization and read by no instruction afterwards.
const TERMS_HASH: [u8; 32] = [0x33; 32];

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
        .collect::<Vec<_>>();
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

fn claim(
    state: &V03State,
    program_id: [u32; 8],
    metadata_id: AccountId,
    custody: AccountId,
    signer: (AccountId, &PrivateKey),
    preimage: [u8; 32],
) -> PublicTransaction {
    transaction(
        state,
        program_id,
        vec![metadata_id, custody, signer.0],
        &[signer],
        EscrowInstruction::ClaimNative {
            swap_id: SWAP_ID,
            preimage,
        },
    )
}

#[test]
fn escrow_pays_the_claimant_holding_the_preimage_whatever_the_foreign_leg() {
    let program = Program::new(ZEC_ESCROW_V02_ELF.into()).expect("checked guest is canonical");
    let program_id = program.id();
    let authenticated_transfer = programs::authenticated_transfer();
    let asset_program = authenticated_transfer.id();
    // TakerSellsLez: the Taker deposits first, the Maker is the LEZ claimant.
    let (taker, taker_key) = actor(1);
    let (maker, maker_key) = actor(2);
    let (stranger, stranger_key) = actor(3);
    let funded = |balance| Account {
        program_owner: asset_program,
        balance,
        ..Account::default()
    };
    let mut state = V03State::new()
        .with_public_accounts([
            (taker, funded(2_000)),
            (maker, funded(5)),
            (stranger, funded(5)),
        ])
        .with_programs([program, authenticated_transfer]);
    let (metadata_id, custody) = escrow_ids(program_id);

    let initialize = transaction(
        &state,
        program_id,
        vec![metadata_id, custody, taker, maker],
        &[(taker, &taker_key)],
        EscrowInstruction::InitializeNative {
            swap_id: SWAP_ID,
            terms_hash: TERMS_HASH,
            secret_digest: Sha256::digest(PREIMAGE).into(),
            amount: AMOUNT,
            refund_at: REFUND_AT,
            authenticated_transfer_program: asset_program,
        },
    );
    state
        .transition_from_public_transaction(&initialize, 1, 1_000)
        .expect("the Taker initializes the escrow");
    let early = claim(
        &state,
        program_id,
        metadata_id,
        custody,
        (maker, &maker_key),
        PREIMAGE,
    );
    state
        .transition_from_public_transaction(&early, 1, 1_000)
        .expect_err("an unfunded escrow cannot be claimed");

    let fund = transaction(
        &state,
        program_id,
        vec![metadata_id, custody, taker],
        &[(taker, &taker_key)],
        EscrowInstruction::FundNative { swap_id: SWAP_ID },
    );
    state
        .transition_from_public_transaction(&fund, 2, 1_001)
        .expect("the Taker funds custody");
    assert_eq!(metadata(&state, metadata_id).status, EscrowStatus::Funded);
    assert_eq!(state.get_account_by_id(custody).balance, AMOUNT);

    // What the escrow enforces: the stored claimant's signature and the preimage.
    let wrong_preimage = claim(
        &state,
        program_id,
        metadata_id,
        custody,
        (maker, &maker_key),
        [23; 32],
    );
    state
        .transition_from_public_transaction(&wrong_preimage, 3, 1_002)
        .expect_err("a preimage of another digest is refused");
    let not_the_claimant = claim(
        &state,
        program_id,
        metadata_id,
        custody,
        (stranger, &stranger_key),
        PREIMAGE,
    );
    state
        .transition_from_public_transaction(&not_the_claimant, 3, 1_002)
        .expect_err("the preimage alone does not let another account claim");

    // What it does not: anything about the foreign leg. With no Zcash anywhere,
    // the claimant holding the preimage takes the custody. Safety therefore rests
    // on the claimant not holding the preimage before its own leg is funded.
    let taker_before = state.get_account_by_id(taker).balance;
    let maker_before = state.get_account_by_id(maker).balance;
    let paid = claim(
        &state,
        program_id,
        metadata_id,
        custody,
        (maker, &maker_key),
        PREIMAGE,
    );
    state
        .transition_from_public_transaction(&paid, 3, 1_002)
        .expect("the claimant with the preimage is paid without any foreign evidence");
    assert_eq!(metadata(&state, metadata_id).status, EscrowStatus::Claimed);
    assert_eq!(state.get_account_by_id(custody).balance, 0);
    assert_eq!(state.get_account_by_id(maker).balance, maker_before + AMOUNT);
    assert_eq!(state.get_account_by_id(taker).balance, taker_before);

    let refund = transaction(
        &state,
        program_id,
        vec![metadata_id, custody, taker],
        &[],
        EscrowInstruction::RefundNative { swap_id: SWAP_ID },
    );
    state
        .transition_from_public_transaction(&refund, 4, REFUND_AT)
        .expect_err("a claimed escrow is not refunded");
    assert_eq!(state.get_account_by_id(custody).balance, 0);
}
