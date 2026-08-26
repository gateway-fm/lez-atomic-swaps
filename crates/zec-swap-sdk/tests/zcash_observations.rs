use lez_zec_swap_sdk::{
    Bip199Contract, CanonicalZcashOutputObservation, ExpectedBip199Output, ObservationError,
    TransparentFundingRequest, TransparentUtxo, ZcashNodeSnapshot, ZcashStableTip,
    build_funding_transaction,
};
use secp256k1::{PublicKey, Secp256k1, SecretKey};
use zcash_primitives::{block::BlockHash, transaction::Transaction};
use zcash_protocol::{
    TxId,
    consensus::{BlockHeight, BranchId, NetworkType},
    value::Zatoshis,
};
use zcash_transparent::{
    address::{Script, TransparentAddress},
    bundle::{OutPoint, TxOut},
};

fn zatoshis(value: u64) -> Zatoshis {
    Zatoshis::from_u64(value).unwrap()
}

fn transaction_fixture() -> (Bip199Contract, Transaction, Vec<u8>) {
    let key = SecretKey::from_slice(&[7; 32]).unwrap();
    let public_key = PublicKey::from_secret_key(&Secp256k1::new(), &key);
    let owner_script: Script = TransparentAddress::from_pubkey(&public_key).script().into();
    let request = TransparentFundingRequest::new(
        vec![TransparentUtxo::new(
            OutPoint::new([9; 32], 0),
            TxOut::new(zatoshis(120_000), owner_script),
        )],
        public_key,
        zatoshis(100_000),
        zatoshis(10_000),
        zatoshis(1_000),
        BlockHeight::from_u32(4_100_000),
        BranchId::Nu6_2,
    )
    .unwrap();
    let contract = Bip199Contract::new(500_000, [0x11; 20], [0x22; 32], [0x33; 20]);
    let transaction = build_funding_transaction(&contract, &request, &key).unwrap();
    let mut raw = vec![];
    transaction.write(&mut raw).unwrap();
    (contract, transaction, raw)
}

fn snapshot(transaction: &Transaction, raw: Vec<u8>) -> ZcashNodeSnapshot {
    ZcashNodeSnapshot::new(
        NetworkType::Regtest,
        BranchId::Nu6_2,
        true,
        BlockHash([0x44; 32]),
        BlockHash([0x44; 32]),
        BlockHeight::from_u32(100),
        ZcashStableTip::new(
            BlockHash([0xaa; 32]),
            BlockHeight::from_u32(102),
            BlockHash([0xaa; 32]),
            BlockHeight::from_u32(102),
        ),
        transaction.txid(),
        raw,
        0,
        3,
    )
}

#[test]
fn canonical_observation_binds_complete_output_and_derives_core_evidence() {
    let (contract, transaction, raw) = transaction_fixture();
    let expected = ExpectedBip199Output::new(
        NetworkType::Regtest,
        BranchId::Nu6_2,
        zatoshis(100_000),
        contract.clone(),
    );
    let observation =
        CanonicalZcashOutputObservation::validate(&expected, &snapshot(&transaction, raw.clone()))
            .unwrap();

    assert_eq!(observation.network(), NetworkType::Regtest);
    assert_eq!(observation.consensus_branch_id(), BranchId::Nu6_2);
    assert_eq!(observation.block_hash(), BlockHash([0x44; 32]));
    assert_eq!(observation.block_height(), BlockHeight::from_u32(100));
    assert_eq!(observation.tip_block_hash(), BlockHash([0xaa; 32]));
    assert_eq!(observation.tip_height(), BlockHeight::from_u32(102));
    assert_eq!(observation.transaction_id(), transaction.txid());
    assert_eq!(observation.outpoint().n(), 0);
    assert_eq!(observation.output().value(), zatoshis(100_000));
    assert_eq!(observation.redeem_script(), contract.redeem_script());
    assert_eq!(
        observation.p2sh_script_pubkey(),
        contract.p2sh_script_pubkey()
    );
    assert_eq!(observation.confirmations().get(), 3);
    assert_eq!(observation.raw_transaction(), raw);
    let proof = observation.chain_proof().unwrap();
    assert_eq!(proof.transaction_id(), transaction.txid().to_string());
    assert_eq!(proof.confirmations(), 3);
}

