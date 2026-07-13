use lez_swap_core::{
    Chain, ChainEventProof, ChainPosition, ChainProof, ClaimEvidence, ConfirmationPolicy, Pair,
    Phase, RecoverySchedule, SwapCoordinator, SwapDirection, SwapId, TimelockSafety,
};
use lez_swap_store::SqliteSwapStore;
use rusqlite::{Connection, params};
use sha2::{Digest as _, Sha256};
use tempfile::{TempDir, tempdir};

fn swap(id: &str, pair: Pair) -> SwapCoordinator {
    let direction = if pair == Pair::Monero {
        SwapDirection::TakerSellsLez
    } else {
        SwapDirection::TakerSellsForeign
    };
    let foreign = Chain::from(pair);
    let role_chains = match direction {
        SwapDirection::TakerSellsForeign => [Chain::Lez, foreign],
        SwapDirection::TakerSellsLez => [foreign, Chain::Lez],
    };
    let schedule = if pair == Pair::Monero {
        RecoverySchedule::xmr_lez_first(ChainPosition::block_height(Chain::Lez, 120), 2).unwrap()
    } else {
        let safety_chains = if pair == Pair::Zcash {
            [Chain::Lez, Chain::Zcash]
        } else {
            role_chains
        };
        RecoverySchedule::new(
            pair,
            direction,
            ChainPosition::block_height(role_chains[0], 100),
            ChainPosition::block_height(role_chains[1], 120),
            TimelockSafety::between(safety_chains[0], safety_chains[1], 1_000, 1_200, 100).unwrap(),
        )
        .unwrap()
    };
    SwapCoordinator::new_with_direction(
        SwapId::new(id).unwrap(),
        pair,
        direction,
        ConfirmationPolicy::new(2).unwrap(),
        schedule,
    )
}

#[test]
fn xmr_event_gated_recovery_survives_each_restart() {
    let data_dir = tempdir().unwrap();
    let database = data_dir.path().join("xmr-recovery.sqlite3");
    let mut current = swap("xmr-restart-recovery", Pair::Monero);
    current
        .observe_taker_lock(ChainProof::new("lez-lock", 2).unwrap())
        .unwrap();
    current
        .observe_maker_lock(ChainProof::new("xmr-lock", 10).unwrap())
        .unwrap();
    current
        .refund_taker_leg(ChainPosition::block_height(Chain::Lez, 120))
        .unwrap();
    save_and_reload(&database, &mut current);
    assert_eq!(current.phase(), Phase::TakerLegRefunded);

    current
        .observe_taker_refund_for_maker_recovery(
            ChainEventProof::new(Chain::Lez, "lez-refund", 2).unwrap(),
        )
        .unwrap();
    save_and_reload(&database, &mut current);
    assert_eq!(current.phase(), Phase::MakerRecoveryAvailable);

    current
        .observe_maker_recovery(ChainProof::new("xmr-recovery", 10).unwrap())
        .unwrap();
    save_and_reload(&database, &mut current);
    assert_eq!(current.phase(), Phase::Refunded);
}

fn save_and_reload(database: &std::path::Path, swap: &mut SwapCoordinator) {
    SqliteSwapStore::open(database).unwrap().save(swap).unwrap();
    *swap = SqliteSwapStore::open(database)
        .unwrap()
        .load(swap.id())
        .unwrap()
        .expect("swap survives restart");
}

#[test]
fn schema_v8_plaintext_claim_evidence_migrates_to_current_schema_and_is_scrubbed() {
    let data = TempDir::new().expect("isolated migration directory");
    let path = data.path().join("legacy-v8-claim.sqlite3");
    let preimage = [
        0xd3, 0x25, 0xe7, 0x41, 0x9b, 0x06, 0xca, 0x58, 0xf2, 0x1d, 0x83, 0x6e, 0xb4, 0x79, 0x0f,
        0xa5, 0x67, 0xc1, 0x3a, 0x8d, 0xee, 0x52, 0x94, 0x17, 0xbb, 0x63, 0x2c, 0xf8, 0x45, 0xad,
        0x71, 0x0b,
    ];
    let mut legacy = swap("legacy-v8-claim", Pair::Zcash);
    legacy
        .observe_taker_lock(ChainProof::new("legacy-taker-lock", 2).unwrap())
        .unwrap();
    legacy
        .observe_maker_lock(ChainProof::new("legacy-maker-lock", 2).unwrap())
        .unwrap();
    legacy
        .observe_revealing_claim(
            legacy.first_claimant(),
            ChainProof::new("legacy-revealing-claim", 2).unwrap(),
            ClaimEvidence::new(preimage),
        )
        .unwrap();
    let mut state = serde_json::to_value(&legacy).expect("current coordinator JSON");
    state["claim_evidence"] = serde_json::json!(preimage);
    assert!(
        serde_json::from_value::<SwapCoordinator>(state.clone()).is_err(),
        "core must continue rejecting the historical plaintext tuple"
    );
    let state_json = serde_json::to_string(&state).expect("legacy coordinator JSON");
    let legacy_pattern = format!(
        "\"claim_evidence\":{}",
        serde_json::to_string(&preimage).expect("legacy tuple JSON")
    );
    create_schema_v8_claim_fixture(&path, legacy.id(), &state_json);
    assert_sqlite_files_contain(&path, legacy_pattern.as_bytes());

    let store = SqliteSwapStore::open(&path).expect("migrate schema-v8 claim row");
    let recovered = store
        .load(legacy.id())
        .expect("load migrated coordinator")
        .expect("migrated coordinator exists");
    assert_eq!(recovered.phase(), Phase::ClaimEvidenceAvailable);
    assert_eq!(
        recovered
            .claim_evidence()
            .expect("one-way claim marker")
            .commitment(),
        &<[u8; 32]>::from(Sha256::digest(preimage))
    );
    drop(store);

    let connection = Connection::open(&path).expect("inspect migrated database");
    let version: i64 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .expect("schema version");
    assert_eq!(version, 10);
    connection
        .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
        .expect("checkpoint migrated plaintext pages");
    drop(connection);
    assert_sqlite_files_exclude(&path, legacy_pattern.as_bytes());
    assert_sqlite_files_exclude(&path, &preimage);
}

