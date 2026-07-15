use std::fs::{self, hard_link};
use std::os::unix::fs::{PermissionsExt as _, symlink};
use std::time::Duration;

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
use jsonrpsee::core::ClientError;
use jsonrpsee_http_client::types::ErrorObjectOwned;
use lez_btc_core_adapter::{
    BitcoinCoreRpc, HttpBitcoinCoreConfig, HttpBitcoinCoreError, HttpBitcoinCoreRpc, SendFailure,
};

fn authenticated_config(endpoint: &str) -> HttpBitcoinCoreConfig {
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("cookie");
    fs::write(&path, b"user:password").expect("cookie file");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).expect("owner-only cookie mode");
    HttpBitcoinCoreConfig::new(endpoint)
        .expect("literal loopback endpoint")
        .with_cookie_file(path)
        .expect("valid file-backed credential")
}

#[test]
fn endpoint_is_literal_loopback_http_root_with_explicit_nonzero_port() {
    for endpoint in ["http://127.0.0.1:18443", "http://[::1]:18443/"] {
        let config = authenticated_config(endpoint);
        HttpBitcoinCoreRpc::connect(&config)
            .expect("client construction is local and does not connect");
    }

    for endpoint in [
        "http://localhost:18443",
        "http://127.0.0.2:18443",
        "http://0.0.0.0:18443",
        "http://2130706433:18443",
        "http://0177.0.0.1:18443",
        "http://[0:0:0:0:0:0:0:1]:18443",
        "https://127.0.0.1:18443",
        "HTTP://127.0.0.1:18443",
        "http://127.0.0.1",
        "http://127.0.0.1:0",
        "http://127.0.0.1:18443/rpc",
        "http://127.0.0.1:18443/?secret=query",
        "http://127.0.0.1:18443/#fragment",
        "http://user:password@127.0.0.1:18443",
        "http://example.test:18443",
        "not-a-url",
    ] {
        assert!(matches!(
            HttpBitcoinCoreConfig::new(endpoint),
            Err(HttpBitcoinCoreError::NonLoopbackEndpoint)
        ));
    }
    assert!(matches!(
        HttpBitcoinCoreConfig::new(format!("http://{}.example.test:18443", "a".repeat(2_048))),
        Err(HttpBitcoinCoreError::NonLoopbackEndpoint)
    ));
}

#[test]
fn cookie_file_is_bounded_owner_private_regular_and_secret_free_in_diagnostics() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let cookie_path = directory.path().join(".cookie");
    fs::write(&cookie_path, b"__cookie__:top-secret\n").expect("cookie file");
    fs::set_permissions(&cookie_path, fs::Permissions::from_mode(0o600))
        .expect("owner-only cookie mode");

    let config = HttpBitcoinCoreConfig::new("http://127.0.0.1:18443")
        .expect("loopback endpoint")
        .with_cookie_file(&cookie_path)
        .expect("valid Core cookie file");
    let config_debug = format!("{config:?}");
    assert!(config_debug.contains("cookie_auth_enabled: true"));
    assert!(!config_debug.contains("top-secret"));
    assert!(!config_debug.contains(&BASE64_STANDARD.encode(b"__cookie__:top-secret")));
    assert!(!config_debug.contains(cookie_path.to_string_lossy().as_ref()));

    let rpc = HttpBitcoinCoreRpc::connect(&config).expect("bounded local client");
    let rpc_debug = format!("{rpc:?}");
    assert!(!rpc_debug.contains("top-secret"));
    assert!(!rpc_debug.contains(cookie_path.to_string_lossy().as_ref()));

    for (name, contents) in [
        ("empty", b"".as_slice()),
        ("delimiter", b"missing-delimiter".as_slice()),
        ("empty-user", b":password".as_slice()),
        ("empty-password", b"username:".as_slice()),
        ("space", b"user:has space".as_slice()),
        ("two-newlines", b"user:secret\n\n".as_slice()),
    ] {
        let path = directory.path().join(name);
        fs::write(&path, contents).expect("invalid cookie fixture");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
            .expect("owner-only fixture mode");
        let error = HttpBitcoinCoreConfig::new("http://127.0.0.1:18443")
            .expect("loopback endpoint")
            .with_cookie_file(&path)
            .expect_err("invalid credential rejected");
        assert!(matches!(error, HttpBitcoinCoreError::InvalidCookieFile));
        let diagnostic = format!("{error:?} {error}");
        assert!(!diagnostic.contains(path.to_string_lossy().as_ref()));
        assert!(!diagnostic.contains("has space"));
    }

    let oversized = directory.path().join("oversized");
    fs::write(&oversized, vec![b'a'; 1_025]).expect("oversized fixture");
    fs::set_permissions(&oversized, fs::Permissions::from_mode(0o600))
        .expect("owner-only fixture mode");
    assert!(matches!(
        HttpBitcoinCoreConfig::new("http://127.0.0.1:18443")
            .expect("loopback endpoint")
            .with_cookie_file(&oversized),
        Err(HttpBitcoinCoreError::InvalidCookieFile)
    ));

    let exposed = directory.path().join("exposed");
    fs::write(&exposed, b"user:secret").expect("exposed fixture");
    fs::set_permissions(&exposed, fs::Permissions::from_mode(0o640))
        .expect("group-readable fixture mode");
    assert!(matches!(
        HttpBitcoinCoreConfig::new("http://127.0.0.1:18443")
            .expect("loopback endpoint")
            .with_cookie_file(&exposed),
        Err(HttpBitcoinCoreError::InsecureCookieFile)
    ));

    let executable = directory.path().join("executable");
    fs::write(&executable, b"user:secret").expect("executable fixture");
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o700))
        .expect("owner-executable fixture mode");
    assert!(matches!(
        HttpBitcoinCoreConfig::new("http://127.0.0.1:18443")
            .expect("loopback endpoint")
            .with_cookie_file(&executable),
        Err(HttpBitcoinCoreError::InsecureCookieFile)
    ));

    let hardlinked = directory.path().join("hardlinked-cookie");
    hard_link(&cookie_path, &hardlinked).expect("cookie hard link");
    assert!(matches!(
        HttpBitcoinCoreConfig::new("http://127.0.0.1:18443")
            .expect("loopback endpoint")
            .with_cookie_file(&cookie_path),
        Err(HttpBitcoinCoreError::InsecureCookieFile)
    ));

    let linked = directory.path().join("linked-cookie");
    symlink(&cookie_path, &linked).expect("cookie symlink");
    assert!(matches!(
        HttpBitcoinCoreConfig::new("http://127.0.0.1:18443")
            .expect("loopback endpoint")
            .with_cookie_file(&linked),
        Err(HttpBitcoinCoreError::InsecureCookieFile)
    ));

    assert!(matches!(
        HttpBitcoinCoreConfig::new("http://127.0.0.1:18443")
            .expect("loopback endpoint")
            .with_cookie_file(directory.path()),
        Err(HttpBitcoinCoreError::InsecureCookieFile)
    ));
}

