//! Direct JSON-RPC contract for the implemented read-only Taker facade.

use std::{collections::BTreeSet, fs};

use jsonrpsee::RpcModule;
use lez_bridge_protocol::RequestId;
use lez_maker_node::{
    DeliveryPublicationV1, RunLocalDelivery, TakerDependencyProbe, TakerDependencyStateV1,
    TakerFacadeBackend, TakerHealthRequestV1, TakerHealthV1, TakerOfferListRequestV1,
    TakerOfferListV1, TakerTrustedTimeSource, taker_read_only_rpc_module,
};
use lez_swap_core::{Pair, SwapDirection};
use lez_swap_sdk_core::OfferDiscovery as _;
use lez_swap_store::{
    LocalPriceV1, MakerOfferId, MakerPairConfigurationV1, MakerPriceSourceKind, MakerRouteV1,
    SqliteSwapStore,
};
use secp256k1::{PublicKey, Secp256k1, SecretKey};
use serde_json::{Value, json};
use tempfile::TempDir;

const NOW: u64 = 1_001;
const INVALID_PARAMS: i64 = -32_602;
const METHOD_NOT_FOUND: i64 = -32_601;
const DEPENDENCY_UNAVAILABLE: i64 = -32_010;
const RESULT_LIMIT_EXCEEDED: i64 = -32_011;
const AUTHENTICATED_OFFER_CONFLICT: i64 = -32_012;

#[derive(Clone, Copy)]
struct FixedClock(Option<u64>);

impl TakerTrustedTimeSource for FixedClock {
    fn now_unix_seconds(&self) -> Option<u64> {
        self.0
    }
}

#[derive(Clone, Copy)]
struct FixedProbe(bool);

impl TakerDependencyProbe for FixedProbe {
    fn is_available(&self) -> bool {
        self.0
    }
}

type TestBackend = TakerFacadeBackend<FixedClock, FixedProbe>;

#[tokio::test]
async fn module_registers_exactly_two_read_only_methods_and_no_mutations() {
    let (_run, module, _, _) = signed_module(16).await;
    let methods = module.method_names().collect::<BTreeSet<_>>();
    assert_eq!(
        methods,
        BTreeSet::from(["taker_health", "taker_offer_list_v1"])
    );

    for method in [
        "taker_swap_list_v1",
        "taker_swap_initiate_v1",
        "taker_swap_monitor_v1",
        "taker_swap_claim_v1",
        "taker_swap_refund_v1",
    ] {
        let response = rpc_response(&module, method, json!([{"schema_version": 1}])).await;
        assert_eq!(response["error"]["code"], METHOD_NOT_FOUND, "{response}");
    }
}

#[tokio::test]
async fn health_and_offer_list_use_the_real_authenticated_delivery_backend() {
    let (_run, module, expected_offer, commitment) = signed_module(16).await;

    let health: TakerHealthV1 = module
        .call("taker_health", [TakerHealthRequestV1 { schema_version: 1 }])
        .await
        .unwrap();
    assert!(health.is_ready());
    assert_eq!(health.delivery(), TakerDependencyStateV1::Available);
    assert_eq!(health.chat(), TakerDependencyStateV1::Available);

    let listed: TakerOfferListV1 = module
        .call(
            "taker_offer_list_v1",
            [TakerOfferListRequestV1 {
                schema_version: 1,
                route: Some(zec_route()),
            }],
        )
        .await
        .unwrap();
    assert_eq!(listed.offers.len(), 1);
    assert_eq!(listed.offers[0].offer, expected_offer);
    assert_eq!(listed.offers[0].signed_envelope_sha256, commitment);
}

#[tokio::test]
async fn params_schema_and_route_fail_with_fixed_invalid_request_categories() {
    let (_run, module, _, _) = signed_module(16).await;

    let unknown = rpc_response(
        &module,
        "taker_health",
        json!([{"schema_version": 1, "private_key_path": "/must/not/be-accepted"}]),
    )
    .await;
    assert_rpc_error(&unknown, INVALID_PARAMS, "Invalid params", "invalid_params");

    let version = rpc_response(
        &module,
        "taker_offer_list_v1",
        json!([{"schema_version": 2, "route": null}]),
    )
    .await;
    assert_rpc_error(
        &version,
        INVALID_PARAMS,
        "Invalid params",
        "unsupported_schema_version",
    );

    let unsupported_route = MakerRouteV1::new(Pair::Bitcoin, SwapDirection::TakerSellsLez).unwrap();
    let unsupported = rpc_response(
        &module,
        "taker_offer_list_v1",
        json!([{"schema_version": 1, "route": unsupported_route}]),
    )
    .await;
    assert_rpc_error(
        &unsupported,
        INVALID_PARAMS,
        "Invalid params",
        "unsupported_route",
    );
}

