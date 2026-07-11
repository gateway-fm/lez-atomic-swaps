//! Actor-keyed consensus acceptance against the pinned isolated Zebra node.

use std::{fmt::Write as _, time::Duration};

use jsonrpsee::{core::client::ClientT, rpc_params};
use jsonrpsee_http_client::{HttpClient, HttpClientBuilder};
use lez_zec_swap_sdk::{
    Bip199Contract, TransparentFundingRequest, TransparentSpendRequest, TransparentUtxo,
    build_claim_transaction, build_funding_transaction, build_refund_transaction,
};
use secp256k1::{PublicKey, Secp256k1, SecretKey};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use zcash_primitives::transaction::Transaction;
use zcash_protocol::{
    consensus::{BlockHeight, BranchId},
    value::Zatoshis,
};
use zcash_transparent::{
    address::TransparentAddress,
    bundle::{OutPoint, TxOut},
};

const MINER_ADDRESS: &str = "tmNAP26Sw5Ra2jepAoTr1kqdkggawba6Akd";
const PREIMAGE: [u8; 32] = [0x44; 32];
const SECRET_DIGEST: [u8; 32] = [
    0xbb, 0x39, 0x14, 0x15, 0xc0, 0x5e, 0x39, 0xd7, 0x7c, 0xa1, 0x73, 0x81, 0xd3, 0xbe, 0x3f, 0x7d,
    0x0c, 0xd5, 0xe5, 0x33, 0x2e, 0x5a, 0x57, 0x93, 0x11, 0xad, 0xaa, 0x0a, 0xa6, 0x21, 0x06, 0xe9,
];

fn key(byte: u8) -> SecretKey {
    SecretKey::from_slice(&[byte; 32]).expect("fixed test key is valid")
}

fn public_key(secret_key: &SecretKey) -> PublicKey {
    PublicKey::from_secret_key(&Secp256k1::new(), secret_key)
}

fn pubkey_hash(secret_key: &SecretKey) -> [u8; 20] {
    match TransparentAddress::from_pubkey(&public_key(secret_key)) {
        TransparentAddress::PublicKeyHash(hash) => hash,
        TransparentAddress::ScriptHash(_) => unreachable!("public keys always yield P2PKH"),
    }
}

fn zatoshis(value: u64) -> Zatoshis {
    Zatoshis::from_u64(value).expect("test amount is in range")
}

fn serialize(transaction: &Transaction) -> Vec<u8> {
    let mut bytes = vec![];
    transaction
        .write(&mut bytes)
        .expect("serializing a frozen transaction succeeds");
    bytes
}

fn tx_hex(transaction: &Transaction) -> String {
    hex::encode(serialize(transaction))
}

fn mutate_first_signature(transaction: &Transaction) -> String {
    let script_sig = &transaction
        .transparent_bundle()
        .expect("test transactions are transparent")
        .vin[0]
        .script_sig()
        .0
        .0;
    let mut bytes = serialize(transaction);
    let script_offset = bytes
        .windows(script_sig.len())
        .position(|window| window == script_sig)
        .expect("serialized transaction contains its scriptSig");
    bytes[script_offset + 10] ^= 1;
    hex::encode(bytes)
}

fn client() -> HttpClient {
    let endpoint = std::env::var("ZEBRA_RPC_URL")
        .expect("ZEBRA_RPC_URL is supplied by scripts/run-zebra-e2e.sh");
    HttpClientBuilder::default()
        .request_timeout(Duration::from_mins(2))
        .build(endpoint)
        .expect("isolated Zebra RPC URL is valid")
}

async fn block_count(client: &HttpClient) -> u32 {
    client
        .request("getblockcount", rpc_params![])
        .await
        .expect("getblockcount succeeds")
}

async fn generate_to(client: &HttpClient, target: u32) {
    loop {
        let current = block_count(client).await;
        if current >= target {
            break;
        }
        let batch = (target - current).min(10);
        let _: Vec<String> = client
            .request("generate", rpc_params![batch])
            .await
            .expect("Regtest block generation succeeds");
    }
}

async fn actor_utxos(client: &HttpClient) -> Vec<Value> {
    let response: Value = client
        .request(
            "getaddressutxos",
            rpc_params![json!({"addresses": [MINER_ADDRESS], "chaininfo": true})],
        )
        .await
        .expect("Zebra returns the actor's transparent UTXOs");
    response
        .as_array()
        .expect("getaddressutxos returns an array")
        .clone()
}

