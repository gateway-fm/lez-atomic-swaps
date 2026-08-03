use std::{
    fs,
    os::unix::fs::{PermissionsExt as _, symlink},
    path::Path,
    sync::{Arc, Barrier},
    thread,
};

use lez_bridge_protocol::RequestId;
use lez_swap_core::{Pair, SwapDirection, SwapId};
use lez_swap_store::{
    MakerOfferId, MakerRouteV1, SqliteTakerFacadeStore, TakerFacadeStoreError,
    TakerInitiationAuthorityV1, TakerInitiationFactsV1, TakerPrivateFileBindingV1,
};
use rusqlite::Connection;
use serde_json::Value;
use tempfile::tempdir;

#[test]
fn empty_registry_create_and_exact_reopen_are_strict() {
    let root = private_root();
    let database = root.path().join("taker.sqlite3");
    let registry = SqliteTakerFacadeStore::create_new(&database).unwrap();
    assert!(registry.list_initiations().unwrap().is_empty());
    assert_eq!(
        fs::metadata(&database).unwrap().permissions().mode() & 0o7777,
        0o600
    );
    drop(registry);

    let reopened = SqliteTakerFacadeStore::open_existing(&database).unwrap();
    assert!(reopened.list_initiations().unwrap().is_empty());
    assert!(matches!(
        SqliteTakerFacadeStore::create_new(&database),
        Err(TakerFacadeStoreError::DatabaseAlreadyExists)
    ));
}

#[test]
fn future_foreign_and_replaced_database_files_fail_closed() {
    let root = private_root();
    let future = root.path().join("future.sqlite3");
    drop(SqliteTakerFacadeStore::create_new(&future).unwrap());
    let connection = Connection::open(&future).unwrap();
    connection.pragma_update(None, "user_version", 2).unwrap();
    drop(connection);
    assert!(matches!(
        SqliteTakerFacadeStore::open_existing(&future),
        Err(TakerFacadeStoreError::FutureSchema)
    ));

    let foreign = root.path().join("foreign.sqlite3");
    let connection = Connection::open(&foreign).unwrap();
    connection
        .execute_batch("CREATE TABLE foreign_table (id INTEGER);")
        .unwrap();
    drop(connection);
    fs::set_permissions(&foreign, fs::Permissions::from_mode(0o600)).unwrap();
    assert!(matches!(
        SqliteTakerFacadeStore::open_existing(&foreign),
        Err(TakerFacadeStoreError::ForeignSchema)
    ));

    let replaced = root.path().join("replaced.sqlite3");
    let registry = SqliteTakerFacadeStore::create_new(&replaced).unwrap();
    let moved = root.path().join("moved.sqlite3");
    fs::rename(&replaced, &moved).unwrap();
    fs::copy(&moved, &replaced).unwrap();
    fs::set_permissions(&replaced, fs::Permissions::from_mode(0o600)).unwrap();
    assert!(matches!(
        registry.list_initiations(),
        Err(TakerFacadeStoreError::UnsafeDatabaseFile)
    ));
}

#[test]
fn create_and_reopen_reject_symlinked_ancestors() {
    let root = private_root();
    let real = root.path().join("real");
    let owner = real.join("owner");
    fs::create_dir(&real).unwrap();
    fs::create_dir(&owner).unwrap();
    fs::set_permissions(&owner, fs::Permissions::from_mode(0o700)).unwrap();
    let alias = root.path().join("alias");
    symlink(&real, &alias).unwrap();

    let aliased_database = alias.join("owner").join("taker.sqlite3");
    assert_eq!(
        SqliteTakerFacadeStore::create_new(&aliased_database).unwrap_err(),
        TakerFacadeStoreError::UnsafeDatabaseFile
    );

    let real_database = owner.join("taker.sqlite3");
    assert!(!real_database.exists());
    drop(SqliteTakerFacadeStore::create_new(&real_database).unwrap());
    assert_eq!(
        SqliteTakerFacadeStore::open_existing(&aliased_database).unwrap_err(),
        TakerFacadeStoreError::UnsafeDatabaseFile
    );
}

