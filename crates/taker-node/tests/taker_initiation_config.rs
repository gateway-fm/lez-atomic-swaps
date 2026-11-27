//! Contract for the Taker service's initiation context: what the startup
//! configuration may name, and which prepared swaps the Node holds after it
//! loaded. A Bitcoin swap may be one of the owner's `prepared_btc` entries; a
//! Zcash swap is only ever the record the Node itself persisted when it
//! agreed the swap at take time (the tests gated on `pair-zec`), and a
//! configuration that still names `initiation.prepared_zec` is refused by name.

#[cfg(feature = "pair-zec")]
mod support;

use std::{
    fs,
    os::unix::fs::PermissionsExt as _,
    path::{Path, PathBuf},
};

use lez_bridge_protocol::RequestId;
use lez_swap_core::{Pair, SwapDirection};
use lez_swap_store::{
    LocalPriceV1, MakerOfferId, MakerPairConfigurationV1, MakerPriceSourceKind, MakerRouteV1,
    SqliteSwapStore, SqliteTakerFacadeStore,
};
use lez_taker_node::{
    DeliveryPublicationV1, RunLocalDelivery, TakerServiceStartupError, load_taker_service_backend,
    load_taker_service_context,
};
use secp256k1::{PublicKey, Secp256k1, SecretKey};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};

const OFFER: &str = "m6-offer-001";
const SWAP: &str = "m6-swap-001";
const RESERVATION: &str = "m6-reservation-001";

#[test]
fn existing_registry_and_named_source_build_one_prepared_btc_entry() {
    let fixture = Fixture::new();
    let context = load_taker_service_context(&fixture.config).unwrap();
    let initiation = context.initiation().expect("configured initiation");
    assert_eq!(initiation.prepared_count(), 1);
    let offer = MakerOfferId::new(OFFER).unwrap();
    let prepared = initiation.prepared_for_offer(&offer).unwrap();
    assert_eq!(prepared.swap_id().as_str(), SWAP);
    assert_eq!(prepared.offer_id(), &offer);
    assert_eq!(prepared.reservation_id().as_str(), RESERVATION);
    assert_eq!(prepared.maker_identity(), &fixture.maker);
    assert_eq!(prepared.facts().route().pair(), Pair::Bitcoin);
    assert_eq!(
        prepared.facts().route().direction(),
        SwapDirection::TakerSellsLez
    );
    assert_eq!(prepared.facts().foreign_units(), 42);
    assert_eq!(prepared.facts().lez_units(), 84);
    assert_eq!(
        format!("{:?}", prepared.execution()),
        "PreparedExecutionV1 { configured: true, .. }"
    );

    let debug = format!("{context:?}");
    assert!(debug.contains("initiation_configured: true"));
    assert!(!debug.contains(fixture.root.to_str().unwrap()));
    assert!(!debug.contains(RESERVATION));
    assert_eq!(
        load_taker_service_backend(&fixture.config).unwrap_err(),
        TakerServiceStartupError::InvalidConfiguration,
        "read-only loader must not silently discard initiation authority"
    );
}

#[test]
fn a_prepared_btc_entry_derives_either_direction_from_the_authenticated_offer() {
    for direction in [
        SwapDirection::TakerSellsForeign,
        SwapDirection::TakerSellsLez,
    ] {
        let fixture = Fixture::new_with(Pair::Bitcoin, direction);
        let context = load_taker_service_context(&fixture.config).unwrap();
        let prepared = context
            .initiation()
            .unwrap()
            .prepared_for_offer(&MakerOfferId::new(OFFER).unwrap())
            .unwrap();

        assert_eq!(prepared.facts().route().pair(), Pair::Bitcoin);
        assert_eq!(prepared.facts().route().direction(), direction);
    }
}

#[test]
fn legacy_read_configuration_remains_compatible_without_source_id() {
    let fixture = Fixture::new();
    let mut legacy = fixture.value.clone();
    legacy.as_object_mut().unwrap().remove("initiation");
    legacy["delivery_sources"][0]
        .as_object_mut()
        .unwrap()
        .remove("source_id");
    let config = fixture.write_config("legacy", &legacy);
    let context = load_taker_service_context(&config).unwrap();
    assert!(context.initiation().is_none());
    assert!(load_taker_service_backend(&config).is_ok());
}

#[test]
fn prepared_entries_require_an_owner_local_chat_socket() {
    let fixture = Fixture::new();
    let mut missing_chat = fixture.value.clone();
    missing_chat.as_object_mut().unwrap().remove("chat_socket");
    fixture.assert_invalid(
        "missing-chat",
        &missing_chat,
        TakerServiceStartupError::InvalidConfiguration,
    );
}

