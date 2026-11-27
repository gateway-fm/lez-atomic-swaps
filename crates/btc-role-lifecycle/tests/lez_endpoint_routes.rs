//! The role configuration's LEZ sequencer and indexer URLs are checked against
//! the configured route when the configuration is loaded, and the Node's own
//! readers of the two services connect only to what that check passed.

use std::{
    fs::{self, DirBuilder, OpenOptions},
    io::Write as _,
    net::TcpListener,
    os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use lez_btc_role_lifecycle::{
    BtcRoleRuntime, LezFinalityWatch, LezNodeEndpoint, LezSequencerRoute,
    OFFICIAL_PUBLIC_LEZ_ENDPOINT, balances::lez_owner_sequenced_balance,
};
use lez_swap_core::Participant;

fn private_dir(path: &Path) {
    DirBuilder::new().mode(0o700).create(path).unwrap();
}

fn private_file(path: &Path, bytes: &[u8]) {
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .unwrap();
    file.write_all(bytes).unwrap();
}

/// A role root whose configuration is rewritten per case.
struct Role {
    _directory: tempfile::TempDir,
    root: PathBuf,
}

impl Role {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(directory.path()).unwrap().join("role");
        private_dir(&root);
        private_dir(&root.join("swaps"));
        private_file(&root.join("cookie"), b"user:pass\n");
        private_file(
            &root.join("lez-signer.key"),
            format!("{}\n", hex::encode([0x77; 32])).as_bytes(),
        );
        Self {
            _directory: directory,
            root,
        }
    }

    /// Loads the role with the given route (`None`: the field is absent, which
    /// means `local`) and LEZ endpoints.
    fn load(
        &self,
        route: Option<&str>,
        sequencer: &str,
        indexer: &str,
    ) -> Result<BtcRoleRuntime, String> {
        let mut config = serde_json::json!({
            "schema_version": 1,
            "swaps_root": self.root.join("swaps"),
            "bitcoin": {
                "network": "regtest",
                "endpoint": "http://127.0.0.1:18443/",
                "cookie_file": self.root.join("cookie"),
                "genesis_block_hash": "0f9188f13cb7b2c71f2a335e3a4fc328bf5beb436012afca590b1a11466e2206",
                "required_confirmations": 2,
                "refund_csv_blocks": 144,
                "second_lock_refund_csv_blocks": 7,
                "claim_fee_sat": 1000
            },
            "lez": {
                "channel_id": hex::encode([0x23; 32]),
                "genesis_block_hash": hex::encode([0x22; 32]),
                "escrow_program_id": hex::encode([0x24; 32]),
                "authenticated_transfer_program_id": hex::encode([0x25; 32]),
                "sidecar_program": "/usr/local/bin/lez-v02-bridge-poc",
                "sequencer_url": sequencer,
                "indexer_url": indexer,
                "sidecar_port_base": 19000,
                "sidecar_port_count": 100,
                "signer_key_file": self.root.join("lez-signer.key"),
                "request_timeout_millis": 2000,
                "discovery_max_blocks": 512
            },
            "recovery": {
                "maker_second_lock_cutoff_seconds": 1800,
                "earlier_refund_latest_seconds": 3600,
                "later_refund_earliest_seconds": 7200,
                "required_margin_seconds": 300
            },
            "actor": { "program": "/usr/local/bin/lez-btc-actor", "program_sha256": hex::encode([0x11; 32]) }
        });
        if let Some(route) = route {
            config["lez"]["sequencer_route"] = route.into();
        }
        let path = self.root.join("btc-role.json");
        private_file(&path, &serde_json::to_vec_pretty(&config).unwrap());
        BtcRoleRuntime::load(Participant::Taker, &path).map_err(|error| format!("{error:#}"))
    }
}

const LOOPBACK_INDEXER: &str = "http://127.0.0.1:8779/";

