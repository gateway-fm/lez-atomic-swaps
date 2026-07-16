mod support;

use std::sync::{Arc, Mutex};
use std::{fs, os::unix::fs::PermissionsExt as _};

use bitcoin::consensus::serialize;
use jsonrpsee_http_client::types::ErrorObjectOwned;
use jsonrpsee_server::{RpcModule, ServerBuilder};
use lez_btc_core_adapter::{BitcoinCoreRpc, HttpBitcoinCoreConfig, HttpBitcoinCoreRpc};
use serde_json::{Value, json};

use support::{REGTEST_GENESIS, raw_verbose, swap_fixture};

const TIP: &str = "6f8c2a4d807e31d3f650d7228af87f9e75bfac506bdf9c7730483cf1524e7ac4";

fn authenticated_config(endpoint: String) -> HttpBitcoinCoreConfig {
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("cookie");
    fs::write(&path, b"user:password").expect("cookie file");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).expect("owner-only cookie mode");
    HttpBitcoinCoreConfig::new(endpoint)
        .expect("literal loopback endpoint")
        .with_cookie_file(path)
        .expect("valid file-backed credential")
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn production_transport_uses_exact_core_31_methods_params_and_typed_responses() {
    let fixture = swap_fixture();
    let funding_txid = fixture.funding.compute_txid();
    let claim_txid = fixture.claim.compute_txid();
    let outpoint = fixture.agreement.cooperative_claim().funding_outpoint();
    let claim_hex = hex::encode(serialize(&fixture.claim));
    let verbose = serde_json::to_value(raw_verbose(&fixture.funding, Some(6), Some(TIP)))
        .expect("verbose JSON");
    let calls = Arc::new(Mutex::new(Vec::<String>::new()));
    let mut module = RpcModule::new(Arc::clone(&calls));

    module
        .register_method::<Result<Value, ErrorObjectOwned>, _>("getnetworkinfo", |_, calls, _| {
            calls.lock().expect("call log").push("network".to_owned());
            Ok(json!({
                "version": 310_100, "subversion": "/Satoshi:31.1.0/",
                "protocolversion": 70016, "localservices": "0000000000000409",
                "localservicesnames": [], "localrelay": true, "timeoffset": 0,
                "connections": 0, "connections_in": 0, "connections_out": 0,
                "networkactive": true, "networks": [], "relayfee": 0.00001,
                "incrementalfee": 0.00001, "localaddresses": [], "warnings": []
            }))
        })
        .expect("network method");
    module
        .register_method::<Result<Value, ErrorObjectOwned>, _>(
            "getblockchaininfo",
            |_, calls, _| {
                calls.lock().expect("call log").push("chain".to_owned());
                Ok(json!({
                    "chain": "regtest", "blocks": 200, "headers": 200,
                    "bestblockhash": TIP, "bits": "207fffff",
                    "target": "7fffff0000000000000000000000000000000000000000000000000000000000",
                    "difficulty": 4.656_542_373_906_925e-10, "time": 1_700_000_000,
                    "mediantime": 1_699_999_000, "verificationprogress": 1.0,
                    "initialblockdownload": false,
                    "chainwork": "0000000000000000000000000000000000000000000000000000000000000192",
                    "size_on_disk": 4096, "pruned": false, "warnings": []
                }))
            },
        )
        .expect("chain method");
    module
        .register_method::<Result<Value, ErrorObjectOwned>, _>(
            "getblockhash",
            |params, calls, _| {
                let height: u32 = params.one()?;
                calls
                    .lock()
                    .expect("call log")
                    .push(format!("blockhash:{height}"));
                Ok(json!(REGTEST_GENESIS))
            },
        )
        .expect("block hash method");
    module
        .register_method::<Result<Value, ErrorObjectOwned>, _>("getindexinfo", |_, calls, _| {
            calls.lock().expect("call log").push("indexes".to_owned());
            Ok(json!({
                "txindex": { "synced": true, "best_block_height": 200 },
                "txospenderindex": { "synced": true, "best_block_height": 200 }
            }))
        })
        .expect("index method");
    module
        .register_method::<Result<Value, ErrorObjectOwned>, _>(
            "getrawtransaction",
            move |params, calls, _| {
                let (transaction_id, verbose_flag): (String, bool) = params.parse()?;
                calls
                    .lock()
                    .expect("call log")
                    .push(format!("raw:{transaction_id}:{verbose_flag}"));
                Ok(verbose.clone())
            },
        )
        .expect("raw transaction method");
    module
        .register_method::<Result<Value, ErrorObjectOwned>, _>(
            "gettxspendingprevout",
            move |params, calls, _| {
                let (requested, options): (Vec<Value>, Value) = params.parse()?;
                assert_eq!(
                    requested,
                    [json!({ "txid": outpoint.txid, "vout": outpoint.vout })]
                );
                assert_eq!(
                    options,
                    json!({ "mempool_only": false, "return_spending_tx": true })
                );
                calls
                    .lock()
                    .expect("call log")
                    .push("spender:core31-options".to_owned());
                Ok(json!([{
                    "txid": outpoint.txid,
                    "vout": outpoint.vout,
                    "spendingtxid": claim_txid,
                    "spendingtx": claim_hex,
                    "blockhash": TIP
                }]))
            },
        )
        .expect("spender method");
    let mempool_claim = fixture.claim.clone();
    module
        .register_method::<Result<Value, ErrorObjectOwned>, _>(
            "testmempoolaccept",
            move |params, calls, _| {
                let transactions: Vec<String> = params.one()?;
                assert_eq!(transactions, [hex::encode(serialize(&mempool_claim))]);
                calls.lock().expect("call log").push("mempool".to_owned());
                Ok(json!([{
                    "txid": mempool_claim.compute_txid(),
                    "wtxid": mempool_claim.compute_wtxid(),
                    "allowed": true,
                    "vsize": mempool_claim.vsize(),
                    "fees": { "base": 0.00001 }
                }]))
            },
        )
        .expect("mempool method");
    let sent_claim = fixture.claim.clone();
    module
        .register_method::<Result<Value, ErrorObjectOwned>, _>(
            "sendrawtransaction",
            move |params, calls, _| {
                let transaction: String = params.one()?;
                assert_eq!(transaction, hex::encode(serialize(&sent_claim)));
                calls.lock().expect("call log").push("send".to_owned());
                Ok(json!(sent_claim.compute_txid()))
            },
        )
        .expect("send method");

    let server = ServerBuilder::default()
        .build("127.0.0.1:0")
        .await
        .expect("isolated loopback server");
    let address = server.local_addr().expect("server address");
    let handle = server.start(module);
    let rpc = HttpBitcoinCoreRpc::connect(&authenticated_config(format!("http://{address}")))
        .expect("bounded client");

    assert_eq!(
        rpc.get_network_info().await.expect("network").version,
        310_100
    );
    assert_eq!(rpc.get_blockchain_info().await.expect("chain").blocks, 200);
    assert_eq!(
        rpc.get_genesis_hash().await.expect("genesis").0,
        REGTEST_GENESIS
    );
    assert_eq!(rpc.get_index_info().await.expect("indexes").0.len(), 2);
    assert_eq!(
        rpc.get_raw_transaction(funding_txid)
            .await
            .expect("raw transaction")
            .expect("present")
            .txid,
        funding_txid.to_string()
    );
    assert_eq!(
        rpc.get_tx_spending_prevout(outpoint)
            .await
            .expect("spender")
            .0[0]
            .spending_txid
            .as_deref(),
        Some(claim_txid.to_string().as_str())
    );
    assert!(
        rpc.test_mempool_accept(&serialize(&fixture.claim))
            .await
            .expect("mempool")
            .0[0]
            .allowed
    );
    assert_eq!(
        rpc.send_raw_transaction(&serialize(&fixture.claim))
            .await
            .expect("send")
            .0,
        claim_txid.to_string()
    );
    assert_eq!(
        *calls.lock().expect("call log"),
        [
            "network".to_owned(),
            "chain".to_owned(),
            "blockhash:0".to_owned(),
            "indexes".to_owned(),
            format!("raw:{funding_txid}:true"),
            "spender:core31-options".to_owned(),
            "mempool".to_owned(),
            "send".to_owned()
        ]
    );
    handle.stop().expect("stop server");
    handle.stopped().await;
}

#[tokio::test]
async fn only_call_error_minus_five_is_transaction_absence() {
    let mut module = RpcModule::new(());
    module
        .register_method::<Result<Value, ErrorObjectOwned>, _>("getrawtransaction", |_, (), _| {
            Err(ErrorObjectOwned::owned(-5, "not found", None::<()>))
        })
        .expect("raw transaction method");
    let server = ServerBuilder::default()
        .build("127.0.0.1:0")
        .await
        .expect("isolated loopback server");
    let address = server.local_addr().expect("server address");
    let handle = server.start(module);
    let rpc = HttpBitcoinCoreRpc::connect(&authenticated_config(format!("http://{address}")))
        .expect("bounded client");
    assert_eq!(
        rpc.get_raw_transaction(swap_fixture().funding.compute_txid())
            .await
            .expect("only -5 maps to absence"),
        None
    );
    handle.stop().expect("stop server");
    handle.stopped().await;
}
