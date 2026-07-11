use lez_zec_swap_sdk::{
    Bip199Contract, CanonicalZcashOutputObservation, ExpectedBip199Output,
    TransparentFundingRequest, TransparentUtxo, ZcashNodeSnapshot, ZcashObservationEventRecordV1,
    ZcashStableTip, build_funding_transaction,
};
use secp256k1::{PublicKey, Secp256k1, SecretKey};
use serde_json::Value;
use zcash_primitives::block::BlockHash;
use zcash_protocol::{
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

fn observation() -> CanonicalZcashOutputObservation {
    let key = SecretKey::from_slice(&[7; 32]).unwrap();
    let public_key = PublicKey::from_secret_key(&Secp256k1::new(), &key);
    let owner_script: Script = TransparentAddress::from_pubkey(&public_key).script().into();
    let contract = Bip199Contract::new(500_000, [0x11; 20], [0x22; 32], [0x33; 20]);
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
    let transaction = build_funding_transaction(&contract, &request, &key).unwrap();
    let mut raw = vec![];
    transaction.write(&mut raw).unwrap();
    CanonicalZcashOutputObservation::validate(
        &ExpectedBip199Output::new(
            NetworkType::Regtest,
            BranchId::Nu6_2,
            zatoshis(100_000),
            contract,
        ),
        &ZcashNodeSnapshot::new(
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
        ),
    )
    .unwrap()
}

#[test]
fn primitive_v1_record_roundtrips_without_deserializing_trusted_evidence() {
    let observation = observation();
    let record = ZcashObservationEventRecordV1::from_canonical(&observation);
    record.validate().unwrap();

    let json = serde_json::to_string(&record).unwrap();
    let restored: ZcashObservationEventRecordV1 = serde_json::from_str(&json).unwrap();
    assert_eq!(restored, record);
    assert!(restored.matches_canonical(&observation));
}

#[test]
fn primitive_v1_record_rejects_corrupted_depth_and_raw_transaction() {
    let record = ZcashObservationEventRecordV1::from_canonical(&observation());
    let mut json = serde_json::to_value(record).unwrap();
    json["canonical"]["confirmations"] = Value::from(0);
    let corrupted: ZcashObservationEventRecordV1 = serde_json::from_value(json).unwrap();
    assert!(corrupted.validate().is_err());

    let record = ZcashObservationEventRecordV1::from_canonical(&observation());
    let mut json = serde_json::to_value(record).unwrap();
    json["canonical"]["raw_transaction"] = Value::from(vec![0]);
    let corrupted: ZcashObservationEventRecordV1 = serde_json::from_value(json).unwrap();
    assert!(corrupted.validate().is_err());
}