#[test]
fn transport_bounds_cannot_be_disabled() {
    let zero_timeout = HttpBitcoinCoreConfig::new("http://127.0.0.1:18443")
        .expect("loopback endpoint")
        .with_request_timeout(Duration::ZERO);
    assert!(matches!(
        HttpBitcoinCoreRpc::connect(&zero_timeout),
        Err(HttpBitcoinCoreError::InvalidTransportBounds)
    ));

    let zero_concurrency = HttpBitcoinCoreConfig::new("http://[::1]:18443")
        .expect("loopback endpoint")
        .with_max_concurrent_requests(0);
    assert!(matches!(
        HttpBitcoinCoreRpc::connect(&zero_concurrency),
        Err(HttpBitcoinCoreError::InvalidTransportBounds)
    ));

    let unbounded_timeout = HttpBitcoinCoreConfig::new("http://127.0.0.1:18443")
        .expect("loopback endpoint")
        .with_request_timeout(Duration::MAX);
    assert!(matches!(
        HttpBitcoinCoreRpc::connect(&unbounded_timeout),
        Err(HttpBitcoinCoreError::InvalidTransportBounds)
    ));

    let unbounded_concurrency = HttpBitcoinCoreConfig::new("http://[::1]:18443")
        .expect("loopback endpoint")
        .with_max_concurrent_requests(usize::MAX);
    assert!(matches!(
        HttpBitcoinCoreRpc::connect(&unbounded_concurrency),
        Err(HttpBitcoinCoreError::InvalidTransportBounds)
    ));
}

#[test]
fn connection_requires_file_backed_basic_credentials() {
    let config =
        HttpBitcoinCoreConfig::new("http://127.0.0.1:18443").expect("literal loopback endpoint");
    assert!(matches!(
        HttpBitcoinCoreRpc::connect(&config),
        Err(HttpBitcoinCoreError::MissingCookieCredentials)
    ));
}

#[test]
fn only_explicit_core_validation_codes_are_definitive_send_rejections() {
    for code in [-22, -25, -26] {
        let error = HttpBitcoinCoreError::Request(ClientError::Call(ErrorObjectOwned::owned(
            code, "rejected", None::<()>,
        )));
        assert_eq!(
            <HttpBitcoinCoreRpc as BitcoinCoreRpc>::classify_send_failure(&error),
            SendFailure::DefinitiveRejection
        );
    }
    for code in [-27, -28, -1] {
        let error = HttpBitcoinCoreError::Request(ClientError::Call(ErrorObjectOwned::owned(
            code,
            "ambiguous",
            None::<()>,
        )));
        assert_eq!(
            <HttpBitcoinCoreRpc as BitcoinCoreRpc>::classify_send_failure(&error),
            SendFailure::Unknown
        );
    }
}

#[tokio::test]
async fn outgoing_transaction_body_is_locally_bounded_before_network_io() {
    let rpc = HttpBitcoinCoreRpc::connect(&authenticated_config("http://127.0.0.1:18443"))
        .expect("bounded local client");
    assert!(matches!(
        rpc.test_mempool_accept(&[]).await,
        Err(HttpBitcoinCoreError::MalformedOutgoingTransaction)
    ));
    assert!(matches!(
        rpc.send_raw_transaction(&vec![0_u8; 1_000_001]).await,
        Err(HttpBitcoinCoreError::MalformedOutgoingTransaction)
    ));
}
