//! What a Maker and a Taker need to agree a Zcash swap at take time
//! (`zec_reserve_v1`) without a chain: a fake Zebra that serves only what the
//! round reads (the chain's tip, one mature funder output), a fake LEZ sidecar
//! that serves a finalized clock, and each role's Bitcoin and Zcash role
//! configuration. A test file includes this module by path and, when its
//! Nodes complete a take, declares the ignored test `fake_sidecar_process`
//! that calls [`serve_fake_sidecar`]: the sidecar the Nodes spawn is the test
//! binary itself.
#![allow(dead_code)]

use std::{
    collections::HashMap,
    ffi::OsString,
    fs::{self, OpenOptions},
    io::Write as _,
    net::SocketAddr,
    os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _},
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use jsonrpsee::{RpcModule, server::ServerBuilder, types::ErrorObjectOwned};
use lez_bridge_protocol::{
    ChainClock, Hex32, METHOD_OBSERVE_FINALIZED_CLOCK, ObserveFinalizedClockRequest,
    ObserveFinalizedClockResult, RequestId,
};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use zcash_primitives::{block::BlockHash, transaction::Transaction};
use zcash_protocol::consensus::BranchId;

/// The coinbase of height 1 on the pinned Zebra 7 Regtest fixture: output 0
/// pays 208,333,333 zatoshis to the key `[4; 32]` (zec-reference-actor
/// `local_poc.rs`, `REGTEST_NU7_COINBASE`).
const REGTEST_NU7_COINBASE: &str = concat!(
    "0600008098b684d8d90a19770000000001000000010000000000000000000000000000000000000000000000",
    "000000000000000000ffffffff285126f09fa6933a2039601c2a397699d2de9efe8dbc1c15ea18b323e3bd7f",
    "9ae732933ca8eb11e3f4ffffffff0115ea6a0c000000001976a914888dbbd7998f9f80d4f82b4f9ce6d5882f",
    "310aec88ac00000000",
);
/// The Regtest fixture's genesis, internal byte order (zebra-node-adapter
/// `ZebraChainIdentity::deterministic_regtest_nu6_2`).
const REGTEST_GENESIS: [u8; 32] = [
    0x27, 0xe3, 0x01, 0x34, 0xd6, 0x20, 0xe9, 0xfe, 0x61, 0xf7, 0x19, 0x93, 0x83, 0x20, 0xba, 0xb6,
    0x3e, 0x7e, 0x72, 0xc9, 0x1b, 0x5e, 0x23, 0x02, 0x56, 0x76, 0xf9, 0x0e, 0xd8, 0x11, 0x9f, 0x02,
];
/// The key the fake Zebra's one output pays: the role that funds Zcash (the
/// Maker when the Taker sells LEZ, the Taker when it sells Zcash) holds it.
pub const FUNDER_KEY: u8 = 4;
/// The key of the role that funds nothing on Zcash.
pub const OTHER_KEY: u8 = 2;
const ZEBRA_TIP: u32 = 120;
/// The principal must exceed the agreement's 10,000-zatoshi claim fee.
pub const FOREIGN_UNITS: u64 = 1_000_000;
/// [`FOREIGN_UNITS`] at the price 5 LEZ units per 2 zatoshis.
pub const LEZ_UNITS: u128 = 2_500_000;

fn block_hash(height: u32) -> BlockHash {
    if height == 0 {
        return BlockHash(REGTEST_GENESIS);
    }
    let mut hash = [0_u8; 32];
    hash[..4].copy_from_slice(&height.to_le_bytes());
    hash[31] = 1;
    BlockHash(hash)
}

