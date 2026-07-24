use lez_bridge_protocol::RequestId;
use lez_swap_core::{
    Chain, ChainPosition, ConfirmationPolicy, Pair, RecoverySchedule, SwapCoordinator,
    SwapDirection, SwapId, TimelockSafety,
};
use lez_swap_store::{
    LocalPriceV1, MakerOfferId, MakerOfferStatus, MakerPairConfigurationV1, MakerPriceSourceKind,
    MakerRouteV1, SqliteSwapStore, StoreError,
};
use rusqlite::Connection;
use tempfile::tempdir;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt as _;

fn request(value: &str) -> RequestId {
    RequestId::new(value).expect("bounded request ID")
}

fn offer(value: &str) -> MakerOfferId {
    MakerOfferId::new(value).expect("bounded offer ID")
}

fn zec_route() -> MakerRouteV1 {
    MakerRouteV1::new(Pair::Zcash, SwapDirection::TakerSellsLez).unwrap()
}

fn zec_swap(id: &str) -> SwapCoordinator {
    let direction = SwapDirection::TakerSellsLez;
    SwapCoordinator::new_with_direction(
        SwapId::new(id).unwrap(),
        Pair::Zcash,
        direction,
        ConfirmationPolicy::new(2).unwrap(),
        RecoverySchedule::new(
            Pair::Zcash,
            direction,
            ChainPosition::block_height(Chain::Zcash, 100),
            ChainPosition::block_height(Chain::Lez, 120),
            TimelockSafety::between(Chain::Lez, Chain::Zcash, 1_000, 1_200, 100).unwrap(),
        )
        .unwrap(),
    )
}

fn bitcoin_swap(id: &str) -> SwapCoordinator {
    let direction = SwapDirection::TakerSellsForeign;
    SwapCoordinator::new_with_direction(
        SwapId::new(id).unwrap(),
        Pair::Bitcoin,
        direction,
        ConfirmationPolicy::new(2).unwrap(),
        RecoverySchedule::new(
            Pair::Bitcoin,
            direction,
            ChainPosition::block_height(Chain::Lez, 100),
            ChainPosition::block_height(Chain::Bitcoin, 120),
            TimelockSafety::between(Chain::Lez, Chain::Bitcoin, 1_000, 1_200, 100).unwrap(),
        )
        .unwrap(),
    )
}

fn configure_local_route(store: &mut SqliteSwapStore) {
    let route = zec_route();
    let disabled =
        MakerPairConfigurationV1::new(route, false, MakerPriceSourceKind::Local, 10, 10_000, 300)
            .unwrap();
    store
        .configure_maker_pair(&request("offers-pair-create-001"), None, &disabled)
        .unwrap();
    store
        .set_local_price(
            &request("offers-price-create-001"),
            None,
            &LocalPriceV1::new(route, 5, 2).unwrap(),
        )
        .unwrap();
    let enabled =
        MakerPairConfigurationV1::new(route, true, MakerPriceSourceKind::Local, 10, 10_000, 300)
            .unwrap();
    store
        .configure_maker_pair(&request("offers-pair-enable-001"), Some(1), &enabled)
        .unwrap();
}

