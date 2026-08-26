use lez_zec_swap_sdk::{
    Bip199Contract, CanonicalZcashOutputObservation, CanonicalZcashOutputRemoval,
    ExpectedBip199Output, ObservationError, ObservationTrackerError, TransparentFundingRequest,
    TransparentUtxo, ZcashNodeRemovalSnapshot, ZcashNodeSnapshot, ZcashObservationEvent,
    ZcashObservationReconciliation, ZcashObservationTracker, ZcashStableTip,
    build_funding_transaction,
};
use secp256k1::{PublicKey, Secp256k1, SecretKey};
use zcash_primitives::{block::BlockHash, transaction::Transaction};
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

#[test]
fn removal_requires_a_stable_changed_chain_and_matching_tracker_head() {
    let (expected, transaction, raw) = fixture();
    let previous = observation(&expected, &transaction, &raw, 0x44, 102);

    let unchanged = ZcashNodeRemovalSnapshot::new(
        NetworkType::Regtest,
        BranchId::Nu6_2,
        previous.block_hash(),
        ZcashStableTip::new(
            BlockHash([0xbb; 32]),
            BlockHeight::from_u32(104),
            BlockHash([0xbb; 32]),
            BlockHeight::from_u32(104),
        ),
    );
    assert_eq!(
        CanonicalZcashOutputRemoval::validate(&previous, &unchanged),
        Err(ObservationError::InclusionStillCanonical)
    );

    let unstable = ZcashNodeRemovalSnapshot::new(
        NetworkType::Regtest,
        BranchId::Nu6_2,
        BlockHash([0x55; 32]),
        ZcashStableTip::new(
            BlockHash([0xbb; 32]),
            BlockHeight::from_u32(104),
            BlockHash([0xcc; 32]),
            BlockHeight::from_u32(105),
        ),
    );
    assert_eq!(
        CanonicalZcashOutputRemoval::validate(&previous, &unstable),
        Err(ObservationError::UnstableTip)
    );

    let other = observation(&expected, &transaction, &raw, 0x77, 102);
    let tracker = ZcashObservationTracker::from_current(Some(other));
    assert_eq!(
        tracker.propose(&ZcashObservationReconciliation::Removed(removal(
            &previous, 0x55, 104,
        ))),
        Err(ObservationTrackerError::StaleEvidence)
    );
    assert_eq!(
        tracker.propose(&ZcashObservationReconciliation::Canonical(previous)),
        Err(ObservationTrackerError::ReplacementProofRequired)
    );
}

fn fixture() -> (ExpectedBip199Output, Transaction, Vec<u8>) {
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
    let expected = ExpectedBip199Output::new(
        NetworkType::Regtest,
        BranchId::Nu6_2,
        zatoshis(100_000),
        contract.clone(),
    );
    let transaction = build_funding_transaction(&contract, &request, &key).unwrap();
    let mut raw = vec![];
    transaction.write(&mut raw).unwrap();
    (expected, transaction, raw)
}

fn observation(
    expected: &ExpectedBip199Output,
    transaction: &Transaction,
    raw: &[u8],
    block_byte: u8,
    tip_height: u32,
) -> CanonicalZcashOutputObservation {
    let block_height = 100;
    CanonicalZcashOutputObservation::validate(
        expected,
        &ZcashNodeSnapshot::new(
            NetworkType::Regtest,
            BranchId::Nu6_2,
            true,
            BlockHash([block_byte; 32]),
            BlockHash([block_byte; 32]),
            BlockHeight::from_u32(block_height),
            ZcashStableTip::new(
                BlockHash([0xaa; 32]),
                BlockHeight::from_u32(tip_height),
                BlockHash([0xaa; 32]),
                BlockHeight::from_u32(tip_height),
            ),
            transaction.txid(),
            raw.to_vec(),
            0,
            tip_height - block_height + 1,
        ),
    )
    .unwrap()
}

