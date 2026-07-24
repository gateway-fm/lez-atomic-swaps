use lez_bridge_protocol::RequestId;
use lez_swap_core::{
    Chain, ChainPosition, ConfirmationPolicy, Pair, RecoverySchedule, SwapCoordinator,
    SwapDirection, SwapId, TimelockSafety,
};
use lez_swap_store::{
    LocalPriceV1, MakerConfigurationError, MakerPairConfigurationV1, MakerPriceSourceKind,
    MakerRouteV1, SqliteSwapStore, StoreError,
};
use rusqlite::{Connection, params};
use tempfile::tempdir;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt as _;

fn request(value: &str) -> RequestId {
    RequestId::new(value).expect("bounded request ID")
}

fn route(pair: Pair, direction: SwapDirection) -> MakerRouteV1 {
    MakerRouteV1::new(pair, direction).expect("supported route")
}

fn policy(
    route: MakerRouteV1,
    enabled: bool,
    source: MakerPriceSourceKind,
) -> MakerPairConfigurationV1 {
    MakerPairConfigurationV1::new(route, enabled, source, 10, 10_000, 300).expect("bounded policy")
}

#[test]
fn local_route_configuration_is_cas_replay_safe_and_survives_restart() {
    let run = tempdir().expect("isolated store");
    let database = run.path().join("maker-configuration.sqlite3");
    let zec = route(Pair::Zcash, SwapDirection::TakerSellsLez);
    let disabled = policy(zec, false, MakerPriceSourceKind::Local);
    let enabled = policy(zec, true, MakerPriceSourceKind::Local);
    let price = LocalPriceV1::new(zec, 5, 2).expect("reduced exact price");

    let mut store = SqliteSwapStore::open(&database).expect("open maker store");
    let configured = store
        .configure_maker_pair(&request("pair-create-zec-001"), None, &disabled)
        .expect("insert disabled route");
    assert_eq!(configured.revision(), 1);
    assert!(!configured.was_replay());
    let replay = store
        .configure_maker_pair(&request("pair-create-zec-001"), None, &disabled)
        .expect("exact mutation replay");
    assert_eq!(replay.revision(), 1);
    assert!(replay.was_replay());

    let quoted = store
        .set_local_price(&request("price-create-zec-001"), None, &price)
        .expect("install local quote");
    assert_eq!(quoted.revision(), 1);
    let activated = store
        .configure_maker_pair(&request("pair-enable-zec-001"), Some(1), &enabled)
        .expect("enable quoted route");
    assert_eq!(activated.revision(), 2);

    match store.configure_maker_pair(&request("pair-stale-zec-001"), Some(1), &disabled) {
        Err(StoreError::StaleMakerConfiguration {
            expected: Some(1),
            actual: Some(2),
        }) => {}
        other => panic!("unexpected stale result: {other:?}"),
    }
    match store.configure_maker_pair(&request("pair-enable-zec-001"), Some(2), &disabled) {
        Err(StoreError::MakerConfigurationRequestConflict) => {}
        other => panic!("unexpected request-ID result: {other:?}"),
    }
    drop(store);

    let store = SqliteSwapStore::open(&database).expect("restart maker store");
    let pairs = store.list_maker_pairs().expect("list durable pairs");
    assert_eq!(pairs.len(), 1);
    assert_eq!(pairs[0].revision(), 2);
    assert_eq!(pairs[0].value(), &enabled);
    let prices = store.list_local_prices().expect("list durable prices");
    assert_eq!(prices.len(), 1);
    assert_eq!(prices[0].revision(), 1);
    assert_eq!(prices[0].value(), &price);
}

