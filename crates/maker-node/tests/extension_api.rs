//! An external client publishes only the terms it reviewed, even across retries.

use lez_maker_node::{MakerRpc, rpc_module};
use lez_swap_store::SqliteSwapStore;
use serde_json::{Value, json};
use tempfile::tempdir;

fn route() -> Value {
    json!({"pair": "Bitcoin", "direction": "TakerSellsForeign"})
}

fn save(id: &str, revision: Option<u64>, price: u64) -> Value {
    let (numerator, denominator) = if price == 2 { (1, 500) } else { (price, 1000) };
    json!({
        "request_id": id,
        "expected_pair_revision": revision,
        "expected_price_revision": revision,
        "configuration": {
            "route": route(), "enabled": true, "price_source": "local",
            "minimum_foreign_units": 1000, "maximum_foreign_units": 1_000_000,
            "offer_ttl_seconds": 300
        },
        "price": {"route": route(), "lez_units_per_lot": numerator, "foreign_units_per_lot": denominator}
    })
}

fn publish(id: &str, offer: &str, pair_revision: u64, price_revision: u64) -> Value {
    json!({
        "schema_version": 1, "request_id": id, "offer_id": offer, "route": route(),
        "expected_pair_revision": pair_revision, "expected_price_revision": price_revision
    })
}

async fn error(module: &jsonrpsee::RpcModule<MakerRpc>, method: &str, params: Value) -> i64 {
    let envelope = json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": [params]});
    let (response, _) = module
        .raw_json_request(&envelope.to_string(), 1)
        .await
        .unwrap();
    let response: Value = serde_json::from_str(response.get()).unwrap();
    response["error"]["code"]
        .as_i64()
        .unwrap_or_else(|| panic!("expected error: {response}"))
}

#[tokio::test]
#[allow(clippy::too_many_lines)] // One persisted external-client journey across restart.
async fn stale_terms_never_publish_and_replay_survives_restart_and_config_changes() {
    let run = tempdir().unwrap();
    let path = run.path().join("maker.sqlite3");
    let module = rpc_module(MakerRpc::new(SqliteSwapStore::open(&path).unwrap())).unwrap();
    let _: Value = module
        .call("maker_local_route_save_v1", [save("route-first", None, 1)])
        .await
        .unwrap();
    // A second client updates both rows after the first client reviewed them.
    let _: Value = module
        .call(
            "maker_local_route_save_v1",
            [save("route-second", Some(1), 2)],
        )
        .await
        .unwrap();
    for (pair, price) in [(1, 2), (2, 1)] {
        assert_eq!(
            error(
                &module,
                "maker_offer_publish_v1",
                publish("stale-request", "stale-offer", pair, price)
            )
            .await,
            -32009
        );
    }
    let offers: Value = module.call("maker_offer_list", [json!({})]).await.unwrap();
    assert_eq!(offers, json!([]));

    let request = publish("strategy-publish", "strategy-offer", 2, 2);
    let commit: Value = module
        .call("maker_offer_publish_v1", [request.clone()])
        .await
        .unwrap();
    assert_eq!(commit, json!({"revision": 1, "was_replay": false}));
    let _: Value = module
        .call(
            "maker_local_route_save_v1",
            [save("route-third", Some(2), 3)],
        )
        .await
        .unwrap();
    drop(module);

    let module = rpc_module(MakerRpc::new(SqliteSwapStore::open(&path).unwrap())).unwrap();
    let replay: Value = module
        .call("maker_offer_publish_v1", [request])
        .await
        .unwrap();
    assert_eq!(replay, json!({"revision": 1, "was_replay": true}));
    let offers: Value = module.call("maker_offer_list", [json!({})]).await.unwrap();
    assert_eq!(offers.as_array().unwrap().len(), 1);
    assert_eq!(offers[0]["offer"]["price"]["lez_units_per_lot"], 1);
    assert_eq!(offers[0]["offer"]["price"]["foreign_units_per_lot"], 500);
    assert_eq!(offers[0]["offer"]["pair_configuration_revision"], 2);
    assert_eq!(offers[0]["offer"]["price_source_revision"], 2);

    // Neither new guards nor the legacy method can reinterpret this request ID.
    assert_eq!(
        error(
            &module,
            "maker_offer_publish_v1",
            publish("strategy-publish", "strategy-offer", 3, 3)
        )
        .await,
        -32009
    );
    assert_eq!(
        error(
            &module,
            "maker_offer_publish",
            json!({
                "request_id": "strategy-publish", "offer_id": "strategy-offer", "route": route()
            })
        )
        .await,
        -32009
    );
    let legacy =
        json!({"request_id": "legacy-publish", "offer_id": "legacy-offer", "route": route()});
    let _: Value = module.call("maker_offer_publish", [legacy]).await.unwrap();
    assert_eq!(
        error(
            &module,
            "maker_offer_publish_v1",
            publish("legacy-publish", "legacy-offer", 3, 3)
        )
        .await,
        -32009
    );

    // Withdrawal remains the normal one-winner transition, not an in-place edit.
    let withdraw = json!({"request_id": "strategy-withdraw", "offer_id": "strategy-offer", "expected_revision": 1});
    let _: Value = module
        .call("maker_offer_withdraw", [withdraw])
        .await
        .unwrap();
    let replay: Value = module
        .call(
            "maker_offer_publish_v1",
            [publish("strategy-publish", "strategy-offer", 2, 2)],
        )
        .await
        .unwrap();
    assert_eq!(replay["was_replay"], true);
    let offers: Value = module.call("maker_offer_list", [json!({})]).await.unwrap();
    let offer = offers
        .as_array()
        .unwrap()
        .iter()
        .find(|o| o["offer"]["id"] == "strategy-offer")
        .unwrap();
    assert_eq!(offer["status"], "withdrawn");
}