fn removal(
    previous: &CanonicalZcashOutputObservation,
    canonical_block_byte: u8,
    tip_height: u32,
) -> CanonicalZcashOutputRemoval {
    CanonicalZcashOutputRemoval::validate(
        previous,
        &ZcashNodeRemovalSnapshot::new(
            NetworkType::Regtest,
            BranchId::Nu6_2,
            BlockHash([canonical_block_byte; 32]),
            ZcashStableTip::new(
                BlockHash([0xbb; 32]),
                BlockHeight::from_u32(tip_height),
                BlockHash([0xbb; 32]),
                BlockHeight::from_u32(tip_height),
            ),
        ),
    )
    .unwrap()
}

#[test]
fn tracker_emits_only_durable_canonical_changes_and_explicit_removal() {
    let (expected, transaction, raw) = fixture();
    let first = observation(&expected, &transaction, &raw, 0x44, 102);
    let deeper = observation(&expected, &transaction, &raw, 0x44, 103);
    let mut tracker = ZcashObservationTracker::default();

    let first_event = ZcashObservationEvent::Canonical(first.clone());
    assert_eq!(
        tracker
            .propose(&ZcashObservationReconciliation::Canonical(first.clone()))
            .unwrap(),
        Some(first_event.clone())
    );
    assert_eq!(tracker.current(), None, "proposal is not a durable commit");
    assert_eq!(
        tracker
            .propose(&ZcashObservationReconciliation::Canonical(first.clone()))
            .unwrap(),
        Some(first_event.clone()),
        "a failed commit must not suppress the next poll"
    );
    tracker.apply_committed(&first_event).unwrap();
    assert_eq!(
        tracker
            .propose(&ZcashObservationReconciliation::Canonical(first.clone()))
            .unwrap(),
        None
    );
    let deeper_event = tracker
        .propose(&ZcashObservationReconciliation::Canonical(deeper.clone()))
        .unwrap()
        .unwrap();
    assert_eq!(
        deeper_event,
        ZcashObservationEvent::Canonical(deeper.clone())
    );
    tracker.apply_committed(&deeper_event).unwrap();

    let removed = removal(&deeper, 0x66, 104);
    let removed_event = tracker
        .propose(&ZcashObservationReconciliation::Removed(removed.clone()))
        .unwrap()
        .unwrap();
    assert_eq!(
        removed_event,
        ZcashObservationEvent::Removed(removed.clone())
    );
    assert_eq!(tracker.current(), Some(&deeper));
    tracker.apply_committed(&removed_event).unwrap();
    assert_eq!(
        tracker
            .propose(&ZcashObservationReconciliation::Removed(removed))
            .unwrap(),
        None
    );
    assert_eq!(tracker.current(), None);
}

#[test]
fn tracker_distinguishes_replacement_and_can_resume_after_restart() {
    let (expected, transaction, raw) = fixture();
    let old = observation(&expected, &transaction, &raw, 0x44, 102);
    let replacement = observation(&expected, &transaction, &raw, 0x55, 104);
    let mut tracker = ZcashObservationTracker::default();
    let old_event = tracker
        .propose(&ZcashObservationReconciliation::Canonical(old.clone()))
        .unwrap()
        .unwrap();
    tracker.apply_committed(&old_event).unwrap();

    let restored = tracker.current().cloned();
    let mut restarted = ZcashObservationTracker::from_current(restored);
    assert_eq!(
        restarted
            .propose(&ZcashObservationReconciliation::Canonical(old.clone()))
            .unwrap(),
        None
    );
    let removed = removal(&old, 0x55, 104);
    let replacement_input = ZcashObservationReconciliation::Replaced {
        removed: Box::new(removed.clone()),
        canonical: Box::new(replacement.clone()),
    };
    let replacement_event = ZcashObservationEvent::Replaced {
        removed: Box::new(removed),
        canonical: Box::new(replacement.clone()),
    };
    assert_eq!(
        restarted.propose(&replacement_input).unwrap(),
        Some(replacement_event.clone())
    );
    assert_eq!(restarted.current(), Some(&old));
    restarted.apply_committed(&replacement_event).unwrap();
    assert_eq!(restarted.current(), Some(&replacement));
}