#[test]
fn maker_identity_must_be_a_real_secp256k1_public_key() {
    let mut invalid = [0xff; 33];
    invalid[0] = 0x02;
    assert_eq!(
        TakerInitiationFactsV1::new(
            SwapId::new("m6-invalid-curve-swap").unwrap(),
            MakerOfferId::new("m6-invalid-curve-offer").unwrap(),
            MakerRouteV1::new(Pair::Zcash, SwapDirection::TakerSellsLez).unwrap(),
            invalid,
            [0x42; 32],
            1,
            1,
        ),
        Err(TakerFacadeStoreError::InvalidInput)
    );

    let facts = make_facts("m6-maker-serde-swap", "m6-maker-serde-offer", 42);
    let canonical = serde_json::to_value(&facts).unwrap();
    assert_eq!(
        canonical["maker_identity"],
        "0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798"
    );
    assert_eq!(
        serde_json::from_value::<TakerInitiationFactsV1>(canonical.clone()).unwrap(),
        facts
    );

    let mut uppercase = canonical;
    uppercase["maker_identity"] = Value::String(
        "0279BE667EF9DCBBAC55A06295CE870B07029BFCDB2DCE28D959F2815B16F81798".to_owned(),
    );
    assert!(serde_json::from_value::<TakerInitiationFactsV1>(uppercase).is_err());
}

#[test]
fn initiation_is_atomic_and_exactly_replays_after_restart() {
    let root = private_root();
    let database = root.path().join("registry.sqlite3");
    let mut registry = SqliteTakerFacadeStore::create_new(&database).unwrap();
    let request = request("m6-initiation-001");
    let facts = make_facts("m6-zec-swap-001", "m6-zec-offer-001", 17);
    let authority = make_authority(root.path(), "001", 17);

    let admitted = registry
        .admit_initiation(&request, &facts, &authority, 1_000)
        .unwrap();
    assert_eq!(admitted.facts(), &facts);
    assert!(!admitted.was_replay());
    let replay = registry
        .admit_initiation(&request, &facts, &authority, 1_001)
        .unwrap();
    assert_eq!(replay.facts(), &facts);
    assert!(replay.was_replay());
    assert_eq!(registry.list_initiations().unwrap(), vec![facts.clone()]);
    drop(registry);

    let mut reopened = SqliteTakerFacadeStore::open_existing(&database).unwrap();
    assert_eq!(reopened.list_initiations().unwrap(), vec![facts.clone()]);
    assert_eq!(
        reopened.lookup_initiation(&request).unwrap(),
        Some(facts.clone())
    );
    assert_eq!(
        reopened
            .lookup_initiation(&RequestId::new("m6-initiation-unknown").unwrap())
            .unwrap(),
        None
    );
    assert!(
        reopened
            .admit_initiation(&request, &facts, &authority, 2_000)
            .unwrap()
            .was_replay()
    );
}

#[test]
fn changed_payload_private_authority_or_operation_conflicts_globally() {
    let root = private_root();
    let database = root.path().join("registry.sqlite3");
    let request = request("m6-initiation-conflict-001");
    let facts = make_facts("m6-zec-swap-conflict-001", "m6-zec-offer-conflict-001", 18);
    let authority = make_authority(root.path(), "conflict-001", 18);
    let mut registry = SqliteTakerFacadeStore::create_new(&database).unwrap();
    registry
        .admit_initiation(&request, &facts, &authority, 1_000)
        .unwrap();

    let changed_facts = make_facts("m6-zec-swap-conflict-001", "m6-zec-offer-changed-001", 18);
    assert_eq!(
        registry.admit_initiation(&request, &changed_facts, &authority, 1_001),
        Err(TakerFacadeStoreError::RequestConflict)
    );
    let changed_authority = make_authority(root.path(), "conflict-changed", 18);
    assert_eq!(
        registry.admit_initiation(&request, &facts, &changed_authority, 1_001),
        Err(TakerFacadeStoreError::RequestConflict)
    );
    let connection = Connection::open(&database).unwrap();
    connection
        .execute(
            "UPDATE taker_facade_requests SET operation = 'claim' WHERE request_id = ?1",
            [request.as_str()],
        )
        .unwrap();
    drop(connection);
    assert_eq!(
        registry.admit_initiation(&request, &facts, &authority, 1_002),
        Err(TakerFacadeStoreError::RequestConflict)
    );
}

#[test]
fn exact_replay_rejects_runtime_projection_drift_as_corrupt() {
    let root = private_root();
    let database = root.path().join("runtime-drift.sqlite3");
    let request = request("m6-runtime-drift-request");
    let facts = make_facts("m6-runtime-drift-swap", "m6-runtime-drift-offer", 22);
    let authority = make_authority(root.path(), "runtime-drift", 22);
    let mut registry = SqliteTakerFacadeStore::create_new(&database).unwrap();
    registry
        .admit_initiation(&request, &facts, &authority, 1_000)
        .unwrap();

    let connection = Connection::open(&database).unwrap();
    connection
        .execute("UPDATE taker_facade_swaps SET public_json = '{}'", [])
        .unwrap();
    drop(connection);

    assert_eq!(
        registry.admit_initiation(&request, &facts, &authority, 1_001),
        Err(TakerFacadeStoreError::CorruptState)
    );
    assert_eq!(
        registry.lookup_initiation(&request),
        Err(TakerFacadeStoreError::CorruptState)
    );
}

