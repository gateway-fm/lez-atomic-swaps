use lez_zec_swap_sdk::{
    Bip199Contract, FundingBuildError, TransparentFundingRequest, TransparentUtxo,
    build_funding_transaction, select_funding_utxos,
};
use secp256k1::{PublicKey, Secp256k1, SecretKey};
use zcash_primitives::transaction::{Transaction, TxVersion};
use zcash_protocol::{
    consensus::{BlockHeight, BranchId},
    value::{BalanceError, Zatoshis},
};
use zcash_transparent::{
    address::{Script, TransparentAddress},
    bundle::{OutPoint, TxOut},
};

fn zatoshis(value: u64) -> Zatoshis {
    Zatoshis::from_u64(value).unwrap()
}

fn key(byte: u8) -> SecretKey {
    SecretKey::from_slice(&[byte; 32]).unwrap()
}

fn pubkey(secret_key: &SecretKey) -> PublicKey {
    PublicKey::from_secret_key(&Secp256k1::new(), secret_key)
}

fn owner_script(public_key: &PublicKey) -> Script {
    TransparentAddress::from_pubkey(public_key).script().into()
}

fn utxo(id: u8, index: u32, value: u64, public_key: &PublicKey) -> TransparentUtxo {
    TransparentUtxo::new(
        OutPoint::new([id; 32], index),
        TxOut::new(zatoshis(value), owner_script(public_key)),
    )
}

fn contract() -> Bip199Contract {
    Bip199Contract::new(500_000, [0x11; 20], [0x22; 32], [0x33; 20])
}

fn request(
    candidates: Vec<TransparentUtxo>,
    public_key: PublicKey,
    dust: u64,
) -> TransparentFundingRequest {
    TransparentFundingRequest::new(
        candidates,
        public_key,
        zatoshis(100_000),
        zatoshis(10_000),
        zatoshis(dust),
        BlockHeight::from_u32(4_100_000),
        BranchId::Nu6_2,
    )
    .unwrap()
}

fn serialize(transaction: &Transaction) -> Vec<u8> {
    let mut bytes = vec![];
    transaction.write(&mut bytes).unwrap();
    bytes
}

#[test]
fn selection_is_deterministic_and_largest_first() {
    let owner_key = key(7);
    let owner_pubkey = pubkey(&owner_key);
    let selection = select_funding_utxos(&request(
        vec![
            utxo(1, 0, 40_000, &owner_pubkey),
            utxo(2, 0, 50_000, &owner_pubkey),
            utxo(3, 0, 70_000, &owner_pubkey),
        ],
        owner_pubkey,
        1_000,
    ))
    .unwrap();

    assert_eq!(selection.selected().len(), 2);
    assert_eq!(selection.selected()[0].outpoint().hash(), &[3; 32]);
    assert_eq!(selection.selected()[1].outpoint().hash(), &[2; 32]);
    assert_eq!(selection.selected_value(), zatoshis(120_000));
    assert_eq!(selection.change(), Some(zatoshis(10_000)));
    assert_eq!(selection.fee(), zatoshis(10_000));
}

#[test]
fn sub_threshold_change_is_added_to_fee_instead_of_creating_dust() {
    let owner_key = key(7);
    let owner_pubkey = pubkey(&owner_key);
    let selection = select_funding_utxos(&request(
        vec![utxo(1, 0, 119_999, &owner_pubkey)],
        owner_pubkey,
        10_000,
    ))
    .unwrap();

    assert_eq!(selection.change(), None);
    assert_eq!(selection.fee(), zatoshis(19_999));
}

#[test]
fn change_exactly_at_threshold_is_preserved() {
    let owner_key = key(7);
    let owner_pubkey = pubkey(&owner_key);
    let selection = select_funding_utxos(&request(
        vec![utxo(1, 0, 120_000, &owner_pubkey)],
        owner_pubkey,
        10_000,
    ))
    .unwrap();

    assert_eq!(selection.change(), Some(zatoshis(10_000)));
    assert_eq!(selection.fee(), zatoshis(10_000));
}

#[test]
fn funding_transaction_has_exact_contract_output_and_actor_change() {
    let owner_key = key(7);
    let owner_pubkey = pubkey(&owner_key);
    let contract = contract();
    let request = request(
        vec![
            utxo(1, 0, 40_000, &owner_pubkey),
            utxo(2, 0, 50_000, &owner_pubkey),
            utxo(3, 0, 70_000, &owner_pubkey),
        ],
        owner_pubkey,
        1_000,
    );

    let transaction = build_funding_transaction(&contract, &request, &owner_key).unwrap();
    let bundle = transaction.transparent_bundle().unwrap();

    assert_eq!(bundle.vin.len(), 2);
    assert_eq!(bundle.vin[0].prevout().hash(), &[3; 32]);
    assert_eq!(bundle.vin[1].prevout().hash(), &[2; 32]);
    assert!(
        bundle
            .vin
            .iter()
            .all(|input| !input.script_sig().0.0.is_empty())
    );
    assert_eq!(bundle.vout.len(), 2);
    assert_eq!(bundle.vout[0].value(), zatoshis(100_000));
    assert_eq!(
        bundle.vout[0].script_pubkey().0.0,
        contract.p2sh_script_pubkey()
    );
    assert_eq!(bundle.vout[1].value(), zatoshis(10_000));
    assert_eq!(bundle.vout[1].script_pubkey(), &owner_script(&owner_pubkey));
    assert_eq!(
        transaction
            .fee_paid::<BalanceError, _>(|outpoint| {
                Ok(request
                    .candidates()
                    .iter()
                    .find(|candidate| candidate.outpoint() == outpoint)
                    .map(|candidate| candidate.output().value()))
            })
            .unwrap(),
        Some(zatoshis(10_000)),
    );
}

