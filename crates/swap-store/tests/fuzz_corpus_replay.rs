//! Replays every seed of `fuzz/corpus/coordinator_store` through the fuzz target's own harness
//! and oracles, so the stable workspace test run covers the corpus that only cargo-fuzz (nightly)
//! would otherwise execute, and pins the Zcash records the harness reads to their constructors.

#[path = "../../../fuzz/fuzz_targets/coordinator_model.rs"]
mod coordinator_model;
#[path = "../../../fuzz/fuzz_targets/coordinator_store_harness.rs"]
mod coordinator_store_harness;

use std::path::{Path, PathBuf};

use lez_zec_swap_sdk::{
    Bip199Contract, CanonicalZcashOutputObservation, ExpectedBip199Output,
    TransparentFundingRequest, TransparentUtxo, ZcashNodeSnapshot, ZcashObservationEventRecordV1,
    ZcashStableTip, ZecProfileId, ZecSwapBinding, ZecSwapBindingRecordV1,
    build_funding_transaction,
};
use secp256k1::{PublicKey, Secp256k1, SecretKey};
use zcash_primitives::block::BlockHash;
use zcash_protocol::{
    consensus::{BlockHeight, BranchId, NetworkType},
    value::Zatoshis,
};
use zcash_transparent::{
    address::Script,
    bundle::{OutPoint, TxOut},
};

fn fuzz_directory(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fuzz")
        .join(name)
}

#[test]
fn coordinator_store_fuzz_corpus_replays_on_stable() {
    let corpus = fuzz_directory("corpus/coordinator_store");
    let mut replayed = 0;
    for entry in std::fs::read_dir(&corpus).expect("fuzz corpus directory must be readable") {
        let path = entry.expect("fuzz corpus entry must be readable").path();
        let data = std::fs::read(&path).expect("fuzz corpus seed must be readable");
        println!("replaying {}", path.display());
        coordinator_store_harness::run(&data);
        replayed += 1;
    }
    assert!(replayed > 0, "no seeds found in {}", corpus.display());
}

fn zatoshis(value: u64) -> Zatoshis {
    Zatoshis::from_u64(value).unwrap()
}

fn contract() -> Bip199Contract {
    Bip199Contract::new(500_000, [0x11; 20], [0x22; 32], [0x33; 20])
}

fn expected_output(value: u64) -> ExpectedBip199Output {
    ExpectedBip199Output::new(
        NetworkType::Regtest,
        BranchId::Nu6_2,
        zatoshis(value),
        contract(),
    )
}

/// The binding of `zec_event_journal.rs`.
fn binding(value: u64) -> ZecSwapBindingRecordV1 {
    ZecSwapBindingRecordV1::from_binding(
        &ZecSwapBinding::new(ZecProfileId::DeterministicLocalV1, expected_output(value)).unwrap(),
    )
}

/// The canonical funding observation of `zec_event_journal.rs` at one stable tip.
fn event(tip_height: u32) -> ZcashObservationEventRecordV1 {
    let key = SecretKey::from_slice(&[7; 32]).unwrap();
    let public_key = PublicKey::from_secret_key(&Secp256k1::new(), &key);
    let owner_script: Script = lez_zec_swap_sdk::p2pkh_address(&public_key).script().into();
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
    let transaction = build_funding_transaction(&contract(), &request, &key).unwrap();
    let mut raw = vec![];
    transaction.write(&mut raw).unwrap();
    let observation = CanonicalZcashOutputObservation::validate(
        &expected_output(100_000),
        &ZcashNodeSnapshot::new(
            NetworkType::Regtest,
            BranchId::Nu6_2,
            true,
            BlockHash([0x44; 32]),
            BlockHash([0x44; 32]),
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
    .unwrap();
    ZcashObservationEventRecordV1::from_canonical(&observation)
}

fn assert_fixture(name: &str, value: &impl serde::Serialize) {
    let path = fuzz_directory("fixtures/coordinator_store").join(name);
    let expected = serde_json::to_string(value).unwrap();
    if std::env::var_os("LEZ_WRITE_FUZZ_FIXTURES").is_some() {
        std::fs::write(&path, format!("{expected}\n")).unwrap();
    }
    let fixture = std::fs::read_to_string(&path).expect("fixture must be readable");
    assert_eq!(
        fixture.trim_end(),
        expected,
        "{} is not the JSON of its constructor; run this test with LEZ_WRITE_FUZZ_FIXTURES=1 to rewrite it",
        path.display()
    );
}

#[test]
fn coordinator_store_fuzz_fixtures_are_their_constructors() {
    assert_fixture("zec-binding.json", &binding(100_000));
    assert_fixture("zec-binding-other.json", &binding(99_000));
    assert_fixture("zec-event-tip-102.json", &event(102));
    assert_fixture("zec-event-tip-103.json", &event(103));
}
