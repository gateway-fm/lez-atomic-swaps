//! Focused contract for private Taker service startup dependencies.

use std::{
    fs,
    os::unix::{
        fs::{DirBuilderExt as _, PermissionsExt as _, symlink},
        net::UnixListener,
    },
    time::{SystemTime, UNIX_EPOCH},
};

use lez_maker_node::{
    OwnerChatSocketProbe, SystemTakerTrustedTime, TakerDependencyProbe, TakerDependencyStateV1,
    TakerHealthRequestV1, TakerServiceStartupError, TakerTrustedTimeSource,
    load_taker_service_backend,
};
use secp256k1::{PublicKey, Secp256k1, SecretKey};
use serde_json::{Value, json};
use tempfile::tempdir;

#[tokio::test]
async fn valid_private_config_builds_redacted_backend_and_internal_subscriber() {
    let run = tempdir().unwrap();
    let delivery = private_directory(run.path().join("delivery"));
    let chat = run.path().join("chat.sock");
    let maker = maker_identity();
    let config = private_config(
        run.path().join("taker-service.json"),
        &json!({
            "schema_version": 1,
            "delivery_sources": [{
                "directory": delivery,
                "maker_public_key": maker,
            }],
            "chat_socket": chat,
            "maximum_offers": 17,
        }),
    );

    let backend = load_taker_service_backend(&config).unwrap();
    let debug = format!("{backend:?}");
    assert!(debug.contains("delivery_source_count: 1"));
    assert!(debug.contains("chat_configured: true"));
    assert!(debug.contains("maximum_offers: 17"));
    assert!(!debug.contains(run.path().to_str().unwrap()));
    assert!(!debug.contains(&maker));

    let health = backend
        .health(&TakerHealthRequestV1 { schema_version: 1 })
        .await
        .unwrap();
    assert_eq!(health.delivery(), TakerDependencyStateV1::Available);
    assert_eq!(health.chat(), TakerDependencyStateV1::Unavailable);
}

#[test]
fn startup_schema_is_strict_versioned_and_bounded() {
    let run = tempdir().unwrap();
    let delivery = private_directory(run.path().join("delivery"));
    let maker = maker_identity();
    let base = json!({
        "schema_version": 1,
        "delivery_sources": [{
            "directory": delivery,
            "maker_public_key": maker,
        }],
        "maximum_offers": 1,
    });

    assert_invalid(
        &run,
        "unknown",
        &with_field(&base, "unexpected", json!(true)),
    );
    assert_invalid(
        &run,
        "schema",
        &with_field(&base, "schema_version", json!(2)),
    );
    assert_invalid(
        &run,
        "zero-cap",
        &with_field(&base, "maximum_offers", json!(0)),
    );
    assert_invalid(
        &run,
        "large-cap",
        &with_field(&base, "maximum_offers", json!(1_025)),
    );
    assert_invalid(
        &run,
        "relative-delivery",
        &with_source_field(&base, "directory", json!("relative/delivery")),
    );
    assert_invalid(
        &run,
        "uppercase-key",
        &with_source_field(&base, "maker_public_key", json!(maker.to_ascii_uppercase())),
    );
    assert_invalid(
        &run,
        "relative-chat",
        &with_field(&base, "chat_socket", json!("relative/chat.sock")),
    );

    let source = base["delivery_sources"][0].clone();
    let sources = vec![source; 33];
    assert_invalid(
        &run,
        "too-many-sources",
        &with_field(&base, "delivery_sources", json!(sources)),
    );
}