async fn fetched_utxo(client: &HttpClient, entry: &Value) -> TransparentUtxo {
    let txid = entry["txid"]
        .as_str()
        .expect("UTXO contains a transaction id");
    let index = u32::try_from(
        entry["outputIndex"]
            .as_u64()
            .expect("UTXO contains an output index"),
    )
    .expect("Zcash output index fits u32");
    let raw: String = client
        .request("getrawtransaction", rpc_params![txid, 0])
        .await
        .expect("coinbase transaction is available by id");
    let bytes = hex::decode(raw).expect("Zebra returns transaction hex");
    let transaction = Transaction::read(bytes.as_slice(), BranchId::Nu6_2)
        .expect("NU6.2 coinbase transaction decodes canonically");
    assert_eq!(transaction.txid().to_string(), txid);
    let output = transaction
        .transparent_bundle()
        .expect("coinbase pays a transparent miner output")
        .vout[usize::try_from(index).expect("u32 fits usize")]
    .clone();

    TransparentUtxo::new(OutPoint::new(*transaction.txid().as_ref(), index), output)
}

async fn assert_rejected(client: &HttpClient, transaction_hex: String, reason: &str) {
    let result = client
        .request::<String, _>("sendrawtransaction", rpc_params![transaction_hex])
        .await;
    assert!(result.is_err(), "Zebra accepted {reason}");
}

async fn broadcast(client: &HttpClient, transaction: &Transaction) -> String {
    let rpc_txid: String = client
        .request("sendrawtransaction", rpc_params![tx_hex(transaction)])
        .await
        .expect("Zebra accepts the consensus-valid transaction");
    assert_eq!(rpc_txid, transaction.txid().to_string());
    rpc_txid
}

async fn assert_confirmed(client: &HttpClient, txid: &str, transaction: &Transaction) {
    let observed: Value = client
        .request("getrawtransaction", rpc_params![txid, 1])
        .await
        .expect("confirmed transaction remains observable");
    assert_eq!(observed["hex"], tx_hex(transaction));
    assert!(
        observed["confirmations"]
            .as_u64()
            .expect("verbose transaction has confirmations")
            >= 1
    );
}

async fn raw_mempool(client: &HttpClient) -> Vec<String> {
    client
        .request("getrawmempool", rpc_params![false])
        .await
        .expect("Zebra returns its raw mempool transaction ids")
}