#[test]
fn publication_snapshots_exact_policy_and_price_and_survives_restart() {
    let run = tempdir().expect("isolated offer store");
    let database = run.path().join("offers.sqlite3");
    let mut store = SqliteSwapStore::open(&database).unwrap();
    configure_local_route(&mut store);
    let id = offer("offer-zec-local-001");

    let published = store
        .publish_local_offer(&request("offer-publish-zec-001"), &id, zec_route(), 1_000)
        .unwrap();
    assert_eq!(published.revision(), 1);
    assert!(!published.was_replay());
    let replay = store
        .publish_local_offer(&request("offer-publish-zec-001"), &id, zec_route(), 1_000)
        .unwrap();
    assert_eq!(replay.revision(), 1);
    assert!(replay.was_replay());

    store
        .set_local_price(
            &request("offers-price-update-001"),
            Some(1),
            &LocalPriceV1::new(zec_route(), 7, 3).unwrap(),
        )
        .unwrap();
    drop(store);

    let store = SqliteSwapStore::open(&database).unwrap();
    let records = store.list_discoverable_maker_offers(1_299).unwrap();
    assert_eq!(records.len(), 1);
    let record = &records[0];
    assert_eq!(record.revision(), 1);
    assert_eq!(record.status(), MakerOfferStatus::Active);
    assert_eq!(record.offer().id(), &id);
    assert_eq!(record.offer().pair_configuration_revision(), 2);
    assert_eq!(record.offer().price_source_revision(), 1);
    assert_eq!(record.offer().price().lez_units_per_lot(), 5);
    assert_eq!(record.offer().price().foreign_units_per_lot(), 2);
    assert_eq!(record.offer().price_observed_at_unix_seconds(), 1_000);
    assert_eq!(record.offer().created_at_unix_seconds(), 1_000);
    assert_eq!(record.offer().expires_at_unix_seconds(), 1_300);
}

#[test]
fn reservation_is_one_winner_and_consumption_survives_offer_expiry() {
    let run = tempdir().expect("isolated offer store");
    let mut store = SqliteSwapStore::open(run.path().join("winner.sqlite3")).unwrap();
    configure_local_route(&mut store);
    let id = offer("offer-zec-winner-001");
    store
        .publish_local_offer(&request("offer-winner-publish-001"), &id, zec_route(), 100)
        .unwrap();
    let winning_reservation = request("offer-reservation-win-001");
    let reserved = store
        .reserve_maker_offer(
            &request("offer-reserve-win-001"),
            &id,
            1,
            &winning_reservation,
            399,
        )
        .unwrap();
    assert_eq!(reserved.revision(), 2);

    assert!(matches!(
        store.reserve_maker_offer(
            &request("offer-reserve-lose-001"),
            &id,
            1,
            &request("offer-reservation-lose-001"),
            399,
        ),
        Err(StoreError::StaleMakerOffer {
            expected: 1,
            actual: 2
        })
    ));
    let failed_swap = bitcoin_swap("offer-swap-wrong-001");
    assert!(matches!(
        store.consume_maker_offer(
            &request("offer-consume-win-001"),
            &id,
            2,
            &winning_reservation,
            &failed_swap,
        ),
        Err(StoreError::MakerOfferSwapMismatch)
    ));
    assert_eq!(store.load(failed_swap.id()).unwrap(), None);
    assert_eq!(
        store.list_maker_offer_history(401).unwrap()[0].revision(),
        2
    );

    let swap = zec_swap("offer-swap-zec-001");
    let consumed = store
        .consume_maker_offer(
            &request("offer-consume-win-001"),
            &id,
            2,
            &winning_reservation,
            &swap,
        )
        .unwrap();
    assert_eq!(consumed.revision(), 3);
    assert_eq!(store.load(swap.id()).unwrap(), Some(swap));

    let replay_after_later_transition = store
        .reserve_maker_offer(
            &request("offer-reserve-win-001"),
            &id,
            1,
            &winning_reservation,
            399,
        )
        .unwrap();
    assert_eq!(replay_after_later_transition.revision(), 2);
    assert!(replay_after_later_transition.was_replay());
    let history = store.list_maker_offer_history(401).unwrap();
    assert_eq!(history[0].status(), MakerOfferStatus::Consumed);
    assert_eq!(history[0].reservation_id(), Some(&winning_reservation));
    assert_eq!(history[0].swap_id(), Some("offer-swap-zec-001"));
}