#[tokio::test]
async fn backend_failures_have_fixed_codes_categories_and_no_private_details() {
    let unavailable_backend =
        TakerFacadeBackend::new(Vec::new(), FixedClock(None), None::<FixedProbe>, 16).unwrap();
    let unavailable_module = taker_read_only_rpc_module(unavailable_backend).unwrap();
    let trusted_time = rpc_response(
        &unavailable_module,
        "taker_offer_list_v1",
        json!([{"schema_version": 1, "route": null}]),
    )
    .await;
    assert_rpc_error(
        &trusted_time,
        DEPENDENCY_UNAVAILABLE,
        "Taker dependency unavailable",
        "trusted_time_unavailable",
    );

    let (run, delivery_module, _, _) = signed_module(16).await;
    fs::write(
        run.path().join("delivery/m6-rpc-zec-001.offer.json"),
        b"tampered",
    )
    .unwrap();
    let delivery = rpc_response(
        &delivery_module,
        "taker_offer_list_v1",
        json!([{"schema_version": 1, "route": null}]),
    )
    .await;
    assert_rpc_error(
        &delivery,
        DEPENDENCY_UNAVAILABLE,
        "Taker dependency unavailable",
        "authenticated_delivery_unavailable",
    );

    let (_bounded_run, bounded_module) = two_offer_module(1).await;
    let limited = rpc_response(
        &bounded_module,
        "taker_offer_list_v1",
        json!([{"schema_version": 1, "route": null}]),
    )
    .await;
    assert_rpc_error(
        &limited,
        RESULT_LIMIT_EXCEEDED,
        "Taker result limit exceeded",
        "offer_limit_exceeded",
    );

    let (_conflict_run, conflict_module) = conflicting_offer_module().await;
    let conflict = rpc_response(
        &conflict_module,
        "taker_offer_list_v1",
        json!([{"schema_version": 1, "route": null}]),
    )
    .await;
    assert_rpc_error(
        &conflict,
        AUTHENTICATED_OFFER_CONFLICT,
        "Authenticated offer conflict",
        "conflicting_authenticated_offer",
    );

    for response in [trusted_time, delivery, limited, conflict] {
        let wire = response.to_string().to_ascii_lowercase();
        for forbidden in ["/", "path", "file", "socket", "endpoint", "credential"] {
            assert!(!wire.contains(forbidden), "{wire}");
        }
    }
}

async fn signed_module(
    maximum_offers: usize,
) -> (
    TempDir,
    RpcModule<TestBackend>,
    lez_swap_store::MakerOfferV1,
    [u8; 32],
) {
    let run = tempfile::tempdir().unwrap();
    let delivery_root = run.path().join("delivery");
    let key = signing_key(21);
    let maker = PublicKey::from_secret_key(&Secp256k1::signing_only(), &key);
    let publisher = RunLocalDelivery::publisher(&delivery_root, key).unwrap();
    let expected = offer("m6-rpc-zec-001", zec_route(), 5);
    let authenticated = publisher
        .publish(DeliveryPublicationV1::new(expected.clone(), 1_000))
        .await
        .unwrap();
    let backend = TakerFacadeBackend::new(
        vec![RunLocalDelivery::subscriber(&delivery_root, maker).unwrap()],
        FixedClock(Some(NOW)),
        Some(FixedProbe(true)),
        maximum_offers,
    )
    .unwrap();
    let module = taker_read_only_rpc_module(backend).unwrap();
    (run, module, expected, authenticated.commitment())
}

