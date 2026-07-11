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

fn client_from_env(variable: &str) -> HttpClient {
    let endpoint = std::env::var(variable)
        .unwrap_or_else(|_| panic!("{variable} is supplied by scripts/run-zebra-e2e.sh"));
    HttpClientBuilder::default()
        .request_timeout(Duration::from_mins(2))
        .build(endpoint)
        .expect("isolated Zebra RPC URL is valid")
}

fn client() -> HttpClient {
    client_from_env("ZEBRA_RPC_URL")
}

fn fork_client() -> HttpClient {
    client_from_env("ZEBRA_FORK_RPC_URL")
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

async fn block_hash(client: &HttpClient, height: u32) -> String {
    client
        .request("getblockhash", rpc_params![height])
        .await
        .expect("Zebra returns the canonical block hash")
}

async fn relay_canonical_blocks(
    source: &HttpClient,
    destination: &HttpClient,
    first_height: u32,
    last_height: u32,
) {
    for height in first_height..=last_height {
        let hash = block_hash(source, height).await;
        let raw: String = source
            .request("getblock", rpc_params![&hash, 0])
            .await
            .expect("source Zebra returns canonical raw block bytes");
        let response: Value = destination
            .request("submitblock", rpc_params![raw])
            .await
            .expect("destination Zebra accepts the relayed consensus-valid block");
        assert!(
            response.is_null(),
            "submitblock succeeds with a null result"
        );
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

async fn prove_accepted_competing_fork(
    client: &HttpClient,
    fork_client: &HttpClient,
    claim: &Transaction,
    refund: &Transaction,
    claim_contract: &Bip199Contract,
    claim_request: &TransparentSpendRequest,
    funder_key: &SecretKey,
) {
    let claim_txid = claim.txid().to_string();
    let refund_txid = refund.txid().to_string();
    let conflicting_refund = build_refund_transaction(claim_contract, claim_request, funder_key)
        .expect("the timed-out funder can construct a conflicting valid refund");
    let conflicting_refund_txid = conflicting_refund.txid().to_string();

    let common_height = block_count(client).await;
    let fork_height = block_count(fork_client).await;
    assert!(fork_height <= common_height);
    if fork_height < common_height {
        relay_canonical_blocks(client, fork_client, fork_height + 1, common_height).await;
    }
    assert_eq!(block_count(fork_client).await, common_height);
    assert_eq!(
        block_hash(client, common_height).await,
        block_hash(fork_client, common_height).await,
        "both isolated nodes begin from the exact same canonical prefix"
    );

    let (claim_rpc_txid, refund_rpc_txid) =
        tokio::join!(broadcast(client, claim), broadcast(client, refund));
    assert_eq!(claim_rpc_txid, claim_txid);
    assert_eq!(refund_rpc_txid, refund_txid);
    generate_to(client, common_height + 3).await;
    let mut old_branch_hashes = Vec::with_capacity(3);
    for height in common_height + 1..=common_height + 3 {
        old_branch_hashes.push(block_hash(client, height).await);
    }
    let old_first_hash = &old_branch_hashes[0];
    let old_tip_hash = block_hash(client, common_height + 3).await;
    assert_confirmed(client, &claim_txid, claim).await;
    assert_confirmed(client, &refund_txid, refund).await;

    let (replacement_rpc_txid, shared_refund_rpc_txid) = tokio::join!(
        broadcast(fork_client, &conflicting_refund),
        broadcast(fork_client, refund)
    );
    assert_eq!(replacement_rpc_txid, conflicting_refund_txid);
    assert_eq!(shared_refund_rpc_txid, refund_txid);
    generate_to(fork_client, common_height + 4).await;
    let replacement_first_hash = block_hash(fork_client, common_height + 1).await;
    let replacement_tip_hash = block_hash(fork_client, common_height + 4).await;
    assert_ne!(
        replacement_first_hash, *old_first_hash,
        "the nodes mined distinct competing branches"
    );
    assert_confirmed(fork_client, &conflicting_refund_txid, &conflicting_refund).await;
    assert_confirmed(fork_client, &refund_txid, refund).await;

    relay_canonical_blocks(fork_client, client, common_height + 1, common_height + 4).await;
    assert_eq!(block_count(client).await, common_height + 4);
    assert_eq!(
        block_hash(client, common_height + 4).await,
        replacement_tip_hash,
        "primary Zebra accepts the higher-work competing branch"
    );
    for (offset, old_hash) in old_branch_hashes.iter().enumerate() {
        let height = common_height + 1 + u32::try_from(offset).expect("three offsets fit u32");
        let canonical_hash = block_hash(client, height).await;
        assert_ne!(canonical_hash, *old_hash, "the old branch is detached");
        assert_eq!(
            canonical_hash,
            block_hash(fork_client, height).await,
            "the replacement branch is canonical at every detached height"
        );
    }

    let detached_header = client
        .request::<Value, _>("getblockheader", rpc_params![&old_tip_hash, true])
        .await;
    assert!(
        detached_header.is_err(),
        "Zebra 5.2.0 must not report the evicted old tip as canonical"
    );

    let canonical_replacement: Value = client
        .request(
            "getrawtransaction",
            rpc_params![&conflicting_refund_txid, 1],
        )
        .await
        .expect("the conflicting actor refund is canonical after replacement");
    assert_eq!(canonical_replacement["hex"], tx_hex(&conflicting_refund));
    assert_eq!(canonical_replacement["in_active_chain"], true);
    assert!(
        canonical_replacement["confirmations"]
            .as_u64()
            .expect("canonical replacement has confirmations")
            >= 4
    );
    assert_confirmed(client, &refund_txid, refund).await;
}

async fn prove_concurrent_claim_refund_and_tip_reorg(
    client: &HttpClient,
    fork_client: &HttpClient,
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
    prove_accepted_competing_fork(
        client,
        fork_client,
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
    let fork_client = fork_client();
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

    prove_concurrent_claim_refund_and_tip_reorg(&client, &fork_client, &funder_key, third, fourth)
        .await;

    eprintln!(
        "Zebra accepted actor claim {} and refund {}",
        hex_summary(&claim),
        hex_summary(&refund)
    );
}