fn create_schema_v8_claim_fixture(path: &std::path::Path, id: &SwapId, state_json: &str) {
    let connection = Connection::open(path).expect("create schema-v8 fixture");
    connection
        .execute_batch(
            "
            PRAGMA journal_mode = WAL;
            CREATE TABLE swaps (
                id             TEXT PRIMARY KEY NOT NULL,
                schema_version INTEGER NOT NULL,
                state_json     TEXT NOT NULL,
                revision       INTEGER NOT NULL DEFAULT 0 CHECK (revision >= 0)
            ) STRICT;
            PRAGMA user_version = 8;
            ",
        )
        .expect("schema-v8 swaps table");
    connection
        .execute(
            "INSERT INTO swaps (id, schema_version, state_json, revision)
             VALUES (?1, 1, ?2, 0)",
            params![id.as_str(), state_json],
        )
        .expect("legacy plaintext claim row");
    drop(connection);
}

fn assert_sqlite_files_contain(path: &std::path::Path, pattern: &[u8]) {
    assert!(
        sqlite_file_bytes(path)
            .iter()
            .any(|(_, bytes)| bytes.windows(pattern.len()).any(|window| window == pattern)),
        "legacy fixture must contain its distinctive plaintext JSON"
    );
}

fn assert_sqlite_files_exclude(path: &std::path::Path, pattern: &[u8]) {
    for (candidate, bytes) in sqlite_file_bytes(path) {
        assert!(
            !bytes.windows(pattern.len()).any(|window| window == pattern),
            "legacy plaintext remains in {}",
            candidate.display()
        );
    }
}

fn sqlite_file_bytes(path: &std::path::Path) -> Vec<(std::path::PathBuf, Vec<u8>)> {
    [
        path.to_path_buf(),
        std::path::PathBuf::from(format!("{}-wal", path.display())),
    ]
    .into_iter()
    .filter_map(|candidate| {
        std::fs::read(&candidate)
            .ok()
            .map(|bytes| (candidate, bytes))
    })
    .collect()
}

#[test]
fn maker_daemon_restarts_after_each_durable_step_and_taker_can_complete() {
    let data_dir = tempdir().unwrap();
    let database = data_dir.path().join("maker.sqlite3");
    let mut expected = swap("restart-journey", Pair::Bitcoin);

    {
        let store = SqliteSwapStore::open(&database).unwrap();
        store.save(&expected).unwrap();
    }

    let mut recovered = SqliteSwapStore::open(&database)
        .unwrap()
        .load(expected.id())
        .unwrap()
        .expect("offered swap survives restart");
    assert_eq!(recovered, expected);

    recovered
        .observe_taker_lock(ChainProof::new("btc-lock", 2).unwrap())
        .unwrap();
    expected = recovered.clone();
    {
        let store = SqliteSwapStore::open(&database).unwrap();
        store.save(&expected).unwrap();
    }

    let mut recovered = SqliteSwapStore::open(&database)
        .unwrap()
        .load(expected.id())
        .unwrap()
        .expect("confirmed taker lock survives restart");
    assert_eq!(recovered, expected);
    recovered
        .observe_maker_lock(ChainProof::new("lez-lock", 1).unwrap())
        .unwrap();

    {
        let store = SqliteSwapStore::open(&database).unwrap();
        store.save(&recovered).unwrap();
    }
    let mut recovered = SqliteSwapStore::open(&database)
        .unwrap()
        .load(expected.id())
        .unwrap()
        .expect("both locks survive restart");

    let claim_marker = ClaimEvidence::new([9; 32]);
    let first_claimant = recovered.first_claimant();
    recovered
        .observe_revealing_claim(
            first_claimant,
            ChainProof::new("btc-claim", 1).unwrap(),
            claim_marker.clone(),
        )
        .unwrap();
    {
        let store = SqliteSwapStore::open(&database).unwrap();
        store.save(&recovered).unwrap();
    }

    let mut recovered = SqliteSwapStore::open(&database)
        .unwrap()
        .load(expected.id())
        .unwrap()
        .expect("claim marker survives restart");
    assert_eq!(recovered.claim_evidence(), Some(&claim_marker));
    assert_ne!(claim_marker.commitment(), &[9; 32]);
    recovered
        .observe_followup_claim(
            first_claimant.other(),
            ChainProof::new("lez-claim", 1).unwrap(),
        )
        .unwrap();
    assert_eq!(recovered.phase(), Phase::Completed);
}

#[test]
fn concurrent_maker_users_are_persisted_as_isolated_swaps() {
    let data_dir = tempdir().unwrap();
    let store = SqliteSwapStore::open(data_dir.path().join("maker.sqlite3")).unwrap();
    let mut bitcoin = swap("alice-btc", Pair::Bitcoin);
    let monero = swap("bob-xmr", Pair::Monero);

    bitcoin
        .observe_taker_lock(ChainProof::new("alice-lock", 2).unwrap())
        .unwrap();
    store.save(&bitcoin).unwrap();
    store.save(&monero).unwrap();

    assert_eq!(
        store.load(bitcoin.id()).unwrap().unwrap().phase(),
        Phase::TakerLockConfirmed
    );
    assert_eq!(
        store.load(monero.id()).unwrap().unwrap().phase(),
        Phase::Offered
    );
}