#[test]
fn startup_file_and_delivery_failures_are_fixed_and_path_free() {
    let run = tempdir().unwrap();
    let delivery = private_directory(run.path().join("delivery"));
    let config = run.path().join("unsafe-config.json");
    fs::write(
        &config,
        serde_json::to_vec(&json!({
            "schema_version": 1,
            "delivery_sources": [],
            "maximum_offers": 1,
        }))
        .unwrap(),
    )
    .unwrap();
    fs::set_permissions(&config, fs::Permissions::from_mode(0o644)).unwrap();
    assert_fixed_error(
        load_taker_service_backend(&config).unwrap_err(),
        TakerServiceStartupError::ConfigurationUnavailable,
        run.path().to_str().unwrap(),
    );

    fs::set_permissions(&delivery, fs::Permissions::from_mode(0o755)).unwrap();
    let config = private_config(
        run.path().join("delivery-unavailable.json"),
        &json!({
            "schema_version": 1,
            "delivery_sources": [{
                "directory": delivery,
                "maker_public_key": maker_identity(),
            }],
            "maximum_offers": 1,
        }),
    );
    assert_fixed_error(
        load_taker_service_backend(&config).unwrap_err(),
        TakerServiceStartupError::DeliveryUnavailable,
        run.path().to_str().unwrap(),
    );
}

#[test]
fn chat_probe_is_dynamic_payload_free_and_rejects_aliases_or_wrong_modes() {
    let run = tempdir().unwrap();
    let socket = run.path().join("chat.sock");
    let probe = OwnerChatSocketProbe::new(socket.clone()).unwrap();
    let debug = format!("{probe:?}");
    assert!(!debug.contains(run.path().to_str().unwrap()));
    assert!(!probe.is_available());

    fs::write(&socket, b"not a socket").unwrap();
    fs::set_permissions(&socket, fs::Permissions::from_mode(0o600)).unwrap();
    assert!(!probe.is_available());
    fs::remove_file(&socket).unwrap();

    let listener = UnixListener::bind(&socket).unwrap();
    fs::set_permissions(&socket, fs::Permissions::from_mode(0o660)).unwrap();
    assert!(!probe.is_available());
    fs::set_permissions(&socket, fs::Permissions::from_mode(0o600)).unwrap();
    assert!(probe.is_available());

    let alias = run.path().join("chat-alias.sock");
    symlink(&socket, &alias).unwrap();
    let alias_probe = OwnerChatSocketProbe::new(alias).unwrap();
    assert!(!alias_probe.is_available());
    drop(listener);
}

#[test]
fn chat_probe_requires_an_absolute_configured_path() {
    let error = OwnerChatSocketProbe::new("relative/chat.sock".into()).unwrap_err();
    assert_eq!(error, TakerServiceStartupError::InvalidConfiguration);
    assert_eq!(
        error.to_string(),
        "Taker service startup configuration is invalid"
    );
}

#[test]
fn system_trusted_time_supplies_current_unix_seconds() {
    let before = unix_seconds();
    let observed = SystemTakerTrustedTime.now_unix_seconds().unwrap();
    let after = unix_seconds();
    assert!((before..=after).contains(&observed));
}

fn assert_invalid(run: &tempfile::TempDir, name: &str, value: &Value) {
    let config = private_config(run.path().join(format!("{name}.json")), value);
    assert_fixed_error(
        load_taker_service_backend(&config).unwrap_err(),
        TakerServiceStartupError::InvalidConfiguration,
        run.path().to_str().unwrap(),
    );
}

fn assert_fixed_error(
    error: TakerServiceStartupError,
    expected: TakerServiceStartupError,
    path: &str,
) {
    assert_eq!(error, expected);
    assert!(!error.to_string().contains(path));
    assert!(!format!("{error:?}").contains(path));
}

fn private_config(path: std::path::PathBuf, value: &Value) -> std::path::PathBuf {
    fs::write(&path, serde_json::to_vec(value).unwrap()).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    path
}

fn private_directory(path: std::path::PathBuf) -> std::path::PathBuf {
    fs::DirBuilder::new().mode(0o700).create(&path).unwrap();
    path
}

fn maker_identity() -> String {
    let key = PublicKey::from_secret_key(
        &Secp256k1::signing_only(),
        &SecretKey::from_slice(&[7_u8; 32]).unwrap(),
    );
    hex::encode(key.serialize())
}

fn with_field(base: &Value, field: &str, value: Value) -> Value {
    let mut updated = base.clone();
    updated[field] = value;
    updated
}

fn with_source_field(base: &Value, field: &str, value: Value) -> Value {
    let mut updated = base.clone();
    updated["delivery_sources"][0][field] = value;
    updated
}

fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}
