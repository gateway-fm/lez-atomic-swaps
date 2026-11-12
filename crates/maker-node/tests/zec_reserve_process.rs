//! Exercises the take-time Zcash agreement; compiled only with `pair-zec`.
#![cfg(feature = "pair-zec")]

//! A real Maker daemon and the real Taker service reach one countersigned
//! Zcash agreement through `zec_reserve_v1` → `zec_chat_propose_v1` →
//! `zec_chat_complete_v1` with no corridor provisioned on either side: no
//! actor template, no prepared catalog, no file copied between the roles.
//! Zebra and each swap's LEZ sidecar are fakes that serve only what the round
//! reads (the chain's tip, one mature funder output, a finalized LEZ height).

use std::{
    collections::HashMap,
    fs::{self, OpenOptions},
    io::Write as _,
    net::SocketAddr,
    os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _},
    path::{Path, PathBuf},
    process::{Child, Command},
    sync::Arc,
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use jsonrpsee::{RpcModule, server::ServerBuilder, types::ErrorObjectOwned};
use lez_bridge_protocol::{
    ChainClock, Hex32, METHOD_OBSERVE_FINALIZED_CLOCK, ObserveFinalizedClockRequest,
    ObserveFinalizedClockResult, RequestId,
};
use lez_maker_node::{
    DeliveryOfferQueryV1, LocalPriceSetRequest, PairConfigureRequest, RunLocalDelivery,
    TakerMakerIdentityV1, ZecChatCompleteRequestV1, ZecChatCompleteResponseV1, call_local_rpc,
};
use lez_swap_core::{Pair, Participant, SwapDirection, UnixSeconds};
use lez_swap_sdk_core::OfferDiscovery as _;
use lez_swap_store::{
    LocalPriceV1, MakerOfferId, MakerPairConfigurationV1, MakerPriceSourceKind, MakerRouteV1,
    SqliteTakerFacadeStore,
};
use lez_taker_node::{
    TakerInitiationCommitV1, TakerSwapInitiateRequestV1, load_taker_service_context,
    taker_service_rpc_module,
};
use lez_zec_swap_sdk::AcceptedZecAgreementV1;
use rustix::process::{Pid, Signal, kill_process};
use secp256k1::{PublicKey, Secp256k1, SecretKey};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use tempfile::tempdir;
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
const FUNDER_KEY: u8 = 4;
const OTHER_KEY: u8 = 2;
const ZEBRA_TIP: u32 = 120;
/// The principal must exceed the agreement's 10,000-zatoshi claim fee.
const FOREIGN_UNITS: u64 = 1_000_000;
const LEZ_UNITS: u128 = 2_500_000;

