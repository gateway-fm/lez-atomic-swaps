//! Contract tests for the atomic Maker UI route-save boundary.

use lez_bridge_protocol::RequestId;
use lez_maker_node::{ListRequest, LocalRouteSaveRequest, MakerRpc, rpc_module};
use lez_swap_core::{Pair, SwapDirection};
use lez_swap_store::{
    LocalPriceV1, MakerLocalRouteCommit, MakerPairConfigurationV1, MakerPriceSourceKind,
    MakerRouteV1, SqliteSwapStore, VersionedMakerRecord,
};
use serde_json::json;
use tempfile::tempdir;

fn request(
    request_id: &str,
    route: MakerRouteV1,
    expected_pair_revision: Option<u64>,
    expected_price_revision: Option<u64>,
) -> LocalRouteSaveRequest {
    LocalRouteSaveRequest {
        request_id: RequestId::new(request_id).expect("valid request ID"),
        expected_pair_revision,
        expected_price_revision,
        configuration: MakerPairConfigurationV1::new(
            route,
            true,
            MakerPriceSourceKind::Local,
            10,
            10_000,
            300,
        )
        .expect("valid policy"),
        price: LocalPriceV1::new(route, 5, 2).expect("valid price"),
    }
}

#[tokio::test]
async fn owner_rpc_saves_and_replays_one_enabled_local_route_atomically() {
    let run = tempdir().expect("isolated store");
    let store = SqliteSwapStore::open(run.path().join("maker.sqlite3")).expect("open store");
    let module = rpc_module(MakerRpc::new(store)).expect("build owner RPC");
    assert!(
        module
            .method_names()
            .any(|method| method == "maker_local_route_save_v1")
    );
    let route =
        MakerRouteV1::new(Pair::Zcash, SwapDirection::TakerSellsLez).expect("supported route");

    let committed: MakerLocalRouteCommit = module
        .call(
            "maker_local_route_save_v1",
            [request("m6-route-rpc-001", route, None, None)],
        )
        .await
        .expect("commit route");
    assert_eq!(committed.pair_revision(), 1);
    assert_eq!(committed.price_revision(), 1);
    assert!(!committed.was_replay());

    let replay: MakerLocalRouteCommit = module
        .call(
            "maker_local_route_save_v1",
            [request("m6-route-rpc-001", route, None, None)],
        )
        .await
        .expect("replay route");
    assert!(replay.was_replay());

    let mut params = serde_json::to_value(request(
        "m6-route-rpc-unknown-field",
        route,
        Some(1),
        Some(1),
    ))
    .expect("serialize request");
    params
        .as_object_mut()
        .expect("request is an object")
        .insert("private_key_path".into(), json!("/must/not/be/accepted"));
    let envelope = json!({
        "jsonrpc": "2.0",
        "id": 7,
        "method": "maker_local_route_save_v1",
        "params": [params],
    });
    let (response, _) = module
        .raw_json_request(&envelope.to_string(), 1)
        .await
        .expect("well-formed JSON-RPC");
    assert!(response.get().contains("\"code\":-32602"));

    let pairs: Vec<VersionedMakerRecord<MakerPairConfigurationV1>> = module
        .call("maker_pair_list", [ListRequest::default()])
        .await
        .expect("list pair policies");
    let prices: Vec<VersionedMakerRecord<LocalPriceV1>> = module
        .call("maker_local_price_list", [ListRequest::default()])
        .await
        .expect("list local prices");
    assert_eq!(pairs.len(), 1);
    assert!(pairs[0].value().enabled());
    assert_eq!(prices.len(), 1);
    assert_eq!(pairs[0].revision(), 1);
    assert_eq!(prices[0].revision(), 1);
    assert_eq!(prices[0].value().route(), pairs[0].value().route());
}