#[test]
fn local_route_enablement_requires_a_price_and_rolls_back_request_id() {
    let run = tempdir().expect("isolated store");
    let mut store =
        SqliteSwapStore::open(run.path().join("rollback.sqlite3")).expect("open maker store");
    let bitcoin = route(Pair::Bitcoin, SwapDirection::TakerSellsForeign);
    let enabled = policy(bitcoin, true, MakerPriceSourceKind::Local);
    let disabled = policy(bitcoin, false, MakerPriceSourceKind::Local);
    let request_id = request("pair-bitcoin-rollback-001");

    assert!(matches!(
        store.configure_maker_pair(&request_id, None, &enabled),
        Err(StoreError::MissingMakerLocalPrice)
    ));
    let committed = store
        .configure_maker_pair(&request_id, None, &disabled)
        .expect("failed transaction retained neither row nor request ID");
    assert_eq!(committed.revision(), 1);
    assert!(!committed.was_replay());
}

#[test]
fn route_and_price_constructors_reject_unsafe_shapes() {
    assert_eq!(
        MakerRouteV1::new(Pair::Monero, SwapDirection::TakerSellsForeign),
        Err(MakerConfigurationError::UnsupportedDirection)
    );
    let bitcoin = route(Pair::Bitcoin, SwapDirection::TakerSellsLez);
    assert_eq!(
        LocalPriceV1::new(bitcoin, 2, 4),
        Err(MakerConfigurationError::InvalidLocalPrice)
    );
    assert_eq!(
        LocalPriceV1::new(bitcoin, i64::MAX as u64 + 1, 1),
        Err(MakerConfigurationError::InvalidLocalPrice)
    );
}

#[test]
fn schema_v10_migrates_to_current_without_rewriting_coordinator_bytes() {
    let run = tempdir().expect("isolated migration store");
    let database = run.path().join("schema-v10.sqlite3");
    let swap = zcash_swap("schema-v10-swap");
    let encoded = serde_json::to_string(&swap).expect("encode coordinator");
    let connection = Connection::open(&database).expect("create v10 fixture");
    connection
        .execute_batch(
            "CREATE TABLE swaps (
                 id TEXT PRIMARY KEY NOT NULL,
                 schema_version INTEGER NOT NULL,
                 state_json TEXT NOT NULL,
                 revision INTEGER NOT NULL DEFAULT 0 CHECK (revision >= 0)
             ) STRICT;
             PRAGMA user_version = 10;",
        )
        .expect("create schema-v10 fixture");
    connection
        .execute(
            "INSERT INTO swaps (id, schema_version, state_json, revision)
             VALUES (?1, 1, ?2, 0)",
            params![swap.id().as_str(), encoded],
        )
        .expect("insert v10 aggregate");
    drop(connection);
    make_owner_private(&database);

    let store = SqliteSwapStore::open(&database).expect("migrate schema v10 to v11");
    assert_eq!(store.load(swap.id()).expect("load swap"), Some(swap));
    assert!(store.list_maker_pairs().expect("new pair table").is_empty());
    assert!(
        store
            .list_local_prices()
            .expect("new price table")
            .is_empty()
    );
    drop(store);

    let connection = Connection::open(&database).expect("inspect migrated fixture");
    let version: i64 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .expect("read schema version");
    let retained: String = connection
        .query_row(
            "SELECT state_json FROM swaps WHERE id = ?1",
            params!["schema-v10-swap"],
            |row| row.get(0),
        )
        .expect("read retained aggregate bytes");
    assert_eq!(version, 12);
    assert_eq!(retained, encoded);
}

fn zcash_swap(id: &str) -> SwapCoordinator {
    let direction = SwapDirection::TakerSellsForeign;
    let schedule = RecoverySchedule::new(
        Pair::Zcash,
        direction,
        ChainPosition::block_height(Chain::Lez, 100),
        ChainPosition::block_height(Chain::Zcash, 120),
        TimelockSafety::between(Chain::Lez, Chain::Zcash, 1_000, 1_200, 100).expect("safe margin"),
    )
    .expect("Zcash recovery schedule");
    SwapCoordinator::new_with_direction(
        SwapId::new(id).expect("swap ID"),
        Pair::Zcash,
        direction,
        ConfirmationPolicy::new(2).expect("confirmations"),
        schedule,
    )
}

#[cfg(unix)]
fn make_owner_private(path: &std::path::Path) {
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .expect("make database owner-private");
}

#[cfg(not(unix))]
fn make_owner_private(_path: &std::path::Path) {}
