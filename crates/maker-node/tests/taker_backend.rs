//! Direct tests for the transport-free, read-only Taker facade backend.

use std::{
    fs,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use lez_bridge_protocol::RequestId;
pub use lez_maker_node::{
    DeliveryOfferQueryV1, RunLocalDelivery, TAKER_FACADE_SCHEMA_VERSION_V1, TakerBackendError,
    TakerDependencyProbe, TakerDependencyStateV1, TakerFacadeBackend, TakerHealthRequestV1,
    TakerHealthV1, TakerMakerIdentityV1, TakerOfferListRequestV1, TakerOfferListV1,
    TakerOfferViewV1, TakerTrustedTimeSource, taker_pair_capabilities_v1,
};
use lez_swap_core::{Pair, SwapDirection};
use lez_swap_sdk_core::OfferDiscovery as _;
use lez_swap_store::{
    LocalPriceV1, MakerOfferId, MakerPairConfigurationV1, MakerPriceSourceKind, MakerRouteV1,
    SqliteSwapStore,
};
use secp256k1::{PublicKey, Secp256k1, SecretKey};
use tempfile::tempdir;

const NOW: u64 = 1_001;

#[derive(Clone, Copy)]
struct FixedClock(Option<u64>);

impl TakerTrustedTimeSource for FixedClock {
    fn now_unix_seconds(&self) -> Option<u64> {
        self.0
    }
}

struct CountingClock {
    now: Option<u64>,
    calls: Arc<AtomicUsize>,
}

impl TakerTrustedTimeSource for CountingClock {
    fn now_unix_seconds(&self) -> Option<u64> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.now
    }
}

#[derive(Clone, Copy)]
struct FixedProbe(bool);

impl TakerDependencyProbe for FixedProbe {
    fn is_available(&self) -> bool {
        self.0
    }
}

#[tokio::test]
async fn authenticated_offer_listing_returns_only_public_pinned_facts() {
    let run = tempdir().unwrap();
    let delivery_root = run.path().join("delivery");
    let key = signing_key(7);
    let maker = PublicKey::from_secret_key(&Secp256k1::signing_only(), &key);
    let publisher = RunLocalDelivery::publisher(&delivery_root, key).unwrap();
    let expected = offer("m6-backend-zec-001", zec_route(), 5);
    let authenticated = publisher
        .publish(lez_maker_node::DeliveryPublicationV1::new(
            expected.clone(),
            1_000,
        ))
        .await
        .unwrap();
    let subscriber = RunLocalDelivery::subscriber(&delivery_root, maker).unwrap();
    let backend = TakerFacadeBackend::new(
        vec![subscriber],
        FixedClock(Some(NOW)),
        Some(FixedProbe(true)),
        16,
    )
    .unwrap();
    let diagnostic = format!("{backend:?}");
    assert!(!diagnostic.contains(&delivery_root.display().to_string()));
    assert!(diagnostic.contains("delivery_source_count: 1"));
    assert!(diagnostic.contains("chat_configured: true"));

    let result = backend
        .offer_list(&TakerOfferListRequestV1 {
            schema_version: 1,
            route: Some(zec_route()),
        })
        .await
        .unwrap();

    assert_eq!(result.schema_version, TAKER_FACADE_SCHEMA_VERSION_V1);
    assert_eq!(result.offers.len(), 1);
    assert_eq!(result.offers[0].offer, expected);
    assert_eq!(
        result.offers[0].maker_identity.as_bytes(),
        &maker.serialize()
    );
    assert_eq!(
        result.offers[0].signed_envelope_sha256,
        authenticated.commitment()
    );
    let wire = serde_json::to_value(result).unwrap();
    for forbidden in ["path", "file", "socket", "endpoint", "secret", "credential"] {
        assert!(!wire.to_string().to_ascii_lowercase().contains(forbidden));
    }
}

#[tokio::test]
async fn schema_and_route_validation_fail_before_dependency_access() {
    let backend =
        TakerFacadeBackend::new(Vec::new(), FixedClock(None), None::<FixedProbe>, 16).unwrap();

    assert_eq!(
        backend
            .health(&TakerHealthRequestV1 { schema_version: 2 })
            .await,
        Err(TakerBackendError::UnsupportedSchemaVersion)
    );
    assert_eq!(
        backend
            .offer_list(&TakerOfferListRequestV1 {
                schema_version: 0,
                route: None,
            })
            .await,
        Err(TakerBackendError::UnsupportedSchemaVersion)
    );
    assert_eq!(
        backend
            .offer_list(&TakerOfferListRequestV1 {
                schema_version: 1,
                route: Some(
                    MakerRouteV1::new(Pair::Bitcoin, SwapDirection::TakerSellsLez).unwrap(),
                ),
            })
            .await,
        Err(TakerBackendError::UnsupportedRoute)
    );
    assert_eq!(
        backend
            .offer_list(&TakerOfferListRequestV1 {
                schema_version: 1,
                route: None,
            })
            .await,
        Err(TakerBackendError::TrustedTimeUnavailable)
    );
}