#[test]
fn same_swap_conflict_rolls_back_the_losing_request() {
    let root = private_root();
    let database = root.path().join("registry.sqlite3");
    let mut registry = SqliteTakerFacadeStore::create_new(&database).unwrap();
    let facts = make_facts("m6-zec-swap-rollback-001", "m6-zec-offer-rollback-001", 19);
    registry
        .admit_initiation(
            &request("m6-rollback-winner-001"),
            &facts,
            &make_authority(root.path(), "winner", 19),
            1_000,
        )
        .unwrap();
    let losing_request = request("m6-rollback-loser-001");
    assert_eq!(
        registry.admit_initiation(
            &losing_request,
            &facts,
            &make_authority(root.path(), "loser", 19),
            1_001,
        ),
        Err(TakerFacadeStoreError::SwapConflict)
    );

    let different = make_facts("m6-zec-swap-rollback-002", "m6-zec-offer-rollback-002", 20);
    let admitted = registry
        .admit_initiation(
            &losing_request,
            &different,
            &make_authority(root.path(), "reused-after-rollback", 20),
            1_002,
        )
        .unwrap();
    assert!(!admitted.was_replay());
    assert_eq!(registry.list_initiations().unwrap().len(), 2);
}

#[test]
fn concurrent_exact_initiation_converges_to_one_admission_and_one_replay() {
    let root = private_root();
    let database = root.path().join("concurrent-replay.sqlite3");
    let first = SqliteTakerFacadeStore::create_new(&database).unwrap();
    let second = SqliteTakerFacadeStore::open_existing(&database).unwrap();
    let request = request("m6-concurrent-replay-request");
    let facts = make_facts(
        "m6-concurrent-replay-swap",
        "m6-concurrent-replay-offer",
        23,
    );
    let authority = make_authority(root.path(), "concurrent-replay", 23);
    let barrier = Arc::new(Barrier::new(2));

    let handles = [first, second].map(|mut registry| {
        let barrier = Arc::clone(&barrier);
        let request = request.clone();
        let facts = facts.clone();
        let authority = authority.clone();
        thread::spawn(move || {
            barrier.wait();
            registry.admit_initiation(&request, &facts, &authority, 1_000)
        })
    });
    let mut outcomes = handles.map(|handle| handle.join().unwrap().unwrap());
    outcomes.sort_unstable_by_key(lez_swap_store::TakerInitiationAdmissionV1::was_replay);

    assert!(!outcomes[0].was_replay());
    assert!(outcomes[1].was_replay());
    assert_eq!(outcomes[0].facts(), &facts);
    assert_eq!(outcomes[1].facts(), &facts);
    let reopened = SqliteTakerFacadeStore::open_existing(&database).unwrap();
    assert_eq!(reopened.list_initiations().unwrap(), vec![facts]);
}

