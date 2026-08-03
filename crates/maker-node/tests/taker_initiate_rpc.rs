//! Direct RED contract for service-wired, admission-only Taker initiation.

use std::{
    collections::BTreeSet,
    fs,
    os::unix::fs::PermissionsExt as _,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use jsonrpsee::RpcModule;
use lez_bridge_protocol::RequestId;
use lez_maker_node::{
    DeliveryPublicationV1, RunLocalDelivery, TakerHealthRequestV1, TakerHealthV1,
    TakerInitiationCommitV1, TakerMakerIdentityV1, TakerSwapInitiateRequestV1, TakerSwapStateV1,
    load_taker_service_context, taker_service_rpc_module,
};
use lez_swap_core::{Pair, SwapDirection};
use lez_swap_store::{
    LocalPriceV1, MakerOfferId, MakerPairConfigurationV1, MakerPriceSourceKind, MakerRouteV1,
    SqliteSwapStore, SqliteTakerFacadeStore,
};
use secp256k1::{PublicKey, Secp256k1, SecretKey};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};

const INVALID_PARAMS: i64 = -32_602;
const INITIATION_CONFLICT: i64 = -32_013;

#[tokio::test]
#[allow(clippy::too_many_lines)] // One user-visible journey keeps replay-before-live ordering auditable.
async fn service_initiation_is_live_atomic_redacted_and_replays_before_delivery() {
    let fixture = Fixture::new();
    let context = load_taker_service_context(&fixture.config).unwrap();
    let module = taker_service_rpc_module(context).unwrap();

    assert_eq!(
        module.method_names().collect::<BTreeSet<_>>(),
        BTreeSet::from([
            "taker_health",
            "taker_offer_list_v1",
            "taker_swap_initiate_v1",
            "taker_swap_list_v1",
            "taker_swap_monitor_v1",
        ])
    );
    let health: TakerHealthV1 = module
        .call("taker_health", [TakerHealthRequestV1 { schema_version: 1 }])
        .await
        .unwrap();
    let methods = health.registered_methods();
    assert!(methods.health());
    assert!(methods.offer_list());
    assert!(methods.initiate());
    assert!(methods.swap_list());
    assert!(methods.monitor());
    assert!(!methods.claim());
    assert!(!methods.refund());

    let mut mismatch = fixture.request("m6-initiation-mismatch-001");
    mismatch.signed_envelope_sha256[0] ^= 0xff;
    let mismatch_response = rpc_response(
        &module,
        "taker_swap_initiate_v1",
        serde_json::to_value([mismatch.clone()]).unwrap(),
    )
    .await;
    assert_rpc_error(
        &mismatch_response,
        INVALID_PARAMS,
        "Invalid params",
        "initiation_selection_mismatch",
    );
    assert_redacted(&mismatch_response, &fixture);
    let mismatch_id = mismatch.request_id.clone();
    assert_eq!(
        SqliteTakerFacadeStore::open_existing(&fixture.registry)
            .unwrap()
            .lookup_initiation(&mismatch_id)
            .unwrap(),
        None,
        "rejected public selection mutated the registry"
    );

    let request = fixture.request("m6-initiation-live-001");
    let first: TakerInitiationCommitV1 = module
        .call("taker_swap_initiate_v1", [request.clone()])
        .await
        .unwrap();
    assert!(!first.was_replay);
    assert_eq!(first.schema_version, 1);
    assert_eq!(first.swap.schema_version, 1);
    assert_eq!(first.swap.swap_id.as_str(), "m6-zec-swap-001");
    assert_eq!(first.swap.offer_id, request.offer_id);
    assert_eq!(first.swap.route, request.route);
    assert_eq!(first.swap.foreign_units, request.foreign_units);
    assert_eq!(first.swap.lez_units, request.expected_lez_units);
    assert_eq!(first.swap.progress_generation, 0);
    assert_eq!(first.swap.state, TakerSwapStateV1::Initiating);
    assert_eq!(first.swap.available_action, None);
    assert_eq!(first.swap.privacy_guidance, None);

    let durable = SqliteTakerFacadeStore::open_existing(&fixture.registry)
        .unwrap()
        .lookup_initiation(&request.request_id)
        .unwrap()
        .expect("first admission must be durable before the RPC result");
    assert_eq!(durable.swap_id(), &first.swap.swap_id);
    assert_eq!(durable.offer_id(), &first.swap.offer_id);
    assert_eq!(durable.route(), first.swap.route);
    assert_eq!(durable.maker_identity(), request.maker_identity.as_bytes());
    assert_eq!(
        durable.signed_envelope_sha256(),
        &request.signed_envelope_sha256
    );
    assert_eq!(durable.foreign_units(), first.swap.foreign_units);
    assert_eq!(durable.lez_units(), first.swap.lez_units);

    fs::remove_file(&fixture.delivery_offer).unwrap();

    let replay: TakerInitiationCommitV1 = module
        .call("taker_swap_initiate_v1", [request.clone()])
        .await
        .expect("exact replay must not depend on live Delivery");
    assert!(replay.was_replay);
    assert_eq!(replay.swap, first.swap);

    let mut changed = request;
    changed.foreign_units += 1;
    let changed_response = rpc_response(
        &module,
        "taker_swap_initiate_v1",
        serde_json::to_value([changed]).unwrap(),
    )
    .await;
    assert_rpc_error(
        &changed_response,
        INITIATION_CONFLICT,
        "Taker initiation conflict",
        "initiation_conflict",
    );
    assert_redacted(&changed_response, &fixture);
}