async fn two_offer_module(maximum_offers: usize) -> (TempDir, RpcModule<TestBackend>) {
    let run = tempfile::tempdir().unwrap();
    let delivery_root = run.path().join("delivery");
    let key = signing_key(22);
    let maker = PublicKey::from_secret_key(&Secp256k1::signing_only(), &key);
    let publisher = RunLocalDelivery::publisher(&delivery_root, key).unwrap();
    for (id, numerator) in [("m6-rpc-bound-001", 5), ("m6-rpc-bound-002", 7)] {
        publisher
            .publish(DeliveryPublicationV1::new(
                offer(id, zec_route(), numerator),
                1_000,
            ))
            .await
            .unwrap();
    }
    let backend = TakerFacadeBackend::new(
        vec![RunLocalDelivery::subscriber(&delivery_root, maker).unwrap()],
        FixedClock(Some(NOW)),
        None::<FixedProbe>,
        maximum_offers,
    )
    .unwrap();
    (run, taker_read_only_rpc_module(backend).unwrap())
}

async fn conflicting_offer_module() -> (TempDir, RpcModule<TestBackend>) {
    let run = tempfile::tempdir().unwrap();
    let first_root = run.path().join("first");
    let second_root = run.path().join("second");
    let key = signing_key(23);
    let maker = PublicKey::from_secret_key(&Secp256k1::signing_only(), &key);
    let first = RunLocalDelivery::publisher(&first_root, key).unwrap();
    let second = RunLocalDelivery::publisher(&second_root, key).unwrap();
    first
        .publish(DeliveryPublicationV1::new(
            offer("m6-rpc-conflict-001", zec_route(), 5),
            1_000,
        ))
        .await
        .unwrap();
    second
        .publish(DeliveryPublicationV1::new(
            offer("m6-rpc-conflict-001", zec_route(), 7),
            1_000,
        ))
        .await
        .unwrap();
    let backend = TakerFacadeBackend::new(
        vec![
            RunLocalDelivery::subscriber(&first_root, maker).unwrap(),
            RunLocalDelivery::subscriber(&second_root, maker).unwrap(),
        ],
        FixedClock(Some(NOW)),
        None::<FixedProbe>,
        16,
    )
    .unwrap();
    (run, taker_read_only_rpc_module(backend).unwrap())
}

async fn rpc_response<Context: Send + Sync + 'static>(
    module: &RpcModule<Context>,
    method: &str,
    params: Value,
) -> Value {
    let request = json!({
        "jsonrpc": "2.0",
        "id": 7,
        "method": method,
        "params": params,
    });
    let (response, _) = module
        .raw_json_request(&request.to_string(), 1)
        .await
        .unwrap();
    serde_json::from_str(response.get()).unwrap()
}

fn assert_rpc_error(response: &Value, code: i64, message: &str, category: &str) {
    assert_eq!(response["error"]["code"], code, "{response}");
    assert_eq!(response["error"]["message"], message, "{response}");
    assert_eq!(
        response["error"]["data"]["category"], category,
        "{response}"
    );
}

fn offer(id: &str, route: MakerRouteV1, price_numerator: u64) -> lez_swap_store::MakerOfferV1 {
    let run = tempfile::tempdir().unwrap();
    let mut store = SqliteSwapStore::open(run.path().join("offer.sqlite3")).unwrap();
    let disabled =
        MakerPairConfigurationV1::new(route, false, MakerPriceSourceKind::Local, 1, 10_000, 300)
            .unwrap();
    store
        .configure_maker_pair(&request(&format!("pair-create-{id}")), None, &disabled)
        .unwrap();
    store
        .set_local_price(
            &request(&format!("price-create-{id}")),
            None,
            &LocalPriceV1::new(route, price_numerator, 2).unwrap(),
        )
        .unwrap();
    let enabled =
        MakerPairConfigurationV1::new(route, true, MakerPriceSourceKind::Local, 1, 10_000, 300)
            .unwrap();
    store
        .configure_maker_pair(&request(&format!("pair-enable-{id}")), Some(1), &enabled)
        .unwrap();
    store
        .publish_local_offer(
            &request(&format!("offer-publish-{id}")),
            &MakerOfferId::new(id).unwrap(),
            route,
            1_000,
        )
        .unwrap();
    store.list_discoverable_maker_offers(1_000).unwrap()[0]
        .offer()
        .clone()
}

fn request(value: &str) -> RequestId {
    RequestId::new(value).unwrap()
}

fn signing_key(byte: u8) -> SecretKey {
    SecretKey::from_slice(&[byte; 32]).unwrap()
}

fn zec_route() -> MakerRouteV1 {
    MakerRouteV1::new(Pair::Zcash, SwapDirection::TakerSellsLez).unwrap()
}