/// A Regtest Zebra on NU7 whose block 1 holds the fixture coinbase, unspent
/// and `ZEBRA_TIP` deep, and whose other blocks are empty.
pub async fn start_fake_zebra() -> String {
    let transaction = hex::decode(REGTEST_NU7_COINBASE).unwrap();
    let parsed = Transaction::read(transaction.as_slice(), BranchId::Nu7).unwrap();
    let txid = parsed.txid().to_string();
    let output = parsed.transparent_bundle().unwrap().vout[0].clone();
    let heights: HashMap<String, u32> = (0..=ZEBRA_TIP)
        .map(|height| (block_hash(height).to_string(), height))
        .collect();
    let state = Arc::new((heights, txid, transaction, output));
    let mut module = RpcModule::new(state);
    module
        .register_method("getblockchaininfo", |_, _, _| {
            Ok::<_, ErrorObjectOwned>(json!({
                "chain": "test",
                "blocks": ZEBRA_TIP,
                "bestblockhash": block_hash(ZEBRA_TIP).to_string(),
                "consensus": {"chaintip": format!("{:08x}", u32::from(BranchId::Nu7))},
            }))
        })
        .unwrap();
    module
        .register_method("getblockhash", |params, _, _| {
            let (height,): (u32,) = params.parse()?;
            Ok::<_, ErrorObjectOwned>(block_hash(height).to_string())
        })
        .unwrap();
    module
        .register_method("getblock", |params, state, _| {
            let (hash, _): (String, u8) = params.parse()?;
            let height = state.0[&hash];
            let tx = if height == 1 {
                vec![state.1.clone()]
            } else {
                Vec::new()
            };
            Ok::<_, ErrorObjectOwned>(json!({"hash": hash, "height": height, "tx": tx}))
        })
        .unwrap();
    module
        .register_method("getrawtransaction", |params, state, _| {
            let values: Vec<Value> = params.parse()?;
            if values[0] != state.1 {
                return Err(ErrorObjectOwned::owned(
                    -5,
                    "unknown transaction",
                    None::<()>,
                ));
            }
            Ok(hex::encode(&state.2))
        })
        .unwrap();
    module
        .register_method("gettxout", |params, state, _| {
            let (txid, index, _): (String, u32, bool) = params.parse()?;
            if txid != state.1 || index != 0 {
                return Ok::<_, ErrorObjectOwned>(Value::Null);
            }
            let zatoshis = u64::from(state.3.value());
            Ok(json!({
                "bestblock": block_hash(ZEBRA_TIP).to_string(),
                "confirmations": ZEBRA_TIP,
                "value": serde_json::Number::from_f64(f64::from(u32::try_from(zatoshis).unwrap()) / 1e8).unwrap(),
                "scriptPubKey": {"hex": hex::encode(&state.3.script_pubkey().0.0)},
            }))
        })
        .unwrap();
    let server = ServerBuilder::default().build("127.0.0.1:0").await.unwrap();
    let address = server.local_addr().unwrap();
    let handle = server.start(module);
    // The server lives as long as the test process.
    std::mem::forget(handle);
    format!("http://{address}")
}

/// A LEZ sequencer and indexer in one: a chain whose tip and finalized block
/// stand at the fake sidecar's height, which a Node's health reads as a chain
/// with nothing to finalize, not as a degraded one.
pub async fn start_fake_lez_node() -> String {
    let mut module = RpcModule::new(());
    for method in ["getLastFinalizedBlockId", "getLastBlockId"] {
        module
            .register_method(method, |_, (), _| Ok::<_, ErrorObjectOwned>(70_u64))
            .unwrap();
    }
    let server = ServerBuilder::default().build("127.0.0.1:0").await.unwrap();
    let address = server.local_addr().unwrap();
    // The server lives as long as the test process.
    std::mem::forget(server.start(module));
    format!("http://{address}")
}

/// The swap sidecar the Nodes spawn (btc-role-lifecycle sidecar.rs): this
/// test binary again, as its ignored test `fake_sidecar_process`, until the
/// run directory `root` is removed.
pub fn fake_sidecar_program(root: &Path) -> PathBuf {
    let script = root.join("fake-sidecar.sh");
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o700)
        .open(&script)
        .unwrap();
    writeln!(
        file,
        "#!/bin/sh\nLEZ_FAKE_SIDECAR_ARGS=\"$*\" LEZ_FAKE_SIDECAR_RUN='{}' exec '{}' --exact \
         fake_sidecar_process --nocapture --include-ignored --test-threads 1",
        root.display(),
        std::env::current_exe().unwrap().display()
    )
    .unwrap();
    script
}