#[test]
fn concurrent_same_swap_has_one_winner_and_restart_reusable_loser_request() {
    let root = private_root();
    let database = root.path().join("concurrent-conflict.sqlite3");
    let first = SqliteTakerFacadeStore::create_new(&database).unwrap();
    let second = SqliteTakerFacadeStore::open_existing(&database).unwrap();
    let facts = make_facts(
        "m6-concurrent-conflict-swap",
        "m6-concurrent-conflict-offer",
        24,
    );
    let contenders = [
        (
            first,
            request("m6-concurrent-conflict-request-a"),
            make_authority(root.path(), "concurrent-conflict-a", 24),
        ),
        (
            second,
            request("m6-concurrent-conflict-request-b"),
            make_authority(root.path(), "concurrent-conflict-b", 34),
        ),
    ];
    let barrier = Arc::new(Barrier::new(2));
    let handles = contenders.map(|(mut registry, request, authority)| {
        let barrier = Arc::clone(&barrier);
        let facts = facts.clone();
        thread::spawn(move || {
            barrier.wait();
            let outcome = registry.admit_initiation(&request, &facts, &authority, 1_000);
            (request, authority, outcome)
        })
    });
    let outcomes = handles.map(|handle| handle.join().unwrap());

    let winner = outcomes
        .iter()
        .find(|(_, _, outcome)| outcome.is_ok())
        .expect("one concurrent contender must win");
    assert!(!winner.2.as_ref().unwrap().was_replay());
    let loser = outcomes
        .iter()
        .find(|(_, _, outcome)| matches!(outcome, Err(TakerFacadeStoreError::SwapConflict)))
        .expect("one concurrent contender must lose with a swap conflict");
    assert_eq!(
        outcomes
            .iter()
            .filter(|(_, _, outcome)| outcome.is_ok())
            .count(),
        1
    );
    assert_eq!(
        outcomes
            .iter()
            .filter(|(_, _, outcome)| {
                matches!(outcome, Err(TakerFacadeStoreError::SwapConflict))
            })
            .count(),
        1
    );

    let mut reopened = SqliteTakerFacadeStore::open_existing(&database).unwrap();
    assert_eq!(reopened.list_initiations().unwrap(), vec![facts]);
    assert!(reopened.lookup_initiation(&winner.0).unwrap().is_some());
    assert_eq!(reopened.lookup_initiation(&loser.0).unwrap(), None);
    let replacement = make_facts(
        "m6-concurrent-reused-swap",
        "m6-concurrent-reused-offer",
        25,
    );
    assert!(
        !reopened
            .admit_initiation(&loser.0, &replacement, &loser.1, 2_000)
            .unwrap()
            .was_replay()
    );
    drop(reopened);

    let reopened = SqliteTakerFacadeStore::open_existing(&database).unwrap();
    assert_eq!(reopened.list_initiations().unwrap().len(), 2);
    assert_eq!(
        reopened.lookup_initiation(&loser.0).unwrap(),
        Some(replacement)
    );
}

#[test]
fn reopen_rejects_missing_or_drifted_initiation_request_rows() {
    let root = private_root();

    let missing = root.path().join("missing-request.sqlite3");
    seed_registry(&missing, root.path(), "missing");
    let connection = Connection::open(&missing).unwrap();
    connection
        .execute("DELETE FROM taker_facade_requests", [])
        .unwrap();
    drop(connection);
    assert_eq!(
        SqliteTakerFacadeStore::open_existing(&missing).unwrap_err(),
        TakerFacadeStoreError::CorruptState
    );

    let drifted = root.path().join("drifted-request.sqlite3");
    seed_registry(&drifted, root.path(), "drifted");
    let connection = Connection::open(&drifted).unwrap();
    connection
        .execute("UPDATE taker_facade_requests SET result_json = '{}'", [])
        .unwrap();
    drop(connection);
    assert_eq!(
        SqliteTakerFacadeStore::open_existing(&drifted).unwrap_err(),
        TakerFacadeStoreError::CorruptState
    );

    let duplicated = root.path().join("duplicate-request.sqlite3");
    seed_registry(&duplicated, root.path(), "duplicated");
    let connection = Connection::open(&duplicated).unwrap();
    connection
        .execute(
            "INSERT INTO taker_facade_requests (
                 request_id, operation, swap_id, request_payload_version, request_json,
                 result_payload_version, result_json, state, created_at, updated_at
             ) SELECT 'm6-corrupt-extra-request', operation, swap_id,
                      request_payload_version, request_json, result_payload_version,
                      result_json, state, created_at, updated_at
               FROM taker_facade_requests",
            [],
        )
        .unwrap();
    drop(connection);
    assert_eq!(
        SqliteTakerFacadeStore::open_existing(&duplicated).unwrap_err(),
        TakerFacadeStoreError::CorruptState
    );
}

