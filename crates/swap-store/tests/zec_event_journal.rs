use lez_swap_core::{
    Chain, ChainPosition, ConfirmationPolicy, Pair, Participant, RecoverySchedule, SwapCoordinator,
    SwapDirection, SwapId, TimelockSafety,
};
use lez_swap_store::{SqliteSwapStore, StoreError};
use lez_zec_swap_sdk::{
    Bip199Contract, CanonicalZcashOutputObservation, ExpectedBip199Output,
    TransparentFundingRequest, TransparentUtxo, ZcashNodeSnapshot, ZcashObservationEventRecordV1,
    ZcashStableTip, ZecProfileId, ZecSwapBinding, build_funding_transaction,
};
use rusqlite::{Connection, params};
use secp256k1::{PublicKey, Secp256k1, SecretKey};
use tempfile::tempdir;
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

fn swap() -> SwapCoordinator {
    SwapCoordinator::new_with_direction(
        SwapId::new("zec-journal").unwrap(),
        Pair::Zcash,
        SwapDirection::TakerSellsForeign,
        ConfirmationPolicy::new(1).unwrap(),
        RecoverySchedule::new(
            Pair::Zcash,
            SwapDirection::TakerSellsForeign,
            ChainPosition::block_height(Chain::Lez, 100),
            ChainPosition::block_height(Chain::Zcash, 120),
            TimelockSafety::between(Chain::Lez, Chain::Zcash, 1_000, 1_200, 100).unwrap(),
        )
        .unwrap(),
    )
}