/// The list of Zcash swaps prepared ahead of the take is gone. A
/// configuration that still names it, empty or not, is refused at load and
/// the error names the field and what replaced it, in every build.
#[test]
fn a_configuration_that_names_the_removed_prepared_zec_list_is_refused_by_name() {
    let fixture = Fixture::new();
    let entry = fixture.value["initiation"]["prepared_btc"][0].clone();
    for (name, list) in [("empty", json!([])), ("one", json!([entry]))] {
        let mut older = fixture.value.clone();
        older["initiation"]["prepared_zec"] = list;
        fixture.assert_invalid(name, &older, TakerServiceStartupError::RemovedPreparedZec);
        // ... also when nothing else is configured beside it.
        older["initiation"]
            .as_object_mut()
            .unwrap()
            .remove("prepared_btc");
        fixture.assert_invalid(name, &older, TakerServiceStartupError::RemovedPreparedZec);
    }
    let message = TakerServiceStartupError::RemovedPreparedZec.to_string();
    assert!(message.contains("initiation.prepared_zec"), "{message}");
    assert!(message.contains("initiation.zec_role_config"), "{message}");

    // Any other unknown field is still refused, without a name of its own.
    let mut unknown = fixture.value.clone();
    unknown["initiation"]["prepared_ltc"] = json!([]);
    fixture.assert_invalid(
        "unknown",
        &unknown,
        TakerServiceStartupError::InvalidConfiguration,
    );
}

#[test]
fn prepared_btc_rejects_dynamic_identity_bad_sources_duplicates_and_overflow() {
    let fixture = Fixture::new();
    for (name, field, value) in [
        ("request", "request_id", json!("dynamic-client-request")),
        (
            "identity",
            "maker_identity",
            json!(hex::encode(fixture.maker)),
        ),
        (
            "route",
            "route",
            json!({"pair":"bitcoin","direction":"taker_sells_lez"}),
        ),
    ] {
        let mut invalid = fixture.value.clone();
        invalid["initiation"]["prepared_btc"][0][field] = value;
        fixture.assert_invalid(
            name,
            &invalid,
            TakerServiceStartupError::InvalidConfiguration,
        );
    }

    let mut unnamed = fixture.value.clone();
    unnamed["delivery_sources"][0]
        .as_object_mut()
        .unwrap()
        .remove("source_id");
    fixture.assert_invalid(
        "unnamed",
        &unnamed,
        TakerServiceStartupError::InvalidConfiguration,
    );

    let mut missing = fixture.value.clone();
    missing["initiation"]["prepared_btc"][0]["source_id"] = json!("missing");
    fixture.assert_invalid(
        "missing",
        &missing,
        TakerServiceStartupError::InvalidConfiguration,
    );

    let mut wrong_offer = fixture.value.clone();
    wrong_offer["initiation"]["prepared_btc"][0]["offer_id"] = json!("m6-offer-002");
    fixture.assert_invalid(
        "wrong-offer",
        &wrong_offer,
        TakerServiceStartupError::InvalidConfiguration,
    );

    let mut wrong_quote = fixture.value.clone();
    wrong_quote["initiation"]["prepared_btc"][0]["lez_units"] = json!(85);
    fixture.assert_invalid(
        "wrong-quote",
        &wrong_quote,
        TakerServiceStartupError::InvalidConfiguration,
    );

    let mut wrong_source = fixture.value.clone();
    let other_maker = PublicKey::from_secret_key(
        &Secp256k1::signing_only(),
        &SecretKey::from_slice(&[8; 32]).unwrap(),
    );
    let mut second_source = wrong_source["delivery_sources"][0].clone();
    second_source["source_id"] = json!("maker-b");
    second_source["maker_public_key"] = json!(hex::encode(other_maker.serialize()));
    wrong_source["delivery_sources"]
        .as_array_mut()
        .unwrap()
        .push(second_source);
    wrong_source["initiation"]["prepared_btc"][0]["source_id"] = json!("maker-b");
    fixture.assert_invalid(
        "wrong-source",
        &wrong_source,
        TakerServiceStartupError::InvalidConfiguration,
    );
    let mut duplicate = fixture.value.clone();
    let repeated_source = duplicate["delivery_sources"][0].clone();
    duplicate["delivery_sources"]
        .as_array_mut()
        .unwrap()
        .push(repeated_source);
    fixture.assert_invalid(
        "duplicate-source",
        &duplicate,
        TakerServiceStartupError::InvalidConfiguration,
    );

    let mut overflow = fixture.value.clone();
    overflow["initiation"]["prepared_btc"] =
        Value::Array(vec![overflow["initiation"]["prepared_btc"][0].clone(); 257]);
    fixture.assert_invalid(
        "overflow",
        &overflow,
        TakerServiceStartupError::InvalidConfiguration,
    );
}