#[test]
fn observation_rejects_unbound_or_noncanonical_node_data() {
    let (contract, transaction, raw) = transaction_fixture();
    let expected = ExpectedBip199Output::new(
        NetworkType::Regtest,
        BranchId::Nu6_2,
        zatoshis(100_000),
        contract,
    );

    let mut inactive = snapshot(&transaction, raw.clone());
    inactive.set_in_active_chain(false);
    assert_eq!(
        CanonicalZcashOutputObservation::validate(&expected, &inactive),
        Err(ObservationError::InactiveChain)
    );

    let mut wrong_network = snapshot(&transaction, raw.clone());
    wrong_network.set_network(NetworkType::Test);
    assert_eq!(
        CanonicalZcashOutputObservation::validate(&expected, &wrong_network),
        Err(ObservationError::NetworkMismatch)
    );

    let mut wrong_branch = snapshot(&transaction, raw.clone());
    wrong_branch.set_consensus_branch_id(BranchId::Nu6_1);
    assert_eq!(
        CanonicalZcashOutputObservation::validate(&expected, &wrong_branch),
        Err(ObservationError::ConsensusBranchMismatch)
    );

    let mut wrong_block = snapshot(&transaction, raw.clone());
    wrong_block.set_canonical_block_hash(BlockHash([0x45; 32]));
    assert_eq!(
        CanonicalZcashOutputObservation::validate(&expected, &wrong_block),
        Err(ObservationError::BlockHashMismatch)
    );

    let mut above_tip = snapshot(&transaction, raw.clone());
    above_tip.set_tip_height(BlockHeight::from_u32(99));
    assert_eq!(
        CanonicalZcashOutputObservation::validate(&expected, &above_tip),
        Err(ObservationError::BlockAboveTip)
    );

    let mut wrong_txid = snapshot(&transaction, raw.clone());
    wrong_txid.set_reported_transaction_id(TxId::from_bytes([0x99; 32]));
    assert_eq!(
        CanonicalZcashOutputObservation::validate(&expected, &wrong_txid),
        Err(ObservationError::TransactionIdMismatch)
    );

    let mut trailing = raw.clone();
    trailing.push(0);
    assert_eq!(
        CanonicalZcashOutputObservation::validate(&expected, &snapshot(&transaction, trailing)),
        Err(ObservationError::TrailingTransactionBytes)
    );

    assert_eq!(
        CanonicalZcashOutputObservation::validate(&expected, &snapshot(&transaction, vec![0])),
        Err(ObservationError::MalformedTransaction)
    );

    let mut missing_output = snapshot(&transaction, raw.clone());
    missing_output.set_output_index(99);
    assert_eq!(
        CanonicalZcashOutputObservation::validate(&expected, &missing_output),
        Err(ObservationError::OutputIndexOutOfRange)
    );

    let wrong_value = ExpectedBip199Output::new(
        NetworkType::Regtest,
        BranchId::Nu6_2,
        zatoshis(99_999),
        expected.contract().clone(),
    );
    assert_eq!(
        CanonicalZcashOutputObservation::validate(
            &wrong_value,
            &snapshot(&transaction, raw.clone()),
        ),
        Err(ObservationError::ValueMismatch)
    );

    let wrong_contract = ExpectedBip199Output::new(
        NetworkType::Regtest,
        BranchId::Nu6_2,
        zatoshis(100_000),
        Bip199Contract::new(500_001, [0x11; 20], [0x22; 32], [0x33; 20]),
    );
    assert_eq!(
        CanonicalZcashOutputObservation::validate(
            &wrong_contract,
            &snapshot(&transaction, raw.clone()),
        ),
        Err(ObservationError::ScriptMismatch)
    );

    let mut wrong_depth = snapshot(&transaction, raw);
    wrong_depth.set_reported_confirmations(2);
    assert_eq!(
        CanonicalZcashOutputObservation::validate(&expected, &wrong_depth),
        Err(ObservationError::ConfirmationMismatch)
    );

    let mut overflowing_depth = snapshot(&transaction, vec![]);
    overflowing_depth.set_block_height(BlockHeight::from_u32(0));
    overflowing_depth.set_tip_height(BlockHeight::from_u32(u32::MAX));
    assert_eq!(
        CanonicalZcashOutputObservation::validate(&expected, &overflowing_depth),
        Err(ObservationError::ConfirmationOverflow)
    );
}

#[test]
fn observation_rejects_a_tip_change_during_multi_query_snapshot() {
    let (contract, transaction, raw) = transaction_fixture();
    let expected = ExpectedBip199Output::new(
        NetworkType::Regtest,
        BranchId::Nu6_2,
        zatoshis(100_000),
        contract,
    );
    let mut unstable_tip = snapshot(&transaction, raw);
    unstable_tip.set_tip_after(BlockHash([0xbb; 32]), BlockHeight::from_u32(103));

    assert_eq!(
        CanonicalZcashOutputObservation::validate(&expected, &unstable_tip),
        Err(ObservationError::UnstableTip)
    );
}