fn record(tip_height: u32) -> ZcashObservationEventRecordV1 {
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
    let observation = CanonicalZcashOutputObservation::validate(
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

fn binding(value: u64) -> ZecSwapBinding {
    ZecSwapBinding::new(
        ZecProfileId::DeterministicLocalV1,
        ExpectedBip199Output::new(
            NetworkType::Regtest,
            BranchId::Nu6_2,
            zatoshis(value),
            Bip199Contract::new(500_000, [0x11; 20], [0x22; 32], [0x33; 20]),
        ),
    )
    .unwrap()
}

#[test]
fn zcash_binding_is_restart_safe_idempotent_and_immutable() {
    let data = tempdir().unwrap();
    let path = data.path().join("binding.sqlite3");
    let swap = swap();
    let original = binding(100_000);
    let changed = binding(99_000);
    let mut store = SqliteSwapStore::open(&path).unwrap();

    store.save_with_zcash_binding(&swap, &original).unwrap();
    assert_eq!(
        store.load_zcash_binding(swap.id()).unwrap(),
        Some(original.clone())
    );
    store.save_with_zcash_binding(&swap, &original).unwrap();
    assert!(matches!(
        store.save_with_zcash_binding(&swap, &changed),
        Err(StoreError::ImmutableZcashBindingMismatch)
    ));
    assert_eq!(
        store.load_zcash_binding(swap.id()).unwrap(),
        Some(original.clone())
    );
    drop(store);

    let store = SqliteSwapStore::open(path).unwrap();
    assert_eq!(store.load_zcash_binding(swap.id()).unwrap(), Some(original));
    assert_eq!(store.load(swap.id()).unwrap(), Some(swap));
}

#[test]
fn failed_binding_insert_rolls_back_the_new_swap() {
    let data = tempdir().unwrap();
    let path = data.path().join("binding-rollback.sqlite3");
    let swap = swap();
    let mut store = SqliteSwapStore::open(&path).unwrap();
    let connection = Connection::open(&path).unwrap();
    connection
        .execute_batch(
            "
            CREATE TRIGGER reject_binding_insert
            BEFORE INSERT ON zcash_swap_bindings
            BEGIN
                SELECT RAISE(ABORT, 'forced binding failure');
            END;
            ",
        )
        .unwrap();
    drop(connection);

    assert!(matches!(
        store.save_with_zcash_binding(&swap, &binding(100_000)),
        Err(StoreError::Sqlite(_))
    ));
    assert_eq!(store.load(swap.id()).unwrap(), None);
    assert_eq!(store.load_zcash_binding(swap.id()).unwrap(), None);
}

#[test]
fn journal_commit_and_replay_probe_require_a_matching_binding() {
    let data = tempdir().unwrap();
    let unbound_swap = swap();
    let event = record(102);
    let mut unbound = SqliteSwapStore::open(data.path().join("unbound-journal.sqlite3")).unwrap();
    unbound.save(&unbound_swap).unwrap();
    assert!(matches!(
        unbound.commit_zcash_event(0, &unbound_swap, Participant::Taker, &event),
        Err(StoreError::MissingZcashBinding)
    ));
    assert!(matches!(
        unbound.committed_zcash_event(0, unbound_swap.id(), Participant::Taker, &event),
        Err(StoreError::MissingZcashBinding)
    ));
    assert_eq!(unbound.revision(unbound_swap.id()).unwrap(), Some(0));

    let mismatched_swap = swap();
    let mut mismatched =
        SqliteSwapStore::open(data.path().join("mismatched-journal.sqlite3")).unwrap();
    mismatched
        .save_with_zcash_binding(&mismatched_swap, &binding(99_000))
        .unwrap();
    assert!(matches!(
        mismatched.commit_zcash_event(0, &mismatched_swap, Participant::Taker, &event),
        Err(StoreError::ZcashBindingRecord(_))
    ));
    assert_eq!(mismatched.revision(mismatched_swap.id()).unwrap(), Some(0));
}

#[test]
fn event_and_aggregate_revision_commit_atomically_and_replay_idempotently() {
    let data = tempdir().unwrap();
    let path = data.path().join("journal.sqlite3");
    let swap = swap();
    let event = record(102);
    let mut store = SqliteSwapStore::open(&path).unwrap();
    store
        .save_with_zcash_binding(&swap, &binding(100_000))
        .unwrap();

    let commit = store
        .commit_zcash_event(0, &swap, Participant::Taker, &event)
        .unwrap();
    assert_eq!(commit.revision(), 1);
    assert!(!commit.was_replay());
    let probed = store
        .committed_zcash_event(0, swap.id(), Participant::Taker, &event)
        .unwrap()
        .expect("the exact predecessor slot is durable");
    assert_eq!(probed.revision(), commit.revision());
    assert!(probed.was_replay());
    let replay = store
        .commit_zcash_event(0, &swap, Participant::Taker, &event)
        .unwrap();
    assert_eq!(replay.revision(), 1);
    assert!(replay.was_replay());
    drop(store);

    let store = SqliteSwapStore::open(&path).unwrap();
    assert_eq!(store.revision(swap.id()).unwrap(), Some(1));
    assert_eq!(
        store
            .load_zcash_events(swap.id(), Participant::Taker)
            .unwrap(),
        vec![event]
    );
}

#[test]
fn role_journals_are_isolated_and_future_event_payloads_fail_explicitly() {
    let data = tempdir().unwrap();
    let path = data.path().join("roles.sqlite3");
    let swap = swap();
    let event = record(102);
    let mut store = SqliteSwapStore::open(&path).unwrap();
    store
        .save_with_zcash_binding(&swap, &binding(100_000))
        .unwrap();
    store
        .commit_zcash_event(0, &swap, Participant::Taker, &event)
        .unwrap();
    store
        .commit_zcash_event(1, &swap, Participant::Maker, &event)
        .unwrap();
    assert_eq!(
        store
            .load_zcash_events(swap.id(), Participant::Taker)
            .unwrap(),
        vec![event.clone()]
    );
    assert_eq!(
        store
            .load_zcash_events(swap.id(), Participant::Maker)
            .unwrap(),
        vec![event]
    );
    drop(store);

    let connection = Connection::open(&path).unwrap();
    connection
        .execute(
            "UPDATE chain_events SET payload_version = 99 WHERE funded_by = 'maker'",
            [],
        )
        .unwrap();
    drop(connection);
    let store = SqliteSwapStore::open(&path).unwrap();
    assert!(matches!(
        store.load_zcash_events(swap.id(), Participant::Maker),
        Err(StoreError::UnsupportedPayloadVersion {
            kind: "Zcash event",
            version: 99
        })
    ));
}

#[test]
fn stale_revision_rejects_event_without_advancing_aggregate_or_journal() {
    let data = tempdir().unwrap();
    let mut store = SqliteSwapStore::open(data.path().join("stale.sqlite3")).unwrap();
    let swap = swap();
    store
        .save_with_zcash_binding(&swap, &binding(100_000))
        .unwrap();
    store
        .commit_zcash_event(0, &swap, Participant::Taker, &record(102))
        .unwrap();

    assert!(matches!(
        store.commit_zcash_event(0, &swap, Participant::Taker, &record(103)),
        Err(StoreError::StaleRevision {
            expected: 0,
            actual: 1
        })
    ));
    assert_eq!(store.revision(swap.id()).unwrap(), Some(1));
    assert_eq!(
        store
            .load_zcash_events(swap.id(), Participant::Taker)
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn identical_payload_after_an_intervening_event_is_a_new_transition() {
    let data = tempdir().unwrap();
    let mut store = SqliteSwapStore::open(data.path().join("reappearance.sqlite3")).unwrap();
    let swap = swap();
    let shallow = record(102);
    let deeper = record(103);
    store
        .save_with_zcash_binding(&swap, &binding(100_000))
        .unwrap();
    store
        .commit_zcash_event(0, &swap, Participant::Taker, &shallow)
        .unwrap();
    store
        .commit_zcash_event(1, &swap, Participant::Taker, &deeper)
        .unwrap();
    let reappearance = store
        .commit_zcash_event(2, &swap, Participant::Taker, &shallow)
        .unwrap();

    assert_eq!(reappearance.revision(), 3);
    assert!(!reappearance.was_replay());
    assert_eq!(
        store
            .load_zcash_events(swap.id(), Participant::Taker)
            .unwrap(),
        vec![shallow.clone(), deeper, shallow]
    );
}

#[test]
fn legacy_v1_table_migrates_and_future_versions_fail_explicitly() {
    let data = tempdir().unwrap();
    let legacy_path = data.path().join("legacy.sqlite3");
    let swap = swap();
    let connection = Connection::open(&legacy_path).unwrap();
    connection
        .execute_batch(
            "
            CREATE TABLE swaps (
                id TEXT PRIMARY KEY NOT NULL,
                schema_version INTEGER NOT NULL,
                state_json TEXT NOT NULL
            ) STRICT;
            ",
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO swaps (id, schema_version, state_json) VALUES (?1, 1, ?2)",
            params![swap.id().as_str(), serde_json::to_string(&swap).unwrap()],
        )
        .unwrap();
    drop(connection);

    let store = SqliteSwapStore::open(&legacy_path).unwrap();
    assert_eq!(store.revision(swap.id()).unwrap(), Some(0));
    assert_eq!(store.load_zcash_binding(swap.id()).unwrap(), None);
    assert_eq!(store.load(swap.id()).unwrap(), Some(swap));
    drop(store);
    let connection = Connection::open(&legacy_path).unwrap();
    assert_eq!(
        connection
            .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
            .unwrap(),
        3
    );
    drop(connection);

    let future_path = data.path().join("future.sqlite3");
    let connection = Connection::open(&future_path).unwrap();
    connection.pragma_update(None, "user_version", 99).unwrap();
    drop(connection);
    assert!(matches!(
        SqliteSwapStore::open(future_path),
        Err(StoreError::UnsupportedDatabaseVersion(99))
    ));
}

#[test]
fn failed_aggregate_update_rolls_back_the_event_insert() {
    let data = tempdir().unwrap();
    let path = data.path().join("rollback.sqlite3");
    let swap = swap();
    let mut store = SqliteSwapStore::open(&path).unwrap();
    store
        .save_with_zcash_binding(&swap, &binding(100_000))
        .unwrap();
    let connection = Connection::open(&path).unwrap();
    connection
        .execute_batch(
            "
            CREATE TRIGGER reject_swap_update
            BEFORE UPDATE ON swaps
            BEGIN
                SELECT RAISE(ABORT, 'forced aggregate failure');
            END;
            ",
        )
        .unwrap();
    drop(connection);

    assert!(matches!(
        store.commit_zcash_event(0, &swap, Participant::Taker, &record(102)),
        Err(StoreError::Sqlite(_))
    ));
    assert_eq!(store.revision(swap.id()).unwrap(), Some(0));
    assert!(
        store
            .load_zcash_events(swap.id(), Participant::Taker)
            .unwrap()
            .is_empty()
    );
}