#[test]
fn snapshots_validate_digests_keys_paths_and_existing_registry_with_fixed_errors() {
    let fixture = Fixture::new();

    let mut digest = fixture.value.clone();
    digest["initiation"]["prepared_btc"][0]["signed_envelope"]["sha256"] = json!("00".repeat(32));
    fixture.assert_invalid(
        "digest",
        &digest,
        TakerServiceStartupError::InvalidConfiguration,
    );

    let mut alias = fixture.value.clone();
    alias["initiation"]["prepared_btc"][0]["receipt_output"] =
        alias["initiation"]["prepared_btc"][0]["agreement_output"].clone();
    fixture.assert_invalid(
        "alias",
        &alias,
        TakerServiceStartupError::InvalidConfiguration,
    );

    let mut normalized = fixture.value.clone();
    normalized["initiation"]["prepared_btc"][0]["actor_root"] =
        json!(fixture.root.join("actor/../actor"));
    fixture.assert_invalid(
        "normalized",
        &normalized,
        TakerServiceStartupError::InvalidConfiguration,
    );

    let bad_key = private_file(fixture.root.join("bad-key.bin"), &[0; 32]);
    let mut invalid_key = fixture.value.clone();
    invalid_key["initiation"]["prepared_btc"][0]["signing_key"]["path"] = json!(bad_key);
    fixture.assert_invalid(
        "bad-key",
        &invalid_key,
        TakerServiceStartupError::InvalidConfiguration,
    );

    let mut missing_registry = fixture.value.clone();
    missing_registry["initiation"]["registry_database"] =
        json!(fixture.root.join("missing.sqlite3"));
    fixture.assert_invalid(
        "missing-registry",
        &missing_registry,
        TakerServiceStartupError::InitiationUnavailable,
    );
}

/// A Zcash swap is in the Node's hands only as the record its own take
/// persisted in the swap's directory: a restart reloads it from there, in
/// either direction, with nothing about it in the startup configuration.
#[cfg(feature = "pair-zec")]
#[test]
fn a_zcash_swap_prepared_at_take_time_is_reloaded_from_its_swap_directory() {
    for direction in [
        SwapDirection::TakerSellsForeign,
        SwapDirection::TakerSellsLez,
    ] {
        let fixture = Fixture::new_with(Pair::Zcash, direction);
        assert!(fixture.value["initiation"].get("prepared_zec").is_none());
        assert!(fixture.value["initiation"].get("prepared_btc").is_none());
        let context = load_taker_service_context(&fixture.config).unwrap();
        let initiation = context.initiation().expect("configured initiation");
        assert_eq!(initiation.prepared_count(), 1);
        let offer = MakerOfferId::new(OFFER).unwrap();
        let prepared = initiation.prepared_for_offer(&offer).unwrap();
        assert_eq!(prepared.swap_id().as_str(), SWAP);
        assert_eq!(prepared.reservation_id().as_str(), RESERVATION);
        assert_eq!(prepared.maker_identity(), &fixture.maker);
        assert_eq!(prepared.facts().route().pair(), Pair::Zcash);
        assert_eq!(prepared.facts().route().direction(), direction);
        assert_eq!(prepared.facts().foreign_units(), 42);
        assert_eq!(prepared.facts().lez_units(), 84);
        let debug = format!("{context:?}");
        assert!(!debug.contains(fixture.root.to_str().unwrap()));
        assert!(!debug.contains(RESERVATION));
    }
}

/// A persisted Zcash swap whose files no longer verify (an altered byte, a
/// quote or an offer that is not the authenticated one, a key that is not
/// one) is left out, by itself: the Node starts and holds no entry for it.
#[cfg(feature = "pair-zec")]
#[test]
fn a_persisted_zcash_swap_that_no_longer_verifies_is_left_out_and_the_node_starts() {
    let bad_key_home = tempfile::tempdir().unwrap();
    let bad_key = private_file(bad_key_home.path().join("bad-key.bin"), &[0; 32]);
    for (field, value) in [
        ("/signed_envelope/sha256", json!("00".repeat(32))),
        ("/unsigned_draft/sha256", json!("11".repeat(32))),
        ("/source_config/sha256", json!("22".repeat(32))),
        ("/offer_id", json!("m6-offer-002")),
        ("/lez_units", json!(85)),
        ("/source_id", json!("missing")),
        ("/signing_key/path", json!(bad_key)),
        ("/request_id", json!("dynamic-client-request")),
    ] {
        let fixture = Fixture::new_with(Pair::Zcash, SwapDirection::TakerSellsLez);
        let mut entry = fixture.entry.clone();
        if let Some(slot) = entry.pointer_mut(field) {
            *slot = value;
        } else {
            entry[field.trim_start_matches('/')] = value;
        }
        support::persist_take_time_swap(&fixture.root, &entry);
        let context = load_taker_service_context(&fixture.config)
            .unwrap_or_else(|error| panic!("{field}: the Node must start: {error}"));
        assert_eq!(context.initiation().unwrap().prepared_count(), 0, "{field}");
    }
}