/// What neither route accepts for the sequencer.
const REFUSED_BY_BOTH: &[&str] = &[
    // a name, the devnet's service name and `localhost` included
    "http://localhost:3040/",
    "http://sequencer:3040",
    "http://127.0.0.1.example.com:3040/",
    // not loopback
    "http://192.0.2.1:3040/",
    "http://[2001:db8::1]:3040/",
    // another scheme, no port, credentials, path, query, fragment
    "https://127.0.0.1:3040/",
    "http://127.0.0.1/",
    "http://user:secret@127.0.0.1:3040/",
    "http://127.0.0.1:3040/rpc",
    "http://127.0.0.1:3040/?to=https://testnet.lez.logos.co/",
    "http://127.0.0.1:3040/#https://testnet.lez.logos.co/",
    // the official origin, altered
    "http://testnet.lez.logos.co/",
    "https://testnet.lez.logos.co",
    "https://testnet.lez.logos.co:8443/",
    "https://user:secret@testnet.lez.logos.co/",
    "https://testnet.lez.logos.co/rpc",
    "https://testnet.lez.logos.co/?a=1",
    "https://testnet.lez.logos.co/#a",
    "https://testnet.lez.logos.co.example.com/",
    "https://testnet.lez.logos.co@example.com/",
    "https://example.com/",
];

#[test]
fn the_local_route_loads_only_a_literal_loopback_sequencer() {
    let role = Role::new();
    for route in [None, Some("local")] {
        for accepted in [
            "http://127.0.0.1:3040",
            "http://127.0.0.1:3040/",
            "http://[::1]:3040/",
        ] {
            let runtime = role
                .load(route, accepted, LOOPBACK_INDEXER)
                .unwrap_or_else(|error| panic!("{route:?} {accepted}: {error}"));
            assert_eq!(runtime.lez_sequencer().as_str(), accepted);
            assert_eq!(runtime.lez_indexer().as_str(), LOOPBACK_INDEXER);
        }
        for refused in REFUSED_BY_BOTH
            .iter()
            .chain(&[OFFICIAL_PUBLIC_LEZ_ENDPOINT])
        {
            let refusal = role
                .load(route, refused, LOOPBACK_INDEXER)
                .expect_err(refused);
            assert!(
                refusal.contains("lez.sequencer_url") && refusal.contains("local sequencer route"),
                "{route:?} {refused}: {refusal}"
            );
            assert!(!refusal.contains("secret"), "{refusal}");
        }
    }
}

#[test]
fn the_official_public_route_loads_only_the_exact_official_origin() {
    let role = Role::new();
    let runtime = role
        .load(
            Some("official_public"),
            OFFICIAL_PUBLIC_LEZ_ENDPOINT,
            LOOPBACK_INDEXER,
        )
        .expect("the official origin with a loopback indexer");
    assert_eq!(
        runtime.lez_sequencer().as_str(),
        "https://testnet.lez.logos.co/"
    );
    for refused in REFUSED_BY_BOTH.iter().chain(&[
        "http://127.0.0.1:3040/",
        "http://127.0.0.1:3040",
        "http://[::1]:3040/",
    ]) {
        let refusal = role
            .load(Some("official_public"), refused, LOOPBACK_INDEXER)
            .expect_err(refused);
        assert!(
            refusal.contains("lez.sequencer_url")
                && refusal.contains("official_public sequencer route"),
            "{refused}: {refusal}"
        );
        assert!(!refusal.contains("secret"), "{refusal}");
    }
    // A route the Node does not know is not a way around either rule.
    role.load(Some("public"), "https://example.com/", LOOPBACK_INDEXER)
        .expect_err("an unknown route");
}

#[test]
fn the_indexer_loads_only_as_literal_loopback_under_both_routes() {
    let role = Role::new();
    for (route, sequencer) in [
        (None, "http://127.0.0.1:3040/"),
        (Some("official_public"), OFFICIAL_PUBLIC_LEZ_ENDPOINT),
    ] {
        role.load(route, sequencer, "http://[::1]:8779/")
            .expect("an IPv6 loopback indexer");
        for refused in [
            "http://localhost:8779/",
            "http://indexer:8779",
            "https://127.0.0.1:8779/",
            "http://127.0.0.1/",
            "http://user:secret@127.0.0.1:8779/",
            "http://127.0.0.1:8779/rpc",
            "http://127.0.0.1:8779/?a=1",
            "http://192.0.2.1:8779/",
            OFFICIAL_PUBLIC_LEZ_ENDPOINT,
        ] {
            let refusal = role.load(route, sequencer, refused).expect_err(refused);
            assert!(
                refusal.contains("lez.indexer_url"),
                "{route:?} {refused}: {refusal}"
            );
        }
    }
}