#[tokio::test]
async fn replay_rejects_same_byte_signing_key_inode_drift() {
    let fixture = Fixture::new();
    let request = fixture.request("m6-replay-authority-drift-001");
    let module =
        taker_service_rpc_module(load_taker_service_context(&fixture.config).unwrap()).unwrap();
    let first: TakerInitiationCommitV1 = module
        .call("taker_swap_initiate_v1", [request.clone()])
        .await
        .unwrap();
    assert!(!first.was_replay);
    drop(module);

    let replacement = fixture.root.join("replacement-key.bin");
    private_file(replacement.clone(), &[42; 32]);
    fs::remove_file(&fixture.key).unwrap();
    fs::rename(replacement, &fixture.key).unwrap();

    let replay_module =
        taker_service_rpc_module(load_taker_service_context(&fixture.config).unwrap()).unwrap();
    let response = rpc_response(
        &replay_module,
        "taker_swap_initiate_v1",
        serde_json::to_value([request]).unwrap(),
    )
    .await;
    assert_rpc_error(
        &response,
        INITIATION_CONFLICT,
        "Taker initiation conflict",
        "initiation_conflict",
    );
    assert_redacted(&response, &fixture);
}

#[tokio::test]
async fn concurrent_exact_request_commits_once_and_replays_once() {
    let fixture = Fixture::new();
    let module =
        taker_service_rpc_module(load_taker_service_context(&fixture.config).unwrap()).unwrap();
    let request = fixture.request("m6-concurrent-exact-request-001");
    let params = serde_json::to_value([request.clone()]).unwrap();

    let (first, second) = tokio::join!(
        rpc_response(&module, "taker_swap_initiate_v1", params.clone()),
        rpc_response(&module, "taker_swap_initiate_v1", params),
    );
    let mut commits = [first, second].map(|response| {
        assert!(response.get("error").is_none(), "{response}");
        serde_json::from_value::<TakerInitiationCommitV1>(response["result"].clone()).unwrap()
    });
    commits.sort_unstable_by_key(|commit| commit.was_replay);

    assert!(!commits[0].was_replay);
    assert!(commits[1].was_replay);
    assert_eq!(commits[0].swap, commits[1].swap);
    let registry = SqliteTakerFacadeStore::open_existing(&fixture.registry).unwrap();
    assert_eq!(registry.list_initiations().unwrap().len(), 1);
    assert!(
        registry
            .lookup_initiation(&request.request_id)
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn concurrent_request_ids_for_one_prepared_swap_have_one_conflict() {
    let fixture = Fixture::new();
    let module =
        taker_service_rpc_module(load_taker_service_context(&fixture.config).unwrap()).unwrap();
    let first_request = fixture.request("m6-concurrent-conflict-request-a");
    let second_request = fixture.request("m6-concurrent-conflict-request-b");

    let (first, second) = tokio::join!(
        rpc_response(
            &module,
            "taker_swap_initiate_v1",
            serde_json::to_value([first_request.clone()]).unwrap(),
        ),
        rpc_response(
            &module,
            "taker_swap_initiate_v1",
            serde_json::to_value([second_request.clone()]).unwrap(),
        ),
    );
    let responses = [first, second];
    assert_eq!(
        responses
            .iter()
            .filter(|response| response.get("result").is_some())
            .count(),
        1
    );
    let conflicts = responses
        .iter()
        .filter(|response| response.get("error").is_some())
        .collect::<Vec<_>>();
    assert_eq!(conflicts.len(), 1);
    assert_rpc_error(
        conflicts[0],
        INITIATION_CONFLICT,
        "Taker initiation conflict",
        "initiation_conflict",
    );
    assert_redacted(conflicts[0], &fixture);

    let registry = SqliteTakerFacadeStore::open_existing(&fixture.registry).unwrap();
    assert_eq!(registry.list_initiations().unwrap().len(), 1);
    let durable = [&first_request.request_id, &second_request.request_id]
        .map(|request_id| registry.lookup_initiation(request_id).unwrap());
    assert_eq!(durable.iter().filter(|facts| facts.is_some()).count(), 1);
}

struct Fixture {
    _run: tempfile::TempDir,
    root: PathBuf,
    config: PathBuf,
    key: PathBuf,
    registry: PathBuf,
    delivery_offer: PathBuf,
    route: MakerRouteV1,
    maker: [u8; 33],
    commitment: [u8; 32],
}

impl Fixture {
    fn new() -> Self {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let run = tempfile::tempdir().unwrap();
        let root = private_directory(run.path().join("fixture"));
        let delivery = private_directory(root.join("delivery"));
        let registry = root.join("registry.sqlite3");
        drop(SqliteTakerFacadeStore::create_new(&registry).unwrap());

        let maker_secret = SecretKey::from_slice(&[7; 32]).unwrap();
        let maker =
            PublicKey::from_secret_key(&Secp256k1::signing_only(), &maker_secret).serialize();
        let route = MakerRouteV1::new(Pair::Zcash, SwapDirection::TakerSellsLez).unwrap();
        let offer = prepared_offer(&root, route, now);
        let publisher = RunLocalDelivery::publisher(delivery.clone(), maker_secret).unwrap();
        let authenticated = publisher
            .publish_or_verify(&DeliveryPublicationV1::new(offer, now))
            .unwrap();
        let commitment = authenticated.commitment();
        let delivery_offer = delivery.join("m6-zec-offer-001.offer.json");

        let signed = private_file(root.join("signed.json"), authenticated.signed_envelope());
        let draft = private_file(root.join("draft.json"), br#"{"unsigned":"draft"}"#);
        let key = private_file(root.join("key.bin"), &[42; 32]);
        let actor = private_file(root.join("actor.json"), br#"{"role":"taker"}"#);
        let config = private_json(
            root.join("service.json"),
            &json!({
                "schema_version": 1,
                "delivery_sources": [{
                    "source_id": "maker-a",
                    "directory": delivery,
                    "maker_public_key": hex::encode(maker),
                }],
                "chat_socket": root.join("chat.sock"),
                "maximum_offers": 16,
                "initiation": {
                    "registry_database": registry,
                    "prepared_zec": [{
                        "source_id": "maker-a",
                        "swap_id": "m6-zec-swap-001",
                        "offer_id": "m6-zec-offer-001",
                        "reservation_id": "m6-zec-reservation-001",
                        "foreign_units": 42,
                        "lez_units": 84,
                        "signed_envelope": digest_binding(&signed),
                        "unsigned_draft": digest_binding(&draft),
                        "signing_key": {"path": key},
                        "source_config": digest_binding(&actor),
                        "agreement_output": root.join("agreement.json"),
                        "actor_root": root.join("actor-root"),
                        "receipt_output": root.join("receipt.json"),
                    }]
                }
            }),
        );

        Self {
            _run: run,
            root,
            config,
            key,
            registry,
            delivery_offer,
            route,
            maker,
            commitment,
        }
    }

    fn request(&self, request_id: &str) -> TakerSwapInitiateRequestV1 {
        TakerSwapInitiateRequestV1 {
            schema_version: 1,
            request_id: RequestId::new(request_id).unwrap(),
            offer_id: MakerOfferId::new("m6-zec-offer-001").unwrap(),
            route: self.route,
            maker_identity: TakerMakerIdentityV1::new(self.maker).unwrap(),
            signed_envelope_sha256: self.commitment,
            foreign_units: 42,
            expected_lez_units: 84,
        }
    }
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

fn assert_redacted(response: &Value, fixture: &Fixture) {
    let wire = response.to_string().to_ascii_lowercase();
    assert!(!wire.contains(&fixture.root.display().to_string()));
    for forbidden in [
        "signed.json",
        "draft.json",
        "key.bin",
        "actor.json",
        "m6-zec-reservation-001",
        "private_key",
        "credential",
    ] {
        assert!(!wire.contains(forbidden), "{wire}");
    }
}

fn prepared_offer(root: &Path, route: MakerRouteV1, now: u64) -> lez_swap_store::MakerOfferV1 {
    let mut store = SqliteSwapStore::open(root.join("offer.sqlite3")).unwrap();
    let disabled =
        MakerPairConfigurationV1::new(route, false, MakerPriceSourceKind::Local, 1, 10_000, 300)
            .unwrap();
    store
        .configure_maker_pair(
            &RequestId::new("m6-prepared-pair-create").unwrap(),
            None,
            &disabled,
        )
        .unwrap();
    store
        .set_local_price(
            &RequestId::new("m6-prepared-price-create").unwrap(),
            None,
            &LocalPriceV1::new(route, 2, 1).unwrap(),
        )
        .unwrap();
    let enabled =
        MakerPairConfigurationV1::new(route, true, MakerPriceSourceKind::Local, 1, 10_000, 300)
            .unwrap();
    store
        .configure_maker_pair(
            &RequestId::new("m6-prepared-pair-enable").unwrap(),
            Some(1),
            &enabled,
        )
        .unwrap();
    store
        .publish_local_offer(
            &RequestId::new("m6-prepared-offer-publish").unwrap(),
            &MakerOfferId::new("m6-zec-offer-001").unwrap(),
            route,
            now,
        )
        .unwrap();
    store.list_discoverable_maker_offers(now).unwrap()[0]
        .offer()
        .clone()
}

fn digest_binding(path: &Path) -> Value {
    json!({
        "path": path,
        "sha256": hex::encode(Sha256::digest(fs::read(path).unwrap())),
    })
}

fn private_json(path: PathBuf, value: &Value) -> PathBuf {
    private_file(path, &serde_json::to_vec(value).unwrap())
}

fn private_file(path: PathBuf, bytes: &[u8]) -> PathBuf {
    fs::write(&path, bytes).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    path
}

fn private_directory(path: PathBuf) -> PathBuf {
    fs::create_dir(&path).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
    path
}
