//! Every RPC the adapter makes, against a real public provider.
//!
//! Ignored by default: it talks to a third party (`PublicNode`, keyless, Bitcoin
//! Core 29.3 on testnet3 when this was written), which a build must not depend
//! on. It exists because typed decoding of an older Core's answers, and which
//! methods a provider admits at all, can only be learned from one:
//!
//! ```sh
//! cargo test -p lez-btc-core-adapter --test public_provider_live -- --ignored --nocapture
//! ```

use std::os::unix::fs::PermissionsExt as _;

use bitcoin::consensus::deserialize;
use bitcoin::{BlockHash, OutPoint, Transaction};
use lez_btc_core_adapter::{
    BitcoinCoreRpc, CoreConnectivityPolicy, HttpBitcoinCoreConfig, HttpBitcoinCoreRpc,
    MINIMUM_BITCOIN_CORE_VERSION,
};

const ENDPOINT: &str = "https://bitcoin-testnet-rpc.publicnode.com/";

#[tokio::test]
#[ignore = "talks to a third-party RPC provider"]
async fn a_public_provider_answers_every_call_the_adapter_makes() {
    let directory = tempfile::tempdir().expect("temporary directory");
    // The provider is keyless; the transport still wants file-backed credentials.
    let credentials = directory.path().join("basic");
    std::fs::write(&credentials, b"anonymous:anonymous").expect("credentials");
    std::fs::set_permissions(&credentials, std::fs::Permissions::from_mode(0o600)).expect("mode");
    let config =
        HttpBitcoinCoreConfig::new_exact_https_basic_gateway(ENDPOINT, ENDPOINT, &credentials)
            .expect("exact HTTPS gateway");
    // The route is one the testnet3 policy admits; the raw transport below is
    // what that adapter would drive.
    HttpBitcoinCoreRpc::connect_profiled(&config, CoreConnectivityPolicy::Testnet3Networked)
        .expect("testnet3 over HTTPS");
    let rpc = HttpBitcoinCoreRpc::connect(&config).expect("bounded client");

    let network = rpc.get_network_info().await.expect("getnetworkinfo");
    assert!(network.version >= MINIMUM_BITCOIN_CORE_VERSION);
    let chain = rpc.get_blockchain_info().await.expect("getblockchaininfo");
    assert_eq!(chain.chain, "test");
    let indexes = rpc.get_index_info().await.expect("getindexinfo");
    assert!(indexes.0.contains_key("txindex"));
    assert!(!indexes.0.contains_key("txospenderindex"));
    rpc.get_genesis_hash().await.expect("getblockhash 0");
    println!("{} {} at {}", network.subversion, chain.chain, chain.blocks);

    let tip: BlockHash = chain.best_block_hash.parse().expect("tip hash");
    rpc.get_block_header(tip).await.expect("getblockheader");
    let height = u32::try_from(chain.blocks).expect("height");
    let (hash, transactions) = rpc
        .get_block_transactions(height)
        .await
        .expect("getblock 2");
    assert_eq!(hash, tip);
    let transactions: Vec<Transaction> = transactions
        .iter()
        .map(|bytes| deserialize(bytes).expect("consensus transaction"))
        .collect();

    // An input of a confirmed transaction is, by construction, spent in a block.
    if let Some(spender) = transactions
        .iter()
        .find(|transaction| !transaction.is_coinbase())
    {
        let spent = spender.input[0].previous_output;
        assert!(
            !rpc.is_unspent(spent)
                .await
                .expect("gettxout of a spent output")
        );
        assert_eq!(
            rpc.get_mempool_spender(spent)
                .await
                .expect("mempool spender"),
            None
        );
        let fetched = rpc
            .get_raw_transaction(spender.compute_txid())
            .await
            .expect("getrawtransaction")
            .expect("txindex finds a confirmed transaction");
        assert_eq!(fetched.confirmations, Some(1));
    }
    // The newest coinbase output cannot have been spent yet.
    let coinbase = OutPoint::new(transactions[0].compute_txid(), 0);
    assert!(
        rpc.is_unspent(coinbase)
            .await
            .expect("gettxout of an unspent output")
    );
}