struct Fixture {
    _run: tempfile::TempDir,
    root: PathBuf,
    config: PathBuf,
    value: Value,
    /// The prepared swap: the `prepared_btc` entry, or the Zcash record
    /// persisted in the swap's directory.
    #[allow(dead_code)]
    entry: Value,
    maker: [u8; 33],
}

impl Fixture {
    fn new() -> Self {
        Self::new_with(Pair::Bitcoin, SwapDirection::TakerSellsLez)
    }

    fn new_with(pair: Pair, direction: SwapDirection) -> Self {
        let run = tempfile::tempdir().unwrap();
        let root = private_directory(run.path().join("fixture"));
        let delivery = private_directory(root.join("delivery"));
        let registry = root.join("registry.sqlite3");
        drop(SqliteTakerFacadeStore::create_new(&registry).unwrap());
        let maker_secret = SecretKey::from_slice(&[7; 32]).unwrap();
        let maker =
            PublicKey::from_secret_key(&Secp256k1::signing_only(), &maker_secret).serialize();
        let publisher = RunLocalDelivery::publisher(delivery.clone(), maker_secret).unwrap();
        let authenticated = publisher
            .publish_or_verify(&DeliveryPublicationV1::new(
                prepared_offer(&root, pair, direction),
                1_000,
            ))
            .unwrap();
        let signed = private_file(root.join("signed.json"), authenticated.signed_envelope());
        let draft = private_file(root.join("draft.json"), br#"{"unsigned":"draft"}"#);
        let key = private_file(root.join("key.bin"), &[42; 32]);
        let actor = private_file(root.join("actor.json"), br#"{"role":"taker"}"#);
        let chat = root.join("chat.sock");
        let entry = json!({
            "source_id": "maker-a",
            "swap_id": SWAP,
            "offer_id": OFFER,
            "reservation_id": RESERVATION,
            "foreign_units": 42,
            "lez_units": 84,
            "signed_envelope": digest_binding(&signed),
            "unsigned_draft": digest_binding(&draft),
            "signing_key": {"path": key},
            "source_config": digest_binding(&actor),
            "agreement_output": root.join("agreement.json"),
            "actor_root": root.join("actor-root"),
            "receipt_output": root.join("receipt.json"),
        });
        let initiation = match pair {
            #[cfg(feature = "pair-zec")]
            Pair::Zcash => {
                support::persist_take_time_swap(&root, &entry);
                support::take_time_initiation(&root, &registry, &key)
            }
            _ => json!({"registry_database": registry, "prepared_btc": [entry.clone()]}),
        };
        let value = json!({
            "schema_version": 1,
            "delivery_sources": [{
                "source_id": "maker-a",
                "directory": delivery,
                "maker_public_key": hex::encode(maker),
            }],
            "chat_socket": chat,
            "maximum_offers": 16,
            "initiation": initiation,
        });
        let config = private_json(root.join("service.json"), &value);
        Self {
            _run: run,
            root,
            config,
            value,
            entry,
            maker,
        }
    }

    fn write_config(&self, name: &str, value: &Value) -> PathBuf {
        private_json(self.root.join(format!("{name}.json")), value)
    }

    fn assert_invalid(&self, name: &str, value: &Value, expected: TakerServiceStartupError) {
        let error = load_taker_service_context(&self.write_config(name, value)).unwrap_err();
        assert_eq!(error, expected);
        assert!(!error.to_string().contains(self.root.to_str().unwrap()));
        assert!(!format!("{error:?}").contains(self.root.to_str().unwrap()));
    }
}

fn prepared_offer(
    root: &Path,
    pair: Pair,
    direction: SwapDirection,
) -> lez_swap_store::MakerOfferV1 {
    let route = MakerRouteV1::new(pair, direction).unwrap();
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
            &MakerOfferId::new(OFFER).unwrap(),
            route,
            1_000,
        )
        .unwrap();
    store.list_discoverable_maker_offers(1_000).unwrap()[0]
        .offer()
        .clone()
}
fn digest_binding(path: &Path) -> Value {
    json!({"path": path, "sha256": hex::encode(Sha256::digest(fs::read(path).unwrap()))})
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