/// The body of a test file's ignored `fake_sidecar_process`: serves one
/// swap's finalized LEZ clock on the address the Node chose. Returns at once
/// when the test binary was not started by [`fake_sidecar_program`].
pub fn serve_fake_sidecar() {
    let Ok(arguments) = std::env::var("LEZ_FAKE_SIDECAR_ARGS") else {
        return;
    };
    let run = PathBuf::from(std::env::var("LEZ_FAKE_SIDECAR_RUN").unwrap());
    let mut words = arguments.split_whitespace();
    let address: SocketAddr = words
        .by_ref()
        .skip_while(|word| *word != "--listen-address")
        .nth(1)
        .unwrap()
        .parse()
        .unwrap();
    let runtime = tokio::runtime::Runtime::new().unwrap();
    runtime.block_on(async move {
        let mut module = RpcModule::new(());
        module
            .register_method(METHOD_OBSERVE_FINALIZED_CLOCK, |params, (), _| {
                let request: ObserveFinalizedClockRequest = params.one()?;
                Ok::<_, ErrorObjectOwned>(ObserveFinalizedClockResult::new(
                    request.context,
                    request.runtime,
                    ChainClock::new(Hex32::from_bytes([70; 32]), 70, 1_850_000_000_070),
                ))
            })
            .unwrap();
        let server = ServerBuilder::default().build(address).await.unwrap();
        let _handle = server.start(module);
        println!("{{\"event\":\"ready\"}}");
        while run.exists() {
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    });
}

/// The Node-owned lifecycle's role configuration under `home`: the LEZ role
/// (identity, signer, swaps root, sidecar recipe) a take-time Zcash swap
/// shares with it. No chain is contacted when a Node loads it. `lez_node` is
/// the role's sequencer and indexer ([`start_fake_lez_node`]); without one
/// nothing answers there and the Node's health reports the LEZ chain degraded.
pub fn btc_role_config(
    home: &Path,
    sidecar: &Path,
    port_base: u16,
    signer: u8,
    lez_node: Option<&str>,
) -> PathBuf {
    let swaps = private_dir(&home.join("swaps"));
    write_private(&home.join("cookie"), b"user:pass\n");
    write_private(
        &home.join("lez-signer.key"),
        format!("{}\n", hex::encode([signer; 32])).as_bytes(),
    );
    let path = home.join("btc-role.json");
    write_private(
        &path,
        &serde_json::to_vec(&json!({
            "schema_version": 1,
            "swaps_root": swaps,
            "bitcoin": {
                "network": "regtest",
                "endpoint": "http://127.0.0.1:18443/",
                "cookie_file": home.join("cookie"),
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
                "sidecar_program": sidecar,
                "sequencer_url": lez_node.unwrap_or("http://127.0.0.1:3040"),
                "indexer_url": lez_node.unwrap_or("http://127.0.0.1:8779"),
                "sidecar_port_base": port_base,
                "sidecar_port_count": 10,
                "signer_key_file": home.join("lez-signer.key"),
                "request_timeout_millis": 5000,
                "discovery_max_blocks": 512
            },
            "recovery": {
                "maker_second_lock_cutoff_seconds": 1800,
                "earlier_refund_latest_seconds": 3600,
                "later_refund_earliest_seconds": 7200,
                "required_margin_seconds": 300
            },
            "actor": {"program": "/usr/bin/true", "program_sha256": hex::encode([0x11; 32])}
        }))
        .unwrap(),
    );
    path
}

/// A role's Zcash configuration: the devnet profile and its Zebra. The Taker
/// names its own Zcash key; the Maker's is its Delivery signing key.
pub fn zec_role_config(home: &Path, zebra: &str, zcash_key: Option<&Path>) -> PathBuf {
    let path = home.join("zec-role.json");
    let mut config = json!({
        "schema_version": 1,
        "profile": "deterministic-local-devnet-v5",
        "zebra_endpoint": zebra,
    });
    if let Some(key) = zcash_key {
        config["zcash_key_file"] = json!(key);
    }
    write_private(&path, &serde_json::to_vec(&config).unwrap());
    path
}

/// The Zebra endpoint of a Maker whose test never reserves: nothing answers
/// there, and a Maker contacts its Zebra only for a reservation or a balance.
pub const UNSERVED_ZEBRA: &str = "http://127.0.0.1:18232";

/// A Maker's take-time Zcash authority under `home`, as the role launcher
/// gives it (`modules/lez-role-node/node-entrypoint.sh`): the Node-owned
/// lifecycle's role, a Zcash role naming its Zebra, the claim-recovery key
/// and where the Node keeps the actor of each swap it agrees.
pub struct TakeTimeMaker {
    pub claim_key: PathBuf,
    pub actor_root: PathBuf,
    arguments: Vec<OsString>,
}

impl TakeTimeMaker {
    /// Writes the authority's files under `home` (an owner-private directory
    /// this call creates) on the first call and names the same files on a
    /// later one, so a restarted Maker gets the authority it had.
    pub fn new(
        home: &Path,
        zebra: &str,
        sidecar: &Path,
        port_base: u16,
        lez_node: Option<&str>,
    ) -> Self {
        let btc_key = home.join("btc-chat-signing.key");
        let claim_key = home.join("zec-claim-recovery.key");
        let actor_root = home.join("actors");
        if !home.exists() {
            let home = private_dir(home);
            write_private(&btc_key, &[0x31; 32]);
            write_private(&claim_key, &[0x7a; 32]);
            private_dir(&actor_root);
            btc_role_config(&home, sidecar, port_base, 0x77, lez_node);
            zec_role_config(&home, zebra, None);
        }
        let home = fs::canonicalize(home).unwrap();
        let actor_program = fs::canonicalize("/usr/bin/true").unwrap();
        let actor_sha = hex::encode(Sha256::digest(fs::read(&actor_program).unwrap()));
        let claim_key = home.join("zec-claim-recovery.key");
        let actor_root = home.join("actors");
        let arguments = vec![
            "--btc-maker-signing-key-file".into(),
            home.join("btc-chat-signing.key").into(),
            "--btc-role-config".into(),
            home.join("btc-role.json").into(),
            "--maker-claim-key-id".into(),
            "zec-take-time-claim-v1".into(),
            "--maker-claim-key-file".into(),
            claim_key.clone().into(),
            "--zec-role-config".into(),
            home.join("zec-role.json").into(),
            "--zec-maker-actor-root".into(),
            actor_root.clone().into(),
            "--zec-actor-program".into(),
            actor_program.into(),
            "--zec-actor-program-sha256".into(),
            actor_sha.into(),
        ];
        Self {
            claim_key,
            actor_root,
            arguments,
        }
    }

    /// The Maker Node's flags for this authority.
    pub fn arguments(&self) -> &[OsString] {
        &self.arguments
    }
}

/// The Taker's derived Chat request id (taker-node `acceptance_files.rs`).
pub fn derived_request_id(reservation_id: &RequestId, label: &[u8]) -> RequestId {
    let mut digest = Sha256::new();
    digest.update("lez-atomic-swaps/zec-taker-chat-request/v1\0");
    digest.update(reservation_id.as_str().as_bytes());
    digest.update([0]);
    digest.update(label);
    RequestId::new(hex::encode(digest.finalize())).unwrap()
}

/// The one swap directory under a role's swaps root (not the stage directory,
/// not a decoy).
pub fn only_child(directory: &Path) -> PathBuf {
    let children: Vec<PathBuf> = fs::read_dir(directory)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.is_dir() && !path.file_name().unwrap().to_str().unwrap().contains('.'))
        .collect();
    assert_eq!(children.len(), 1, "{}: {children:?}", directory.display());
    children.into_iter().next().unwrap()
}

pub fn private_dir(path: &Path) -> PathBuf {
    fs::DirBuilder::new().mode(0o700).create(path).unwrap();
    fs::canonicalize(path).unwrap()
}

pub fn write_private(path: &Path, bytes: &[u8]) {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .unwrap();
    file.write_all(bytes).unwrap();
    file.sync_all().unwrap();
}