#[tokio::test(flavor = "multi_thread")]
async fn a_taker_sells_lez_swap_is_agreed_at_take_time_without_a_corridor() {
    agree_at_take_time(SwapDirection::TakerSellsLez).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_taker_sells_zcash_swap_is_agreed_at_take_time_without_a_corridor() {
    agree_at_take_time(SwapDirection::TakerSellsForeign).await;
}

#[allow(clippy::too_many_lines)]
async fn agree_at_take_time(direction: SwapDirection) {
    let run = tempdir().unwrap();
    let root = fs::canonicalize(run.path()).unwrap();
    let (maker_byte, taker_byte) = match direction {
        SwapDirection::TakerSellsLez => (FUNDER_KEY, OTHER_KEY),
        SwapDirection::TakerSellsForeign => (OTHER_KEY, FUNDER_KEY),
    };
    let zebra = start_fake_zebra().await;
    let sidecar_program = fake_sidecar_program(&root);
    let port_base = 20_000
        + u16::try_from(std::process::id() % 400).unwrap() * 50
        + if direction == SwapDirection::TakerSellsLez {
            0
        } else {
            20
        };

    // The Maker: its Delivery key is its Zcash key; its LEZ role is the
    // Node-owned lifecycle's; a Zcash role config names its Zebra.
    let maker_home = private_dir(&root.join("maker"));
    let runtime = private_dir(&maker_home.join("run"));
    let socket = runtime.join("maker.sock");
    let chat_socket = runtime.join("chat.sock");
    let ready = runtime.join("ready");
    let delivery = maker_home.join("delivery");
    let delivery_key = maker_home.join("delivery-signing.key");
    write_private(&delivery_key, &[maker_byte; 32]);
    let btc_key = maker_home.join("btc-chat-signing.key");
    write_private(&btc_key, &[0x31; 32]);
    let claim_key = maker_home.join("zec-claim-recovery.key");
    write_private(&claim_key, &[0x7a; 32]);
    let actor_root = private_dir(&maker_home.join("actors"));
    let actor_program = fs::canonicalize("/usr/bin/true").unwrap();
    let actor_sha = hex::encode(Sha256::digest(fs::read(&actor_program).unwrap()));
    let maker_btc = btc_role_config(&maker_home, &sidecar_program, port_base, 0x77);
    let maker_zec = zec_role_config(&maker_home, &zebra, None);
    let mut daemon = Daemon(
        Command::new(env!("CARGO_BIN_EXE_lez-maker-node"))
            .args([
                "--socket",
                path(&socket),
                "--chat-socket",
                path(&chat_socket),
            ])
            .args(["--database", path(&maker_home.join("maker.sqlite3"))])
            .args(["--ready-file", path(&ready)])
            .args(["--delivery-directory", path(&delivery)])
            .args(["--delivery-signing-key-file", path(&delivery_key)])
            .args(["--btc-maker-signing-key-file", path(&btc_key)])
            .args(["--btc-role-config", path(&maker_btc)])
            .args(["--maker-claim-key-id", "zec-reserve-claim-v1"])
            .args(["--maker-claim-key-file", path(&claim_key)])
            .args(["--zec-role-config", path(&maker_zec)])
            .args(["--zec-maker-actor-root", path(&actor_root)])
            .args(["--zec-actor-program", path(&actor_program)])
            .args(["--zec-actor-program-sha256", &actor_sha])
            .spawn()
            .expect("start the Maker daemon"),
    );
    wait_ready(&mut daemon, &ready);
    let route = MakerRouteV1::new(Pair::Zcash, direction).unwrap();
    configure_live_route(&socket, route).await;
    let offer_id = MakerOfferId::new("zec-reserve-offer-001").unwrap();
    let published = Command::new(env!("CARGO_BIN_EXE_lez-maker-cli"))
        .args(["--socket", path(&socket), "publish-offer"])
        .args(["--request-id", "zec-reserve-publish-001"])
        .args(["--offer-id", offer_id.as_str(), "--pair", "zcash"])
        .args([
            "--direction",
            match direction {
                SwapDirection::TakerSellsForeign => "taker-sells-foreign",
                SwapDirection::TakerSellsLez => "taker-sells-lez",
            },
        ])
        .output()
        .unwrap();
    assert!(
        published.status.success(),
        "publish-offer: {}",
        String::from_utf8_lossy(&published.stderr)
    );

    // The Taker: its own LEZ role and Zcash key, no prepared catalog.
    let taker_home = private_dir(&root.join("taker"));
    let taker_zcash_key = taker_home.join("zcash.key");
    write_private(&taker_zcash_key, &[taker_byte; 32]);
    let taker_btc = btc_role_config(&taker_home, &sidecar_program, port_base + 10, 0x78);
    let taker_zec = zec_role_config(&taker_home, &zebra, Some(&taker_zcash_key));
    let registry = taker_home.join("registry.sqlite3");
    drop(SqliteTakerFacadeStore::create_new(&registry).unwrap());
    let maker_key = PublicKey::from_secret_key(
        &Secp256k1::signing_only(),
        &SecretKey::from_slice(&[maker_byte; 32]).unwrap(),
    );
    let service_config = taker_home.join("role.json");
    write_private(
        &service_config,
        &serde_json::to_vec(&json!({
            "schema_version": 1,
            "delivery_sources": [{
                "source_id": "local-maker",
                "directory": delivery,
                "maker_public_key": hex::encode(maker_key.serialize()),
            }],
            "chat_socket": chat_socket,
            "maximum_offers": 16,
            "initiation": {
                "execute_prepared": true,
                "registry_database": registry,
                "btc_role_config": taker_btc,
                "zec_role_config": taker_zec,
            }
        }))
        .unwrap(),
    );
    let authenticated = RunLocalDelivery::subscriber(&delivery, maker_key)
        .unwrap()
        .discover(&DeliveryOfferQueryV1::for_route(route, now()))
        .await
        .unwrap()
        .into_iter()
        .next()
        .expect("the published offer is discoverable");
    let request = TakerSwapInitiateRequestV1 {
        schema_version: 1,
        request_id: RequestId::new("zec-reserve-initiation-001").unwrap(),
        offer_id: offer_id.clone(),
        route,
        maker_identity: TakerMakerIdentityV1::new(maker_key.serialize()).unwrap(),
        signed_envelope_sha256: authenticated.commitment(),
        foreign_units: FOREIGN_UNITS,
        expected_lez_units: LEZ_UNITS,
        logos_offer_announcement_base64: None,
        funding_transaction_hex: None,
    };
    let module =
        taker_service_rpc_module(load_taker_service_context(&service_config).unwrap()).unwrap();
    let commit: TakerInitiationCommitV1 = module
        .call("taker_swap_initiate_v1", [request.clone()])
        .await
        .expect("the take reserves, proposes and completes over Chat");
    assert!(!commit.was_replay);
    let swap_id = commit.swap.swap_id.clone();
    assert!(swap_id.as_str().starts_with("zec-"));

    // Both roles hold the same countersigned agreement, each its own half.
    let taker_swap = only_child(&taker_home.join("swaps"));
    let maker_swap = only_child(&maker_home.join("swaps"));
    let final_wire = fs::read(taker_swap.join("zec-agreement.borsh")).unwrap();
    let accepted = AcceptedZecAgreementV1::accept_wire_at(
        &final_wire,
        UnixSeconds::new(now()),
        Participant::Taker,
        0,
    )
    .unwrap();
    assert_eq!(accepted.agreement().application_swap_id(), swap_id.as_str());
    let maker_bundle = only_child(&actor_root);
    assert_eq!(
        fs::read(maker_bundle.join("shared/agreement-v2.borsh")).unwrap(),
        final_wire,
        "the Maker provisioned its actor from the same agreement"
    );
    let maker_funds = direction == SwapDirection::TakerSellsLez;
    assert_eq!(
        maker_swap.join("zec/claim-preimage.key").is_file(),
        maker_funds
    );
    assert_eq!(
        taker_swap.join("zec/claim-preimage.key").is_file(),
        !maker_funds
    );
    for side in [&maker_swap, &taker_swap] {
        assert!(side.join("zec/actor-config.json").is_file());
        assert!(side.join("sidecar.json").is_file());
    }
    assert!(taker_swap.join("zec-receipt.json").is_file());

    // Restart safety. The Taker reloads its take-time entry and answers the
    // same initiation as a replay; the Maker re-registers its template, so a
    // replayed completion still finds the swap's own claim secret.
    drop(module);
    let module =
        taker_service_rpc_module(load_taker_service_context(&service_config).unwrap()).unwrap();
    let replay: TakerInitiationCommitV1 = module
        .call("taker_swap_initiate_v1", [request.clone()])
        .await
        .expect("a restarted Taker replays the take from its persisted entry");
    assert!(replay.was_replay);
    assert_eq!(replay.swap.swap_id, swap_id);
    stop(&mut daemon);
    let _ = fs::remove_file(&ready);
    let mut daemon = Daemon(
        Command::new(env!("CARGO_BIN_EXE_lez-maker-node"))
            .args([
                "--socket",
                path(&socket),
                "--chat-socket",
                path(&chat_socket),
            ])
            .args(["--database", path(&maker_home.join("maker.sqlite3"))])
            .args(["--ready-file", path(&ready)])
            .args(["--delivery-directory", path(&delivery)])
            .args(["--delivery-signing-key-file", path(&delivery_key)])
            .args(["--btc-maker-signing-key-file", path(&btc_key)])
            .args(["--btc-role-config", path(&maker_btc)])
            .args(["--maker-claim-key-id", "zec-reserve-claim-v1"])
            .args(["--maker-claim-key-file", path(&claim_key)])
            .args(["--zec-role-config", path(&maker_zec)])
            .args(["--zec-maker-actor-root", path(&actor_root)])
            .args(["--zec-actor-program", path(&actor_program)])
            .args(["--zec-actor-program-sha256", &actor_sha])
            .spawn()
            .expect("restart the Maker daemon"),
    );
    wait_ready(&mut daemon, &ready);
    let reservation_id =
        RequestId::new(maker_swap.file_name().unwrap().to_str().unwrap().to_owned()).unwrap();
    let completion: ZecChatCompleteResponseV1 = call_local_rpc(
        &chat_socket,
        "zec_chat_complete_v1",
        &ZecChatCompleteRequestV1 {
            schema_version: 1,
            request_id: derived_request_id(&reservation_id, b"complete"),
            offer_id: offer_id.clone(),
            expected_offer_revision: 2,
            reservation_id,
            final_agreement_wire: final_wire,
        },
    )
    .await
    .expect("a restarted Maker replays the completion");
    assert!(completion.was_replay);
    assert_eq!(completion.swap_id.as_ref(), swap_id.as_str());
    stop(&mut daemon);
}

/// The Taker's derived Chat request id (taker-node `acceptance_files.rs`).
fn derived_request_id(reservation_id: &RequestId, label: &[u8]) -> RequestId {
    let mut digest = Sha256::new();
    digest.update("lez-atomic-swaps/zec-taker-chat-request/v1\0");
    digest.update(reservation_id.as_str().as_bytes());
    digest.update([0]);
    digest.update(label);
    RequestId::new(hex::encode(digest.finalize())).unwrap()
}

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
async fn start_fake_zebra() -> String {
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

/// The swap sidecar the Nodes spawn (btc-role-lifecycle sidecar.rs): this
/// test binary again, as `fake_sidecar_process`, until the run is removed.
fn fake_sidecar_program(root: &Path) -> PathBuf {
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

/// Serves one swap's finalized LEZ clock on the address the Node chose.
#[test]
#[ignore = "the fake LEZ sidecar the take-time tests spawn as a process"]
fn fake_sidecar_process() {
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

fn btc_role_config(home: &Path, sidecar: &Path, port_base: u16, signer: u8) -> PathBuf {
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
                "sequencer_url": "http://127.0.0.1:3040",
                "indexer_url": "http://127.0.0.1:8779",
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

fn zec_role_config(home: &Path, zebra: &str, zcash_key: Option<&Path>) -> PathBuf {
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

async fn configure_live_route(socket: &Path, route: MakerRouteV1) {
    let disabled = MakerPairConfigurationV1::new(
        route,
        false,
        MakerPriceSourceKind::Local,
        10,
        FOREIGN_UNITS,
        3,
    )
    .unwrap();
    let _: Value = call_local_rpc(
        socket,
        "maker_pair_configure",
        &PairConfigureRequest {
            request_id: RequestId::new("zec-reserve-route-create-001").unwrap(),
            expected_revision: None,
            configuration: disabled,
        },
    )
    .await
    .unwrap();
    let _: Value = call_local_rpc(
        socket,
        "maker_local_price_set",
        &LocalPriceSetRequest {
            request_id: RequestId::new("zec-reserve-price-create-001").unwrap(),
            expected_revision: None,
            price: LocalPriceV1::new(route, 5, 2).unwrap(),
        },
    )
    .await
    .unwrap();
    let enabled = MakerPairConfigurationV1::new(
        route,
        true,
        MakerPriceSourceKind::Local,
        10,
        FOREIGN_UNITS,
        3,
    )
    .unwrap();
    let _: Value = call_local_rpc(
        socket,
        "maker_pair_configure",
        &PairConfigureRequest {
            request_id: RequestId::new("zec-reserve-route-enable-001").unwrap(),
            expected_revision: Some(1),
            configuration: enabled,
        },
    )
    .await
    .unwrap();
}

fn only_child(directory: &Path) -> PathBuf {
    let children: Vec<PathBuf> = fs::read_dir(directory)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.is_dir())
        .collect();
    assert_eq!(children.len(), 1, "{}: {children:?}", directory.display());
    children.into_iter().next().unwrap()
}

/// The Maker daemon, stopped when the test ends however it ends.
struct Daemon(Child);

impl Drop for Daemon {
    fn drop(&mut self) {
        if matches!(self.0.try_wait(), Ok(None)) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

fn wait_ready(daemon: &mut Daemon, ready: &Path) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !ready.exists() {
        assert!(
            daemon.0.try_wait().unwrap().is_none(),
            "the Maker exited early"
        );
        assert!(Instant::now() < deadline, "the Maker did not become ready");
        thread::sleep(Duration::from_millis(20));
    }
}

fn stop(daemon: &mut Daemon) {
    kill_process(Pid::from_child(&daemon.0), Signal::INT).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while daemon.0.try_wait().unwrap().is_none() {
        assert!(Instant::now() < deadline, "the Maker did not stop");
        thread::sleep(Duration::from_millis(20));
    }
}

fn private_dir(path: &Path) -> PathBuf {
    fs::DirBuilder::new().mode(0o700).create(path).unwrap();
    fs::canonicalize(path).unwrap()
}

fn write_private(path: &Path, bytes: &[u8]) {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .unwrap();
    file.write_all(bytes).unwrap();
    file.sync_all().unwrap();
}

fn path(value: &Path) -> &str {
    value.to_str().unwrap()
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}