#[test]
fn public_projection_debug_and_errors_never_expose_private_authority() {
    let root = private_root();
    let database = root.path().join("registry.sqlite3");
    let mut registry = SqliteTakerFacadeStore::create_new(&database).unwrap();
    let request = request("m6-secrecy-request-001");
    let facts = make_facts("m6-zec-swap-secrecy-001", "m6-zec-offer-secrecy-001", 21);
    let authority = make_authority(root.path(), "private-material-marker", 21);
    let authority_debug = format!("{authority:?}");
    assert!(!authority_debug.contains("private-material-marker"));
    assert!(!authority_debug.contains(&root.path().display().to_string()));

    let commit = registry
        .admit_initiation(&request, &facts, &authority, 1_000)
        .unwrap();
    for public in [
        serde_json::to_value(&commit).unwrap(),
        serde_json::to_value(registry.list_initiations().unwrap()).unwrap(),
    ] {
        assert_public(&public);
        assert!(!public.to_string().contains("private-material-marker"));
        assert!(
            !public
                .to_string()
                .contains(&root.path().display().to_string())
        );
    }
    let debug = format!("{registry:?}");
    assert!(!debug.contains(&database.display().to_string()));
    for error in [
        TakerFacadeStoreError::DatabaseUnavailable,
        TakerFacadeStoreError::UnsafeDatabaseFile,
        TakerFacadeStoreError::DatabaseAlreadyExists,
        TakerFacadeStoreError::ForeignSchema,
        TakerFacadeStoreError::FutureSchema,
        TakerFacadeStoreError::CorruptState,
        TakerFacadeStoreError::InvalidInput,
        TakerFacadeStoreError::RequestConflict,
        TakerFacadeStoreError::SwapConflict,
        TakerFacadeStoreError::StorageUnavailable,
    ] {
        let text = error.to_string().to_ascii_lowercase();
        for forbidden in [
            "/",
            "path",
            "file",
            "socket",
            "endpoint",
            "credential",
            "key",
        ] {
            assert!(!text.contains(forbidden), "{text}");
        }
    }
}

fn make_facts(swap: &str, offer: &str, byte: u8) -> TakerInitiationFactsV1 {
    TakerInitiationFactsV1::new(
        SwapId::new(swap).unwrap(),
        MakerOfferId::new(offer).unwrap(),
        MakerRouteV1::new(Pair::Zcash, SwapDirection::TakerSellsLez).unwrap(),
        maker_identity(byte),
        [byte.wrapping_add(1); 32],
        200_000_000,
        1_820,
    )
    .unwrap()
}

fn make_authority(root: &Path, label: &str, inode: u64) -> TakerInitiationAuthorityV1 {
    TakerInitiationAuthorityV1::new(
        format!("source-{label}"),
        request(&format!("reservation-{label}")),
        TakerPrivateFileBindingV1::immutable(
            root.join(format!("{label}-envelope.bin")),
            [0x31; 32],
            1,
            inode,
        )
        .unwrap(),
        TakerPrivateFileBindingV1::immutable(
            root.join(format!("{label}-draft.bin")),
            [0x32; 32],
            1,
            inode + 1,
        )
        .unwrap(),
        TakerPrivateFileBindingV1::secret(root.join(format!("{label}-signing.key")), 1, inode + 2)
            .unwrap(),
        TakerPrivateFileBindingV1::immutable(
            root.join(format!("{label}-source.json")),
            [0x33; 32],
            1,
            inode + 3,
        )
        .unwrap(),
        root.join(format!("{label}-agreement.bin")),
        root.join(format!("{label}-actor")),
        root.join(format!("{label}-receipt.json")),
    )
    .unwrap()
}

fn request(value: &str) -> RequestId {
    RequestId::new(value).unwrap()
}

fn maker_identity(byte: u8) -> [u8; 33] {
    let _ = byte;
    [
        0x02, 0x79, 0xbe, 0x66, 0x7e, 0xf9, 0xdc, 0xbb, 0xac, 0x55, 0xa0, 0x62, 0x95, 0xce, 0x87,
        0x0b, 0x07, 0x02, 0x9b, 0xfc, 0xdb, 0x2d, 0xce, 0x28, 0xd9, 0x59, 0xf2, 0x81, 0x5b, 0x16,
        0xf8, 0x17, 0x98,
    ]
}

fn seed_registry(database: &Path, root: &Path, label: &str) {
    let mut registry = SqliteTakerFacadeStore::create_new(database).unwrap();
    registry
        .admit_initiation(
            &request(&format!("m6-{label}-request")),
            &make_facts(
                &format!("m6-{label}-swap"),
                &format!("m6-{label}-offer"),
                31,
            ),
            &make_authority(root, label, 31),
            1_000,
        )
        .unwrap();
}

fn private_root() -> tempfile::TempDir {
    let root = tempdir().unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    root
}

fn assert_public(value: &Value) {
    match value {
        Value::Object(fields) => {
            for (key, nested) in fields {
                let key = key.to_ascii_lowercase();
                for forbidden in [
                    "path",
                    "file",
                    "socket",
                    "endpoint",
                    "credential",
                    "secret",
                    "authority",
                    "source_id",
                    "device",
                    "inode",
                    "draft",
                    "raw_envelope",
                ] {
                    assert!(!key.contains(forbidden), "forbidden public field {key}");
                }
                assert_public(nested);
            }
        }
        Value::Array(values) => values.iter().for_each(assert_public),
        _ => {}
    }
}
