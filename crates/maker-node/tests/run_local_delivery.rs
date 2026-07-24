//! Acceptance tests for the run-local Delivery-compatible adapter.

use std::{fs, os::unix::fs::PermissionsExt as _, process::Command};

use lez_bridge_protocol::RequestId;
use lez_maker_node::{
    DeliveryOfferQueryV1, DeliveryPublicationV1, RunLocalDelivery, RunLocalDeliveryError,
};
use lez_swap_core::{Pair, SwapDirection};
use lez_swap_sdk_core::OfferDiscovery as _;
use lez_swap_store::{
    LocalPriceV1, MakerOfferId, MakerPairConfigurationV1, MakerPriceSourceKind, MakerRouteV1,
    SqliteSwapStore,
};
use secp256k1::{PublicKey, Secp256k1, SecretKey};
use serde_json::Value;
use tempfile::tempdir;

fn request(value: &str) -> RequestId {
    RequestId::new(value).expect("bounded request ID")
}

fn zec_route() -> MakerRouteV1 {
    MakerRouteV1::new(Pair::Zcash, SwapDirection::TakerSellsLez).unwrap()
}

fn offer() -> lez_swap_store::MakerOfferV1 {
    let run = tempdir().expect("isolated store");
    let mut store = SqliteSwapStore::open(run.path().join("offer.sqlite3")).unwrap();
    let route = zec_route();
    let disabled =
        MakerPairConfigurationV1::new(route, false, MakerPriceSourceKind::Local, 10, 10_000, 300)
            .unwrap();
    store
        .configure_maker_pair(&request("delivery-pair-create-001"), None, &disabled)
        .unwrap();
    store
        .set_local_price(
            &request("delivery-price-create-001"),
            None,
            &LocalPriceV1::new(route, 5, 2).unwrap(),
        )
        .unwrap();
    let enabled =
        MakerPairConfigurationV1::new(route, true, MakerPriceSourceKind::Local, 10, 10_000, 300)
            .unwrap();
    store
        .configure_maker_pair(&request("delivery-pair-enable-001"), Some(1), &enabled)
        .unwrap();
    store
        .publish_local_offer(
            &request("delivery-offer-publish-001"),
            &MakerOfferId::new("delivery-offer-001").unwrap(),
            route,
            1_000,
        )
        .unwrap();
    store.list_discoverable_maker_offers(1_000).unwrap()[0]
        .offer()
        .clone()
}

fn signing_key(byte: u8) -> SecretKey {
    SecretKey::from_slice(&[byte; 32]).expect("valid deterministic test key")
}

#[tokio::test]
async fn maker_publishes_and_taker_discovers_an_authenticated_expiring_offer() {
    let run = tempdir().expect("isolated Delivery root");
    let directory = run.path().join("delivery");
    let key = signing_key(7);
    let identity = PublicKey::from_secret_key(&Secp256k1::signing_only(), &key);
    let publisher = RunLocalDelivery::publisher(&directory, key).unwrap();
    assert_eq!(
        fs::metadata(&directory).unwrap().permissions().mode() & 0o7777,
        0o700
    );

    let expected_offer = offer();
    let authenticated = publisher
        .publish(DeliveryPublicationV1::new(expected_offer.clone(), 1_000))
        .await
        .unwrap();
    assert_eq!(authenticated.offer(), &expected_offer);
    assert_eq!(authenticated.maker_identity(), &identity.serialize());
    assert_ne!(authenticated.commitment(), [0; 32]);

    let subscriber = RunLocalDelivery::subscriber(&directory, identity).unwrap();
    let discovered = subscriber
        .discover(&DeliveryOfferQueryV1::for_route(zec_route(), 1_299))
        .await
        .unwrap();
    assert_eq!(discovered, vec![authenticated]);
    assert!(
        subscriber
            .discover(&DeliveryOfferQueryV1::all(1_300))
            .await
            .unwrap()
            .is_empty(),
        "exclusive expiry boundary must remove the offer"
    );
    assert!(matches!(
        subscriber
            .publish(DeliveryPublicationV1::new(expected_offer, 1_001))
            .await,
        Err(RunLocalDeliveryError::DiscoveryOnly)
    ));
}

