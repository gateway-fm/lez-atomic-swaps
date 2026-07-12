use lez_zec_swap_sdk::{
    Bip199Contract, CanonicalZcashOutputObservation, CanonicalZcashOutputRemoval,
    ExpectedBip199Output, HistoricalReplayError, TransparentFundingRequest, TransparentUtxo,
    ZcashNodeRemovalSnapshot, ZcashNodeSnapshot, ZcashObservationEvent,
    ZcashObservationEventRecordV1, ZcashStableTip, build_funding_transaction,
    replay_zcash_observation_history, revalidate_historical_event,
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
    observation_at(0x44, 102)
}

fn observation_at(block_byte: u8, tip_height: u32) -> CanonicalZcashOutputObservation {
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
            BlockHash([block_byte; 32]),
            BlockHash([block_byte; 32]),
            BlockHeight::from_u32(100),
            ZcashStableTip::new(
                BlockHash([0xaa; 32]),
                BlockHeight::from_u32(tip_height),
                BlockHash([0xaa; 32]),
                BlockHeight::from_u32(tip_height),
            ),
            transaction.txid(),
            raw,
            0,
            tip_height - 99,
        ),
    )
    .unwrap()
}

fn removal(previous: &CanonicalZcashOutputObservation) -> CanonicalZcashOutputRemoval {
    CanonicalZcashOutputRemoval::validate(
        previous,
        &ZcashNodeRemovalSnapshot::new(
            NetworkType::Regtest,
            BranchId::Nu6_2,
            BlockHash([0x55; 32]),
            ZcashStableTip::new(
                BlockHash([0xbb; 32]),
                BlockHeight::from_u32(104),
                BlockHash([0xbb; 32]),
                BlockHeight::from_u32(104),
            ),
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

#[test]
fn ordered_history_revalidates_and_restores_the_exact_committed_head() {
    let first = observation();
    let removed = removal(&first);
    let reappeared = observation_at(0x66, 104);
    let events = [
        ZcashObservationEvent::Canonical(first),
        ZcashObservationEvent::Removed(removed),
        ZcashObservationEvent::Canonical(reappeared.clone()),
    ];
    let records: Vec<_> = events
        .iter()
        .map(ZcashObservationEventRecordV1::from_event)
        .collect();

    assert_eq!(revalidate_historical_event(&records[2]).unwrap(), events[2]);
    let tracker = replay_zcash_observation_history(&records).unwrap();
    assert_eq!(tracker.current(), Some(&reappeared));
}

#[test]
fn history_replay_rejects_missing_predecessor_and_unproved_inclusion_change() {
    let first = observation();
    let removed_only = [ZcashObservationEventRecordV1::from_event(
        &ZcashObservationEvent::Removed(removal(&first)),
    )];
    assert!(matches!(
        replay_zcash_observation_history(&removed_only),
        Err(HistoricalReplayError::Sequence(_))
    ));

    let changed = observation_at(0x66, 104);
    let changed_without_replacement = [
        ZcashObservationEventRecordV1::from_event(&ZcashObservationEvent::Canonical(first)),
        ZcashObservationEventRecordV1::from_event(&ZcashObservationEvent::Canonical(changed)),
    ];
    assert!(matches!(
        replay_zcash_observation_history(&changed_without_replacement),
        Err(HistoricalReplayError::Sequence(_))
    ));
}