#[tokio::test]
async fn offer_list_samples_trusted_time_exactly_once() {
    let calls = Arc::new(AtomicUsize::new(0));
    let backend = TakerFacadeBackend::new(
        Vec::new(),
        CountingClock {
            now: Some(NOW),
            calls: Arc::clone(&calls),
        },
        None::<FixedProbe>,
        16,
    )
    .unwrap();

    assert_eq!(
        backend.trusted_now_for_offer_list(&TakerOfferListRequestV1 {
            schema_version: 2,
            route: None,
        }),
        Err(TakerBackendError::UnsupportedSchemaVersion)
    );
    assert_eq!(
        backend.trusted_now_for_offer_list(&TakerOfferListRequestV1 {
            schema_version: 1,
            route: Some(MakerRouteV1::new(Pair::Bitcoin, SwapDirection::TakerSellsLez,).unwrap()),
        }),
        Err(TakerBackendError::UnsupportedRoute)
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);

    backend
        .offer_list(&TakerOfferListRequestV1 {
            schema_version: 1,
            route: Some(zec_route()),
        })
        .await
        .unwrap();

    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn offer_list_at_uses_the_explicit_expiry_boundary_without_reading_clock() {
    let run = tempdir().unwrap();
    let delivery_root = run.path().join("delivery");
    let key = signing_key(12);
    let maker = PublicKey::from_secret_key(&Secp256k1::signing_only(), &key);
    let publisher = RunLocalDelivery::publisher(&delivery_root, key).unwrap();
    let expected = offer("m6-expiry-boundary-001", zec_route(), 5);
    let expires_at = expected.expires_at_unix_seconds();
    publisher
        .publish(lez_maker_node::DeliveryPublicationV1::new(expected, 1_000))
        .await
        .unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let backend = TakerFacadeBackend::new(
        vec![RunLocalDelivery::subscriber(&delivery_root, maker).unwrap()],
        CountingClock {
            now: None,
            calls: Arc::clone(&calls),
        },
        None::<FixedProbe>,
        16,
    )
    .unwrap();
    let request = TakerOfferListRequestV1 {
        schema_version: 1,
        route: Some(zec_route()),
    };

    assert_eq!(
        backend
            .offer_list_at(&request, expires_at - 1)
            .await
            .unwrap()
            .offers
            .len(),
        1
    );
    assert!(
        backend
            .offer_list_at(&request, expires_at)
            .await
            .unwrap()
            .offers
            .is_empty()
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn exact_duplicate_sources_collapse_to_one_stable_offer() {
    let run = tempdir().unwrap();
    let delivery_root = run.path().join("delivery");
    let key = signing_key(8);
    let maker = PublicKey::from_secret_key(&Secp256k1::signing_only(), &key);
    let publisher = RunLocalDelivery::publisher(&delivery_root, key).unwrap();
    publisher
        .publish(lez_maker_node::DeliveryPublicationV1::new(
            offer("m6-backend-dedup-001", zec_route(), 5),
            1_000,
        ))
        .await
        .unwrap();
    let first = RunLocalDelivery::subscriber(&delivery_root, maker).unwrap();
    let duplicate = RunLocalDelivery::subscriber(&delivery_root, maker).unwrap();
    let backend = TakerFacadeBackend::new(
        vec![first, duplicate],
        FixedClock(Some(NOW)),
        None::<FixedProbe>,
        1,
    )
    .unwrap();

    let listed = backend
        .offer_list(&TakerOfferListRequestV1 {
            schema_version: 1,
            route: None,
        })
        .await
        .unwrap();
    assert_eq!(listed.offers.len(), 1);
}

#[tokio::test]
async fn unique_result_limit_and_conflicting_duplicate_fail_closed() {
    let run = tempdir().unwrap();
    let delivery_root = run.path().join("bounded");
    let key = signing_key(9);
    let maker = PublicKey::from_secret_key(&Secp256k1::signing_only(), &key);
    let publisher = RunLocalDelivery::publisher(&delivery_root, key).unwrap();
    for (id, numerator) in [("m6-bound-001", 5), ("m6-bound-002", 7)] {
        publisher
            .publish(lez_maker_node::DeliveryPublicationV1::new(
                offer(id, zec_route(), numerator),
                1_000,
            ))
            .await
            .unwrap();
    }
    let bounded = TakerFacadeBackend::new(
        vec![RunLocalDelivery::subscriber(&delivery_root, maker).unwrap()],
        FixedClock(Some(NOW)),
        None::<FixedProbe>,
        1,
    )
    .unwrap();
    assert_eq!(
        bounded
            .offer_list(&TakerOfferListRequestV1 {
                schema_version: 1,
                route: None,
            })
            .await,
        Err(TakerBackendError::OfferLimitExceeded)
    );

    let conflict_a = run.path().join("conflict-a");
    let conflict_b = run.path().join("conflict-b");
    let publisher_a = RunLocalDelivery::publisher(&conflict_a, signing_key(10)).unwrap();
    let publisher_b = RunLocalDelivery::publisher(&conflict_b, signing_key(10)).unwrap();
    publisher_a
        .publish(lez_maker_node::DeliveryPublicationV1::new(
            offer("m6-conflict-001", zec_route(), 5),
            1_000,
        ))
        .await
        .unwrap();
    publisher_b
        .publish(lez_maker_node::DeliveryPublicationV1::new(
            offer("m6-conflict-001", zec_route(), 7),
            1_000,
        ))
        .await
        .unwrap();
    let conflict_maker = PublicKey::from_secret_key(&Secp256k1::signing_only(), &signing_key(10));
    let conflicting = TakerFacadeBackend::new(
        vec![
            RunLocalDelivery::subscriber(&conflict_a, conflict_maker).unwrap(),
            RunLocalDelivery::subscriber(&conflict_b, conflict_maker).unwrap(),
        ],
        FixedClock(Some(NOW)),
        None::<FixedProbe>,
        16,
    )
    .unwrap();
    assert_eq!(
        conflicting
            .offer_list(&TakerOfferListRequestV1 {
                schema_version: 1,
                route: None,
            })
            .await,
        Err(TakerBackendError::ConflictingAuthenticatedOffer)
    );
}

#[tokio::test]
async fn health_reports_disabled_available_and_degraded_dependencies() {
    let disabled =
        TakerFacadeBackend::new(Vec::new(), FixedClock(Some(NOW)), None::<FixedProbe>, 16)
            .unwrap()
            .health(&TakerHealthRequestV1 { schema_version: 1 })
            .await
            .unwrap();
    assert_eq!(disabled.delivery(), TakerDependencyStateV1::Disabled);
    assert_eq!(disabled.chat(), TakerDependencyStateV1::Disabled);
    assert!(!disabled.is_degraded());

    let run = tempdir().unwrap();
    let delivery_root = run.path().join("delivery");
    let key = signing_key(11);
    let maker = PublicKey::from_secret_key(&Secp256k1::signing_only(), &key);
    let publisher = RunLocalDelivery::publisher(&delivery_root, key).unwrap();
    publisher
        .publish(lez_maker_node::DeliveryPublicationV1::new(
            offer("m6-health-001", zec_route(), 5),
            1_000,
        ))
        .await
        .unwrap();
    let available = TakerFacadeBackend::new(
        vec![RunLocalDelivery::subscriber(&delivery_root, maker).unwrap()],
        FixedClock(Some(NOW)),
        Some(FixedProbe(true)),
        16,
    )
    .unwrap()
    .health(&TakerHealthRequestV1 { schema_version: 1 })
    .await
    .unwrap();
    assert_eq!(available.delivery(), TakerDependencyStateV1::Available);
    assert_eq!(available.chat(), TakerDependencyStateV1::Available);
    assert!(!available.is_degraded());

    fs::write(delivery_root.join("m6-health-001.offer.json"), b"tampered").unwrap();
    let unavailable = TakerFacadeBackend::new(
        vec![RunLocalDelivery::subscriber(&delivery_root, maker).unwrap()],
        FixedClock(Some(NOW)),
        Some(FixedProbe(false)),
        16,
    )
    .unwrap()
    .health(&TakerHealthRequestV1 { schema_version: 1 })
    .await
    .unwrap();
    assert_eq!(unavailable.delivery(), TakerDependencyStateV1::Unavailable);
    assert_eq!(unavailable.chat(), TakerDependencyStateV1::Unavailable);
    assert!(unavailable.is_degraded());
}

#[test]
fn backend_configuration_and_errors_are_bounded_and_path_free() {
    assert!(matches!(
        TakerFacadeBackend::new(Vec::new(), FixedClock(Some(NOW)), None::<FixedProbe>, 0,),
        Err(TakerBackendError::InvalidConfiguration)
    ));
    for error in [
        TakerBackendError::UnsupportedSchemaVersion,
        TakerBackendError::UnsupportedRoute,
        TakerBackendError::TrustedTimeUnavailable,
        TakerBackendError::DeliveryUnavailable,
        TakerBackendError::OfferLimitExceeded,
        TakerBackendError::ConflictingAuthenticatedOffer,
        TakerBackendError::InvalidConfiguration,
    ] {
        let message = error.to_string().to_ascii_lowercase();
        for forbidden in ["/", "path", "file", "socket", "endpoint", "credential"] {
            assert!(!message.contains(forbidden), "{message}");
        }
    }
}

fn offer(id: &str, route: MakerRouteV1, price_numerator: u64) -> lez_swap_store::MakerOfferV1 {
    let run = tempdir().unwrap();
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