async fn wait_for_mempool(client: &HttpClient, expected_txids: &[&str]) {
    for _ in 0..50 {
        let observed = raw_mempool(client).await;
        if expected_txids
            .iter()
            .all(|expected| observed.iter().any(|txid| txid == expected))
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("actor transactions did not enter Zebra's mempool");
}

async fn rebroadcast_or_observe(client: &HttpClient, transaction: &Transaction) {
    let expected_txid = transaction.txid().to_string();
    let result = client
        .request::<String, _>("sendrawtransaction", rpc_params![tx_hex(transaction)])
        .await;
    if let Ok(observed_txid) = result {
        assert_eq!(observed_txid, expected_txid);
    }
    wait_for_mempool(client, &[&expected_txid]).await;
}

fn funding_transaction(
    contract: &Bip199Contract,
    candidate: TransparentUtxo,
    actor_key: &SecretKey,
    tip: u32,
) -> Transaction {
    let request = TransparentFundingRequest::new(
        vec![candidate],
        public_key(actor_key),
        zatoshis(100_000_000),
        zatoshis(10_000),
        zatoshis(10_000),
        BlockHeight::from_u32(tip + 40),
        BranchId::Nu6_2,
    )
    .expect("actor owns a mature funding output");
    build_funding_transaction(contract, &request, actor_key)
        .expect("canonical funding transaction builds")
}

fn contract_output(transaction: &Transaction) -> TxOut {
    transaction
        .transparent_bundle()
        .expect("funding transaction is transparent")
        .vout[0]
        .clone()
}

fn hex_summary(transaction: &Transaction) -> String {
    let mut summary = String::new();
    write!(
        &mut summary,
        "{}:{}",
        transaction.txid(),
        tx_hex(transaction).len()
    )
    .expect("writing to String succeeds");
    summary
}

async fn prove_tip_reorg_and_actor_rebroadcast(
    client: &HttpClient,
    claim: &Transaction,
    refund: &Transaction,
    claim_contract: &Bip199Contract,
    claim_request: &TransparentSpendRequest,
    funder_key: &SecretKey,
) {
    let claim_txid = claim.txid().to_string();
    let refund_txid = refund.txid().to_string();
    let terminal_height = block_count(client).await;
    let terminal_hash: String = client
        .request("getblockhash", rpc_params![terminal_height])
        .await
        .expect("Zebra returns the concurrent terminal block hash");
    let _: () = client
        .request("invalidateblock", rpc_params![&terminal_hash])
        .await
        .expect("Zebra invalidates the non-finalized terminal block");
    assert_eq!(block_count(client).await, terminal_height - 1);
    tokio::join!(
        rebroadcast_or_observe(client, claim),
        rebroadcast_or_observe(client, refund)
    );
    wait_for_mempool(client, &[&claim_txid, &refund_txid]).await;

    let conflicting_refund = build_refund_transaction(claim_contract, claim_request, funder_key)
        .expect("alternative refund for the claimed contract is constructible after timeout");
    assert_rejected(
        client,
        tx_hex(&conflicting_refund),
        "same-output replacement while the actor claim is in the mempool",
    )
    .await;

    let reconsidered: Value = client
        .request("reconsiderblock", rpc_params![&terminal_hash])
        .await
        .expect("Zebra reconsiders the exact invalidated block");
    assert!(
        !reconsidered
            .as_array()
            .expect("reconsiderblock returns a list of hash byte sequences")
            .is_empty()
    );
    assert_eq!(block_count(client).await, terminal_height);
    let restored_hash: String = client
        .request("getblockhash", rpc_params![terminal_height])
        .await
        .expect("Zebra returns the restored terminal block hash");
    assert_eq!(restored_hash, terminal_hash);
    assert_confirmed(client, &claim_txid, claim).await;
    assert_confirmed(client, &refund_txid, refund).await;
}

async fn prove_concurrent_claim_refund_and_tip_reorg(
    client: &HttpClient,
    funder_key: &SecretKey,
    claim_candidate: TransparentUtxo,
    refund_candidate: TransparentUtxo,
) {
    const CONCURRENT_CLAIM_PREIMAGE: [u8; 32] = [0x55; 32];
    const CONCURRENT_REFUND_PREIMAGE: [u8; 32] = [0x66; 32];

    let claim_key = key(6);
    let refund_claimant_key = key(7);
    let refund_at = block_count(client).await + 4;
    let claim_contract = Bip199Contract::new(
        refund_at,
        pubkey_hash(funder_key),
        Sha256::digest(CONCURRENT_CLAIM_PREIMAGE).into(),
        pubkey_hash(&claim_key),
    );
    let refund_contract = Bip199Contract::new(
        refund_at,
        pubkey_hash(funder_key),
        Sha256::digest(CONCURRENT_REFUND_PREIMAGE).into(),
        pubkey_hash(&refund_claimant_key),
    );
    let tip = block_count(client).await;
    let claim_funding = funding_transaction(&claim_contract, claim_candidate, funder_key, tip);
    let refund_funding = funding_transaction(&refund_contract, refund_candidate, funder_key, tip);

    let (claim_funding_txid, refund_funding_txid) = tokio::join!(
        broadcast(client, &claim_funding),
        broadcast(client, &refund_funding)
    );
    generate_to(client, block_count(client).await + 1).await;
    assert_confirmed(client, &claim_funding_txid, &claim_funding).await;
    assert_confirmed(client, &refund_funding_txid, &refund_funding).await;

    let claim_request = TransparentSpendRequest::new(
        &claim_contract,
        OutPoint::new(*claim_funding.txid().as_ref(), 0),
        contract_output(&claim_funding),
        TransparentAddress::from_pubkey(&public_key(&key(8))),
        zatoshis(10_000),
        BlockHeight::from_u32(refund_at + 40),
        BranchId::Nu6_2,
    )
    .expect("concurrent claim output is spendable");
    let claim = build_claim_transaction(
        &claim_contract,
        &claim_request,
        &claim_key,
        &CONCURRENT_CLAIM_PREIMAGE,
    )
    .expect("concurrent claimant constructs its spend");
    let refund_request = TransparentSpendRequest::new(
        &refund_contract,
        OutPoint::new(*refund_funding.txid().as_ref(), 0),
        contract_output(&refund_funding),
        TransparentAddress::from_pubkey(&public_key(&key(9))),
        zatoshis(10_000),
        BlockHeight::from_u32(refund_at + 40),
        BranchId::Nu6_2,
    )
    .expect("concurrent refund output is spendable");
    let refund = build_refund_transaction(&refund_contract, &refund_request, funder_key)
        .expect("concurrent funder constructs its refund");

    generate_to(client, refund_at).await;
    let (claim_txid, refund_txid) =
        tokio::join!(broadcast(client, &claim), broadcast(client, &refund));
    generate_to(client, block_count(client).await + 1).await;
    assert_confirmed(client, &claim_txid, &claim).await;
    assert_confirmed(client, &refund_txid, &refund).await;
    prove_tip_reorg_and_actor_rebroadcast(
        client,
        &claim,
        &refund,
        &claim_contract,
        &claim_request,
        funder_key,
    )
    .await;
}

#[tokio::test]
#[ignore = "requires scripts/run-zebra-e2e.sh and pinned Docker Zebra"]
async fn real_actor_keys_fund_claim_and_refund_through_zebra_consensus() {
    let client = client();
    let funder_key = key(4);
    let claimant_key = key(2);
    let claim_destination = TransparentAddress::from_pubkey(&public_key(&key(3)));
    let refund_destination = TransparentAddress::from_pubkey(&public_key(&key(5)));

    generate_to(&client, 104).await;
    let candidates = actor_utxos(&client).await;
    assert!(candidates.len() >= 4, "actor needs four independent UTXOs");
    let first = fetched_utxo(&client, &candidates[0]).await;
    let second = fetched_utxo(&client, &candidates[1]).await;
    let third = fetched_utxo(&client, &candidates[2]).await;
    let fourth = fetched_utxo(&client, &candidates[3]).await;

    let claim_contract = Bip199Contract::new(
        block_count(&client).await + 20,
        pubkey_hash(&funder_key),
        SECRET_DIGEST,
        pubkey_hash(&claimant_key),
    );
    let claim_funding = funding_transaction(
        &claim_contract,
        first,
        &funder_key,
        block_count(&client).await,
    );
    assert_rejected(
        &client,
        mutate_first_signature(&claim_funding),
        "funding transaction with a mutated actor signature",
    )
    .await;
    let claim_funding_txid = broadcast(&client, &claim_funding).await;
    generate_to(&client, block_count(&client).await + 1).await;
    assert_confirmed(&client, &claim_funding_txid, &claim_funding).await;

    let claim_request = TransparentSpendRequest::new(
        &claim_contract,
        OutPoint::new(*claim_funding.txid().as_ref(), 0),
        contract_output(&claim_funding),
        claim_destination,
        zatoshis(10_000),
        BlockHeight::from_u32(block_count(&client).await + 40),
        BranchId::Nu6_2,
    )
    .expect("confirmed contract output is spendable");
    let claim = build_claim_transaction(&claim_contract, &claim_request, &claimant_key, &PREIMAGE)
        .expect("claimant constructs the hashlock spend");
    assert_rejected(
        &client,
        mutate_first_signature(&claim),
        "claim with a mutated claimant signature",
    )
    .await;
    let claim_txid = broadcast(&client, &claim).await;
    generate_to(&client, block_count(&client).await + 1).await;
    assert_confirmed(&client, &claim_txid, &claim).await;

    let refund_at = block_count(&client).await + 3;
    let refund_contract = Bip199Contract::new(
        refund_at,
        pubkey_hash(&funder_key),
        SECRET_DIGEST,
        pubkey_hash(&claimant_key),
    );
    let refund_funding = funding_transaction(
        &refund_contract,
        second,
        &funder_key,
        block_count(&client).await,
    );
    let refund_funding_txid = broadcast(&client, &refund_funding).await;
    generate_to(&client, block_count(&client).await + 1).await;
    assert_confirmed(&client, &refund_funding_txid, &refund_funding).await;

    let refund_request = TransparentSpendRequest::new(
        &refund_contract,
        OutPoint::new(*refund_funding.txid().as_ref(), 0),
        contract_output(&refund_funding),
        refund_destination,
        zatoshis(10_000),
        BlockHeight::from_u32(refund_at + 40),
        BranchId::Nu6_2,
    )
    .expect("confirmed contract output is refundable");
    let refund = build_refund_transaction(&refund_contract, &refund_request, &funder_key)
        .expect("funder constructs the timelock spend");
    assert_rejected(&client, tx_hex(&refund), "refund before its CLTV height").await;
    generate_to(&client, refund_at).await;
    let refund_txid = broadcast(&client, &refund).await;
    generate_to(&client, block_count(&client).await + 1).await;
    assert_confirmed(&client, &refund_txid, &refund).await;

    prove_concurrent_claim_refund_and_tip_reorg(&client, &funder_key, third, fourth).await;

    eprintln!(
        "Zebra accepted actor claim {} and refund {}",
        hex_summary(&claim),
        hex_summary(&refund)
    );
}