#[test]
fn funding_transaction_omits_dust_change_and_accounts_for_the_actual_fee() {
    let owner_key = key(7);
    let owner_pubkey = pubkey(&owner_key);
    let request = request(
        vec![utxo(1, 0, 119_999, &owner_pubkey)],
        owner_pubkey,
        10_000,
    );

    let transaction = build_funding_transaction(&contract(), &request, &owner_key).unwrap();
    let bundle = transaction.transparent_bundle().unwrap();

    assert_eq!(bundle.vout.len(), 1);
    assert_eq!(bundle.vout[0].value(), zatoshis(100_000));
    assert_eq!(
        transaction
            .fee_paid::<BalanceError, _>(|outpoint| {
                Ok(request
                    .candidates()
                    .iter()
                    .find(|candidate| candidate.outpoint() == outpoint)
                    .map(|candidate| candidate.output().value()))
            })
            .unwrap(),
        Some(zatoshis(19_999)),
    );
}

#[test]
fn canonical_funding_bytes_are_deterministic_and_roundtrip() {
    let owner_key = key(7);
    let owner_pubkey = pubkey(&owner_key);
    let request = request(
        vec![
            utxo(2, 4, 60_000, &owner_pubkey),
            utxo(1, 3, 60_000, &owner_pubkey),
        ],
        owner_pubkey,
        1_000,
    );
    let contract = contract();

    let first = build_funding_transaction(&contract, &request, &owner_key).unwrap();
    let second = build_funding_transaction(&contract, &request, &owner_key).unwrap();
    let bytes = serialize(&first);
    let decoded = Transaction::read(bytes.as_slice(), BranchId::Nu6_2).unwrap();

    assert_eq!(serialize(&second), bytes);
    assert_eq!(serialize(&decoded), bytes);
    assert_eq!(decoded.txid(), first.txid());
}

#[test]
fn insufficient_funds_wrong_role_and_wrong_utxo_script_fail_closed() {
    let owner_key = key(7);
    let other_key = key(8);
    let owner_pubkey = pubkey(&owner_key);
    let other_pubkey = pubkey(&other_key);

    let insufficient = request(
        vec![utxo(1, 0, 109_999, &owner_pubkey)],
        owner_pubkey,
        1_000,
    );
    assert_eq!(
        select_funding_utxos(&insufficient).unwrap_err(),
        FundingBuildError::InsufficientFunds {
            available: zatoshis(109_999),
            required: zatoshis(110_000),
        }
    );

    let valid = request(
        vec![utxo(1, 0, 120_000, &owner_pubkey)],
        owner_pubkey,
        1_000,
    );
    assert_eq!(
        build_funding_transaction(&contract(), &valid, &other_key).unwrap_err(),
        FundingBuildError::WrongFundingKey,
    );

    let wrong_script = TransparentFundingRequest::new(
        vec![utxo(1, 0, 120_000, &other_pubkey)],
        owner_pubkey,
        zatoshis(100_000),
        zatoshis(10_000),
        zatoshis(1_000),
        BlockHeight::from_u32(4_100_000),
        BranchId::Nu6_2,
    );
    assert_eq!(
        wrong_script.unwrap_err(),
        FundingBuildError::UtxoScriptMismatch
    );
}

#[test]
fn non_v5_consensus_epoch_is_rejected_before_selection() {
    let owner_key = key(7);
    let owner_pubkey = pubkey(&owner_key);
    let invalid = TransparentFundingRequest::new(
        vec![utxo(1, 0, 120_000, &owner_pubkey)],
        owner_pubkey,
        zatoshis(100_000),
        zatoshis(10_000),
        zatoshis(1_000),
        BlockHeight::from_u32(1_000_000),
        BranchId::Canopy,
    );

    assert_eq!(
        invalid.unwrap_err(),
        FundingBuildError::UnsupportedConsensusBranch {
            branch_id: BranchId::Canopy,
            transaction_version: TxVersion::V5,
        }
    );
}

#[test]
fn empty_contract_and_duplicate_outpoint_are_rejected() {
    let owner_key = key(7);
    let owner_pubkey = pubkey(&owner_key);
    let candidate = utxo(1, 0, 120_000, &owner_pubkey);
    let empty = TransparentFundingRequest::new(
        vec![candidate.clone()],
        owner_pubkey,
        Zatoshis::ZERO,
        zatoshis(10_000),
        zatoshis(1_000),
        BlockHeight::from_u32(4_100_000),
        BranchId::Nu6_2,
    );
    assert_eq!(empty.unwrap_err(), FundingBuildError::EmptyContractValue);

    let duplicate = TransparentFundingRequest::new(
        vec![candidate.clone(), candidate],
        owner_pubkey,
        zatoshis(100_000),
        zatoshis(10_000),
        zatoshis(1_000),
        BlockHeight::from_u32(4_100_000),
        BranchId::Nu6_2,
    );
    assert_eq!(duplicate.unwrap_err(), FundingBuildError::DuplicateOutpoint);
}