#[tokio::test]
async fn discovery_rejects_tampering_and_the_wrong_maker_identity() {
    let run = tempdir().expect("isolated Delivery root");
    let directory = run.path().join("delivery");
    let key = signing_key(11);
    let identity = PublicKey::from_secret_key(&Secp256k1::signing_only(), &key);
    let publisher = RunLocalDelivery::publisher(&directory, key).unwrap();
    publisher
        .publish(DeliveryPublicationV1::new(offer(), 1_000))
        .await
        .unwrap();

    let wrong_key = signing_key(12);
    let wrong_identity = PublicKey::from_secret_key(&Secp256k1::signing_only(), &wrong_key);
    let wrong_subscriber = RunLocalDelivery::subscriber(&directory, wrong_identity).unwrap();
    assert!(matches!(
        wrong_subscriber
            .discover(&DeliveryOfferQueryV1::all(1_001))
            .await,
        Err(RunLocalDeliveryError::Authentication)
    ));

    let path = directory.join("delivery-offer-001.offer.json");
    let mut envelope: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    envelope["offer_json"][0] = Value::from(124);
    fs::write(&path, serde_json::to_vec(&envelope).unwrap()).unwrap();
    let subscriber = RunLocalDelivery::subscriber(&directory, identity).unwrap();
    assert!(matches!(
        subscriber.discover(&DeliveryOfferQueryV1::all(1_001)).await,
        Err(RunLocalDeliveryError::Authentication)
    ));
}

#[tokio::test]
async fn publication_is_immutable_and_rejects_insecure_directories() {
    let run = tempdir().expect("isolated Delivery root");
    let directory = run.path().join("delivery");
    let publisher = RunLocalDelivery::publisher(&directory, signing_key(21)).unwrap();
    let publication = DeliveryPublicationV1::new(offer(), 1_000);
    publisher.publish(publication.clone()).await.unwrap();
    assert!(matches!(
        publisher.publish(publication).await,
        Err(RunLocalDeliveryError::AlreadyExists)
    ));

    let insecure = run.path().join("shared");
    fs::create_dir(&insecure).unwrap();
    fs::set_permissions(&insecure, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(matches!(
        RunLocalDelivery::publisher(&insecure, signing_key(22)),
        Err(RunLocalDeliveryError::InsecureDirectory)
    ));
}

#[tokio::test]
async fn separate_taker_process_discovers_only_key_pinned_live_route_offers() {
    let run = tempdir().expect("isolated Delivery root");
    let directory = run.path().join("delivery");
    let key = signing_key(8);
    let identity = PublicKey::from_secret_key(&Secp256k1::signing_only(), &key);
    let publisher = RunLocalDelivery::publisher(&directory, key).unwrap();
    let authenticated = publisher
        .publish(DeliveryPublicationV1::new(offer(), 1_000))
        .await
        .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_lez-taker"))
        .args([
            "--delivery-directory",
            directory.to_str().unwrap(),
            "--maker-public-key",
            &hex::encode(identity.serialize()),
            "--now-unix-seconds",
            "1299",
            "--pair",
            "zcash",
            "--direction",
            "taker-sells-lez",
        ])
        .output()
        .expect("run separate taker process");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["offers"].as_array().unwrap().len(), 1);
    assert_eq!(value["offers"][0]["offer"]["id"], "delivery-offer-001");
    assert_eq!(
        value["offers"][0]["maker_public_key"],
        hex::encode(identity.serialize())
    );
    assert_eq!(
        value["offers"][0]["signed_envelope_sha256"],
        hex::encode(authenticated.commitment())
    );

    let identity_hex = hex::encode(identity.serialize());
    let expired = Command::new(env!("CARGO_BIN_EXE_lez-taker"))
        .args([
            "--delivery-directory",
            directory.to_str().unwrap(),
            "--maker-public-key",
            &identity_hex,
            "--now-unix-seconds",
            "1300",
        ])
        .output()
        .unwrap();
    assert!(expired.status.success());
    let expired: Value = serde_json::from_slice(&expired.stdout).unwrap();
    assert!(expired["offers"].as_array().unwrap().is_empty());
}