#[test]
fn expiry_is_half_open_and_failed_reservation_consumes_no_request_identity() {
    let run = tempdir().expect("isolated offer store");
    let mut store = SqliteSwapStore::open(run.path().join("expiry.sqlite3")).unwrap();
    configure_local_route(&mut store);
    let id = offer("offer-zec-expiry-001");
    store
        .publish_local_offer(&request("offer-expiry-publish-001"), &id, zec_route(), 100)
        .unwrap();
    assert_eq!(store.list_discoverable_maker_offers(399).unwrap().len(), 1);
    assert!(
        store
            .list_discoverable_maker_offers(400)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        store.list_maker_offer_history(400).unwrap()[0].status(),
        MakerOfferStatus::Expired
    );

    let reusable_request = request("offer-expired-transition-001");
    assert!(matches!(
        store.reserve_maker_offer(
            &reusable_request,
            &id,
            1,
            &request("offer-expired-reservation-001"),
            400,
        ),
        Err(StoreError::MakerOfferExpired)
    ));
    let withdrawn = store
        .withdraw_maker_offer(&reusable_request, &id, 1)
        .expect("failed reserve rolled back request ledger and revision");
    assert_eq!(withdrawn.revision(), 2);
    assert_eq!(
        store.list_maker_offer_history(400).unwrap()[0].status(),
        MakerOfferStatus::Withdrawn
    );
}

#[test]
fn maker_request_ids_are_global_across_configuration_and_offer_operations() {
    let run = tempdir().expect("isolated offer store");
    let mut store = SqliteSwapStore::open(run.path().join("global-request.sqlite3")).unwrap();
    configure_local_route(&mut store);
    assert!(matches!(
        store.publish_local_offer(
            &request("offers-pair-create-001"),
            &offer("offer-zec-conflict-001"),
            zec_route(),
            100,
        ),
        Err(StoreError::MakerOfferRequestConflict)
    ));
    assert!(store.list_maker_offer_history(100).unwrap().is_empty());
}

#[test]
fn schema_v11_migrates_the_global_request_ledger_without_reuse() {
    let run = tempdir().expect("isolated migration store");
    let database = run.path().join("schema-v11.sqlite3");
    let connection = Connection::open(&database).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE swaps (
                 id TEXT PRIMARY KEY NOT NULL,
                 schema_version INTEGER NOT NULL,
                 state_json TEXT NOT NULL,
                 revision INTEGER NOT NULL DEFAULT 0 CHECK (revision >= 0)
             ) STRICT;
             CREATE TABLE maker_configuration_mutations (
                 sequence INTEGER PRIMARY KEY AUTOINCREMENT,
                 request_id TEXT NOT NULL UNIQUE,
                 operation TEXT NOT NULL,
                 request_payload_version INTEGER NOT NULL,
                 request_json TEXT NOT NULL,
                 result_json TEXT NOT NULL
             ) STRICT;
             INSERT INTO maker_configuration_mutations (
                 request_id, operation, request_payload_version, request_json, result_json
             ) VALUES (
                 'legacy-global-id-001', 'pair_configure', 1, '{}',
                 '{\"schema_version\":1,\"revision\":1}'
             );
             PRAGMA user_version = 11;",
        )
        .unwrap();
    drop(connection);
    make_owner_private(&database);

    let mut store = SqliteSwapStore::open(&database).unwrap();
    assert!(matches!(
        store.publish_local_offer(
            &request("legacy-global-id-001"),
            &offer("offer-after-v11-001"),
            zec_route(),
            100,
        ),
        Err(StoreError::MakerOfferRequestConflict)
    ));
    drop(store);

    let connection = Connection::open(&database).unwrap();
    let version: i64 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    let migrated_rows: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM maker_application_mutations",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let legacy_exists: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'maker_configuration_mutations')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(version, 12);
    assert_eq!(migrated_rows, 1);
    assert!(!legacy_exists);
}

#[cfg(unix)]
fn make_owner_private(path: &std::path::Path) {
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
}

#[cfg(not(unix))]
fn make_owner_private(_path: &std::path::Path) {}