#[tokio::test]
async fn documented_requests_work_after_migrating_an_existing_mutation_journal() {
    let run = tempdir().unwrap();
    let path = run.path().join("maker.sqlite3");
    let module = rpc_module(MakerRpc::new(SqliteSwapStore::open(&path).unwrap())).unwrap();
    let reference = include_str!("../../../docs/api/README.md");
    let requests: Vec<Value> = reference
        .split("```json\n")
        .skip(1)
        .map(|block| serde_json::from_str(block.split("```").next().unwrap()).unwrap())
        .collect();
    let save = requests[0]["params"][0].clone();
    let _: Value = module
        .call("maker_local_route_save_v1", [save.clone()])
        .await
        .unwrap();
    drop(module);

    // Recreate the prior release's CHECK constraint while retaining its rows
    // and database version. Opening this database must upgrade it transactionally.
    let connection = rusqlite::Connection::open(&path).unwrap();
    let schema: String = connection
        .query_row(
            "SELECT sql FROM sqlite_master WHERE name = 'maker_application_mutations'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let old_schema = schema.replace("'offer_publish_at_revisions_v1', ", "");
    assert_ne!(schema, old_schema);
    connection
        .execute_batch(&format!(
            "BEGIN IMMEDIATE;
         ALTER TABLE maker_application_mutations RENAME TO previous_journal;
         {old_schema};
         INSERT INTO maker_application_mutations SELECT * FROM previous_journal;
         DROP TABLE previous_journal;
         COMMIT;"
        ))
        .unwrap();
    drop(connection);

    let module = rpc_module(MakerRpc::new(SqliteSwapStore::open(&path).unwrap())).unwrap();
    let replay: Value = module
        .call("maker_local_route_save_v1", [save])
        .await
        .unwrap();
    assert_eq!(
        replay,
        json!({"pair_revision": 1, "price_revision": 1, "was_replay": true})
    );
    for request in &requests[1..3] {
        let result: Value = module
            .call(
                request["method"].as_str().unwrap(),
                [request["params"][0].clone()],
            )
            .await
            .unwrap();
        assert_eq!(result["was_replay"], false);
    }
    // The taker example is a documented input only; do not execute a chain action.
    let _: lez_taker_node::TakerClaimRequestV1 =
        serde_json::from_value(requests[3]["params"][0].clone()).unwrap();
    drop(module);
    let module = rpc_module(MakerRpc::new(SqliteSwapStore::open(&path).unwrap())).unwrap();
    let replay: Value = module
        .call("maker_offer_publish_v1", [requests[1]["params"][0].clone()])
        .await
        .unwrap();
    assert_eq!(replay, json!({"revision": 1, "was_replay": true}));
}

#[tokio::test]
async fn guarded_publish_rejects_unknown_fields_versions_and_zero_guards() {
    let run = tempdir().unwrap();
    let module = rpc_module(MakerRpc::new(
        SqliteSwapStore::open(run.path().join("maker.sqlite3")).unwrap(),
    ))
    .unwrap();
    for (field, value) in [
        ("schema_version", json!(2)),
        ("expected_pair_revision", json!(0)),
        ("expected_price_revision", json!(0)),
        ("private_key_path", json!("/not/accepted")),
    ] {
        let mut params = publish("invalid-publish", "invalid-offer", 1, 1);
        params[field] = value;
        assert_eq!(
            error(&module, "maker_offer_publish_v1", params).await,
            -32602
        );
    }
}

#[tokio::test]
async fn guarded_offers_use_signed_delivery_and_replay_cannot_republish_withdrawn_terms() {
    use lez_maker_node::RunLocalDelivery;
    use lez_swap_store::MakerOfferRecordV1;
    use secp256k1::SecretKey;
    use std::time::{SystemTime, UNIX_EPOCH};

    let run = tempdir().unwrap();
    let key = SecretKey::from_slice(&[41; 32]).unwrap();
    let directory = run.path().join("delivery");
    let delivery = RunLocalDelivery::publisher(&directory, key).unwrap();
    let observer = RunLocalDelivery::publisher(&directory, key).unwrap();
    let store = SqliteSwapStore::open(run.path().join("maker.sqlite3")).unwrap();
    let module = rpc_module(MakerRpc::with_delivery_transport(store, delivery, key)).unwrap();
    let _: Value = module
        .call(
            "maker_local_route_save_v1",
            [save("delivery-route", None, 1)],
        )
        .await
        .unwrap();
    let request = publish("delivery-publish", "delivery-offer", 1, 1);
    let _: Value = module
        .call("maker_offer_publish_v1", [request.clone()])
        .await
        .unwrap();
    let offers: Vec<MakerOfferRecordV1> =
        module.call("maker_offer_list", [json!({})]).await.unwrap();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    observer
        .projection_health(&[offers[0].offer().clone()], now)
        .unwrap();
    let _: Value = module.call("maker_offer_withdraw", [json!({
        "request_id": "delivery-withdraw", "offer_id": "delivery-offer", "expected_revision": 1
    })]).await.unwrap();
    let _: Value = module
        .call("maker_offer_publish_v1", [request])
        .await
        .unwrap();
    observer.projection_health(&[], now).unwrap();
}