/// A loopback listener that counts the connections made to it and closes each.
fn counting_listener() -> (u16, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let connections = Arc::new(AtomicUsize::new(0));
    let seen = Arc::clone(&connections);
    std::thread::spawn(move || {
        for connection in listener.incoming() {
            if connection.is_ok() {
                seen.fetch_add(1, Ordering::SeqCst);
            }
        }
    });
    (port, connections)
}

/// `getAccount` (the sequenced nonce) and `getLastBlockId` (the finality
/// stall rule) go to the endpoint the load checked, and a URL the route
/// refuses yields no runtime to read with: nothing ever connects to it.
#[tokio::test]
async fn the_two_sequencer_readers_connect_only_to_a_url_the_route_accepted() {
    let role = Role::new();
    let (port, connections) = counting_listener();
    let listener_url = format!("http://127.0.0.1:{port}/");

    // The listener stands in for an endpoint the route does not allow: on
    // loopback but under the official_public route, and by name under local.
    for (route, sequencer) in [
        (Some("official_public"), listener_url.clone()),
        (Some("local"), format!("http://localhost:{port}/")),
        (None, format!("http://user:secret@127.0.0.1:{port}/")),
        (None, format!("http://127.0.0.1:{port}/?next=1")),
    ] {
        let refusal = role
            .load(route, &sequencer, LOOPBACK_INDEXER)
            .expect_err("a sequencer URL the route refuses");
        assert!(refusal.contains("lez.sequencer_url"), "{refusal}");
    }
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        connections.load(Ordering::SeqCst),
        0,
        "a refused sequencer URL was connected to"
    );

    // The same listener as an accepted local sequencer is what both readers
    // reach; the indexer is a port nothing listens on.
    let runtime = role
        .load(None, &listener_url, "http://127.0.0.1:9/")
        .expect("a literal-loopback sequencer");
    assert!(lez_owner_sequenced_balance(&runtime).await.is_err());
    let after_account = connections.load(Ordering::SeqCst);
    assert!(after_account >= 1, "getAccount did not reach the sequencer");
    let progress = LezFinalityWatch::for_runtime(&runtime, 60).progress().await;
    assert_eq!(progress.tip_block_id, None);
    assert!(
        connections.load(Ordering::SeqCst) > after_account,
        "getLastBlockId did not reach the sequencer"
    );
}

/// A watch can only be built from checked endpoints, and the checks are the
/// only way to make one.
#[test]
fn a_finality_watch_takes_only_checked_endpoints() {
    let sequencer =
        LezNodeEndpoint::sequencer(LezSequencerRoute::Local, "http://127.0.0.1:3040/").unwrap();
    let indexer = LezNodeEndpoint::indexer(LOOPBACK_INDEXER).unwrap();
    let _watch = LezFinalityWatch::new(indexer, sequencer, 60);
    LezNodeEndpoint::sequencer(LezSequencerRoute::Local, "http://sequencer:3040")
        .expect_err("the devnet service name is not loopback");
}

/// No Node source reads the configured URLs past the load: every client is
/// built from `BtcRoleRuntime::lez_sequencer` or `lez_indexer`.
#[test]
fn no_node_source_connects_to_the_configured_url_text() {
    fn visit(directory: &Path, found: &mut Vec<String>) {
        for entry in fs::read_dir(directory).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                visit(&path, found);
            } else if path.extension().is_some_and(|extension| extension == "rs")
                // The load that checks them, and the check's own messages.
                && !path.ends_with("btc-role-lifecycle/src/config.rs")
                && !path.ends_with("btc-role-lifecycle/src/lez_endpoint.rs")
            {
                let text: String = fs::read_to_string(&path)
                    .unwrap()
                    .split_whitespace()
                    .collect();
                if text.contains(".sequencer_url") || text.contains("lez.indexer_url") {
                    found.push(path.display().to_string());
                }
            }
        }
    }
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let mut found = Vec::new();
    for member in fs::read_dir(crates).unwrap() {
        let source = member.unwrap().path().join("src");
        if source.is_dir() {
            visit(&source, &mut found);
        }
    }
    assert!(
        found.is_empty(),
        "these sources read lez.sequencer_url or lez.indexer_url instead of the checked endpoint: {found:?}"
    );
}
