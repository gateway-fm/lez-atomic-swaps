//! Real-node restart proof for the maker Zcash observation runtime.

use std::{path::PathBuf, time::Duration};

use jsonrpsee::{core::client::ClientT, rpc_params};
use jsonrpsee_http_client::{HttpClient, HttpClientBuilder};
use lez_maker_node::{apply_zcash_funding_event, load_zcash_observation_tracker};
use lez_swap_core::{
    Chain, ChainPosition, ConfirmationPolicy, Pair, Participant, Phase, RecoverySchedule,
    SwapCoordinator, SwapDirection, SwapId, TimelockSafety,
};
use lez_swap_store::SqliteSwapStore;
use lez_zec_swap_sdk::{
    Bip199Contract, CanonicalZcashOutputObservation, CanonicalZcashOutputRemoval,
    ExpectedBip199Output, TransparentFundingRequest, TransparentUtxo, ZcashNodeRemovalSnapshot,
    ZcashNodeSnapshot, ZcashObservationEvent, ZcashObservationReconciliation,
    ZcashObservationTracker, ZcashStableTip, ZecProfileId, ZecSwapBinding,
    build_funding_transaction,
};
use rusqlite::Connection;
use secp256k1::{PublicKey, Secp256k1, SecretKey};
use serde_json::{Value, json};
use zcash_encoding::ReverseHex;
use zcash_primitives::{block::BlockHash, transaction::Transaction};
use zcash_protocol::{
    consensus::{BlockHeight, BranchId, NetworkType},
    value::Zatoshis,
};
use zcash_transparent::{address::TransparentAddress, bundle::OutPoint};

const MINER_ADDRESS: &str = "tmNAP26Sw5Ra2jepAoTr1kqdkggawba6Akd";
const CONTRACT_VALUE: u64 = 100_000_000;

fn client(variable: &str) -> HttpClient {
    let endpoint = std::env::var(variable)
        .unwrap_or_else(|_| panic!("{variable} is supplied by scripts/run-zebra-e2e.sh"));
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

async fn block_hash(client: &HttpClient, height: u32) -> String {
    client
        .request("getblockhash", rpc_params![height])
        .await
        .expect("Zebra returns the canonical block hash")
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

async fn relay_blocks(
    source: &HttpClient,
    destination: &HttpClient,
    first_height: u32,
    last_height: u32,
) {
    for height in first_height..=last_height {
        let hash = block_hash(source, height).await;
        let raw: String = source
            .request("getblock", rpc_params![hash, 0])
            .await
            .expect("source Zebra returns canonical raw block bytes");
        let response: Value = destination
            .request("submitblock", rpc_params![raw])
            .await
            .expect("destination Zebra accepts the relayed block");
        assert!(response.is_null(), "submitblock succeeds with null");
    }
}

fn key(byte: u8) -> SecretKey {
    SecretKey::from_slice(&[byte; 32]).expect("fixed actor key is valid")
}

fn public_key(secret_key: &SecretKey) -> PublicKey {
    PublicKey::from_secret_key(&Secp256k1::new(), secret_key)
}

fn pubkey_hash(secret_key: &SecretKey) -> [u8; 20] {
    match TransparentAddress::from_pubkey(&public_key(secret_key)) {
        TransparentAddress::PublicKeyHash(hash) => hash,
        TransparentAddress::ScriptHash(_) => unreachable!("a public key yields P2PKH"),
    }
}

fn zatoshis(value: u64) -> Zatoshis {
    Zatoshis::from_u64(value).expect("test value is in monetary range")
}

fn transaction_hex(transaction: &Transaction) -> String {
    let mut bytes = Vec::new();
    transaction
        .write(&mut bytes)
        .expect("transaction serialization succeeds");
    hex::encode(bytes)
}

async fn actor_utxo(client: &HttpClient) -> TransparentUtxo {
    let response: Value = client
        .request(
            "getaddressutxos",
            rpc_params![json!({"addresses": [MINER_ADDRESS], "chaininfo": true})],
        )
        .await
        .expect("Zebra returns the miner's transparent UTXOs");
    let entry = response
        .as_array()
        .and_then(|entries| entries.first())
        .expect("the deterministic miner has a mature UTXO");
    let transaction_id = entry["txid"].as_str().expect("UTXO has a txid");
    let output_index = u32::try_from(
        entry["outputIndex"]
            .as_u64()
            .expect("UTXO has an output index"),
    )
    .expect("output index fits u32");
    let raw: String = client
        .request("getrawtransaction", rpc_params![transaction_id, 0])
        .await
        .expect("coinbase transaction remains queryable");
    let transaction = Transaction::read(
        hex::decode(raw)
            .expect("transaction hex decodes")
            .as_slice(),
        BranchId::Nu6_2,
    )
    .expect("NU6.2 coinbase transaction decodes");
    assert_eq!(transaction.txid().to_string(), transaction_id);
    let output = transaction
        .transparent_bundle()
        .expect("coinbase has a transparent bundle")
        .vout[usize::try_from(output_index).expect("u32 fits usize")]
    .clone();
    TransparentUtxo::new(
        OutPoint::new(*transaction.txid().as_ref(), output_index),
        output,
    )
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
        zatoshis(CONTRACT_VALUE),
        zatoshis(10_000),
        zatoshis(10_000),
        BlockHeight::from_u32(tip + 40),
        BranchId::Nu6_2,
    )
    .expect("the actor owns the selected mature output");
    build_funding_transaction(contract, &request, actor_key)
        .expect("canonical funding transaction builds")
}

async fn broadcast(client: &HttpClient, transaction: &Transaction) -> String {
    let transaction_id: String = client
        .request(
            "sendrawtransaction",
            rpc_params![transaction_hex(transaction)],
        )
        .await
        .expect("Zebra accepts the signed funding transaction");
    assert_eq!(transaction_id, transaction.txid().to_string());
    transaction_id
}

fn block_hash_from_rpc(value: &str) -> BlockHash {
    BlockHash(ReverseHex::decode(value).expect("RPC block hash is canonical reverse hex"))
}

fn stable_tip(before: &Value, after: &Value) -> ZcashStableTip {
    assert_eq!(before["blocks"], after["blocks"]);
    assert_eq!(before["bestblockhash"], after["bestblockhash"]);
    let height = u32::try_from(before["blocks"].as_u64().expect("tip height is numeric"))
        .expect("Regtest height fits u32");
    let hash = block_hash_from_rpc(
        before["bestblockhash"]
            .as_str()
            .expect("tip hash is textual"),
    );
    ZcashStableTip::new(
        hash,
        BlockHeight::from_u32(height),
        hash,
        BlockHeight::from_u32(height),
    )
}

async fn chain_info(client: &HttpClient) -> Value {
    let info: Value = client
        .request("getblockchaininfo", rpc_params![])
        .await
        .expect("Zebra returns chain context");
    assert_eq!(info["chain"], "test");
    assert_eq!(info["consensus"]["chaintip"], "5437f330");
    info
}

async fn canonical_observation(
    client: &HttpClient,
    transaction: &Transaction,
    expected: &ExpectedBip199Output,
) -> CanonicalZcashOutputObservation {
    let transaction_id = transaction.txid().to_string();
    let observed: Value = client
        .request("getrawtransaction", rpc_params![&transaction_id, 1])
        .await
        .expect("canonical funding transaction is queryable");
    let before = chain_info(client).await;
    let height = u32::try_from(
        observed["height"]
            .as_i64()
            .expect("canonical transaction has a nonnegative height"),
    )
    .expect("Regtest height fits u32");
    let transaction_block_hash = observed["blockhash"]
        .as_str()
        .expect("canonical transaction has a block hash");
    let canonical_hash = block_hash(client, height).await;
    let after = chain_info(client).await;
    let snapshot = ZcashNodeSnapshot::new(
        NetworkType::Regtest,
        BranchId::Nu6_2,
        observed["in_active_chain"]
            .as_bool()
            .expect("verbose response has active-chain membership"),
        block_hash_from_rpc(transaction_block_hash),
        block_hash_from_rpc(&canonical_hash),
        BlockHeight::from_u32(height),
        stable_tip(&before, &after),
        transaction.txid(),
        hex::decode(observed["hex"].as_str().expect("verbose response has hex"))
            .expect("transaction hex decodes"),
        0,
        u32::try_from(
            observed["confirmations"]
                .as_u64()
                .expect("canonical transaction has confirmations"),
        )
        .expect("confirmation depth fits u32"),
    );
    CanonicalZcashOutputObservation::validate(expected, &snapshot)
        .expect("real node evidence matches immutable BIP-199 terms")
}

async fn removal_observation(
    client: &HttpClient,
    previous: &CanonicalZcashOutputObservation,
) -> CanonicalZcashOutputRemoval {
    let before = chain_info(client).await;
    let replacement_hash = block_hash(client, u32::from(previous.block_height())).await;
    let detached = client
        .request::<Value, _>(
            "getrawtransaction",
            rpc_params![previous.transaction_id().to_string(), 1],
        )
        .await;
    if let Ok(transaction) = detached {
        assert_ne!(
            transaction["in_active_chain"], true,
            "detached funding must not still be reported in the active chain"
        );
    }
    let after = chain_info(client).await;
    CanonicalZcashOutputRemoval::validate(
        previous,
        &ZcashNodeRemovalSnapshot::new(
            NetworkType::Regtest,
            BranchId::Nu6_2,
            block_hash_from_rpc(&replacement_hash),
            stable_tip(&before, &after),
        ),
    )
    .expect("stable changed-height evidence proves the funding was detached")
}

fn swap(refund_height: u32) -> SwapCoordinator {
    SwapCoordinator::new_with_confirmation_policies(
        SwapId::new("zebra-runtime-restart").expect("fixed swap id is valid"),
        Pair::Zcash,
        SwapDirection::TakerSellsForeign,
        ConfirmationPolicy::new(1).expect("one confirmation is valid"),
        ConfirmationPolicy::new(1).expect("one confirmation is valid"),
        RecoverySchedule::new(
            Pair::Zcash,
            SwapDirection::TakerSellsForeign,
            ChainPosition::block_height(Chain::Lez, 100),
            ChainPosition::block_height(Chain::Zcash, u64::from(refund_height)),
            TimelockSafety::between(Chain::Lez, Chain::Zcash, 1_000, 1_200, 100)
                .expect("fixture safety bounds are ordered"),
        )
        .expect("forward ZEC recovery schedule is valid"),
    )
}

fn database_path() -> PathBuf {
    let path = PathBuf::from(
        std::env::var("ZEBRA_E2E_DB")
            .expect("scripts/run-zebra-e2e.sh supplies an isolated maker database"),
    );
    assert!(
        path.is_absolute(),
        "maker E2E database path must be absolute"
    );
    assert!(
        path.parent().is_some_and(std::path::Path::is_dir),
        "maker E2E database parent must exist"
    );
    assert!(
        !path.exists(),
        "maker E2E database must be new for this run"
    );
    path
}

struct RealFunding {
    transaction: Transaction,
    expected: ExpectedBip199Output,
    binding: ZecSwapBinding,
    canonical: CanonicalZcashOutputObservation,
    refund_height: u32,
    inclusion_height: u32,
}

async fn fund_on_primary_from_shared_prefix(
    primary: &HttpClient,
    fork: &HttpClient,
) -> RealFunding {
    generate_to(primary, 104).await;
    let common_height = block_count(primary).await;
    let fork_height = block_count(fork).await;
    assert!(fork_height <= common_height);
    if fork_height < common_height {
        relay_blocks(primary, fork, fork_height + 1, common_height).await;
    }
    assert_eq!(block_count(fork).await, common_height);
    assert_eq!(
        block_hash(primary, common_height).await,
        block_hash(fork, common_height).await,
        "the isolated nodes share the exact pre-funding prefix"
    );

    let funder = key(4);
    let claimant = key(2);
    let refund_height = common_height + 20;
    let contract = Bip199Contract::new(
        refund_height,
        pubkey_hash(&funder),
        [0x22; 32],
        pubkey_hash(&claimant),
    );
    let expected = ExpectedBip199Output::new(
        NetworkType::Regtest,
        BranchId::Nu6_2,
        zatoshis(CONTRACT_VALUE),
        contract.clone(),
    );
    let binding = ZecSwapBinding::new(ZecProfileId::DeterministicLocalV1, expected.clone())
        .expect("deterministic profile matches Regtest NU6.2");
    let transaction =
        funding_transaction(&contract, actor_utxo(primary).await, &funder, common_height);
    let transaction_id = broadcast(primary, &transaction).await;
    let inclusion_height = common_height + 1;
    generate_to(primary, inclusion_height).await;
    let canonical = canonical_observation(primary, &transaction, &expected).await;
    assert_eq!(canonical.transaction_id().to_string(), transaction_id);
    assert_eq!(u32::from(canonical.block_height()), inclusion_height);
    RealFunding {
        transaction,
        expected,
        binding,
        canonical,
        refund_height,
        inclusion_height,
    }
}

fn commit_canonical_and_restart(
    database: &std::path::Path,
    funding: &RealFunding,
) -> (SqliteSwapStore, SwapId, ZcashObservationTracker) {
    let swap = swap(funding.refund_height);
    let swap_id = swap.id().clone();
    let mut store = SqliteSwapStore::open(database).expect("open schema-v10 maker store");
    store
        .save_with_zcash_binding(&swap, &funding.binding)
        .expect("swap and immutable ZEC binding commit atomically");
    let first = apply_zcash_funding_event(
        &mut store,
        0,
        &swap_id,
        &ZcashObservationEvent::Canonical(funding.canonical.clone()),
    )
    .expect("maker runtime journals real canonical evidence");
    assert_eq!(first.swap().phase(), Phase::TakerLockConfirmed);
    assert_eq!(first.commit().revision(), 1);
    assert!(!first.commit().was_replay());
    assert_eq!(
        store
            .load_zcash_events(&swap_id, Participant::Taker)
            .expect("load canonical journal")
            .len(),
        1
    );
    drop(store);

    let store = SqliteSwapStore::open(database).expect("restart maker store");
    assert_eq!(
        store
            .load_zcash_binding(&swap_id)
            .expect("reload and revalidate binding"),
        Some(funding.binding.clone())
    );
    let tracker = load_zcash_observation_tracker(&store, &swap_id)
        .expect("replay schema-v10 journal after restart");
    assert_eq!(tracker.current(), Some(&funding.canonical));
    (store, swap_id, tracker)
}

async fn detach_funding(
    primary: &HttpClient,
    fork: &HttpClient,
    funding: &RealFunding,
) -> ZcashObservationReconciliation {
    generate_to(fork, funding.inclusion_height + 1).await;
    let old_inclusion_hash = block_hash(primary, funding.inclusion_height).await;
    relay_blocks(
        fork,
        primary,
        funding.inclusion_height,
        funding.inclusion_height + 1,
    )
    .await;
    assert_eq!(block_count(primary).await, funding.inclusion_height + 1);
    assert_ne!(
        block_hash(primary, funding.inclusion_height).await,
        old_inclusion_hash,
        "the funding inclusion block is detached"
    );
    assert_eq!(
        block_hash(primary, funding.inclusion_height + 1).await,
        block_hash(fork, funding.inclusion_height + 1).await,
        "primary adopts the strictly longer isolated fork"
    );
    ZcashObservationReconciliation::Removed(removal_observation(primary, &funding.canonical).await)
}

fn assert_removal_survives_restart(
    database: &std::path::Path,
    swap_id: &SwapId,
    binding: &ZecSwapBinding,
    event: &ZcashObservationEvent,
) {
    let mut store = SqliteSwapStore::open(database).expect("restart after removal commit");
    assert_eq!(
        store
            .load_zcash_binding(swap_id)
            .expect("binding survives second restart"),
        Some(binding.clone())
    );
    assert_eq!(
        load_zcash_observation_tracker(&store, swap_id)
            .expect("replay canonical then removal")
            .current(),
        None
    );
    let replay = apply_zcash_funding_event(&mut store, 1, swap_id, event)
        .expect("unknown-outcome retry reloads the exact committed event");
    assert_eq!(replay.commit().revision(), 2);
    assert!(replay.commit().was_replay());
    assert_eq!(
        store
            .load_zcash_events(swap_id, Participant::Taker)
            .expect("replay does not append")
            .len(),
        2
    );
    drop(store);

    let connection = Connection::open(database).expect("inspect durable schema evidence");
    let schema_version: i64 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .expect("read schema version");
    let binding_rows: i64 = connection
        .query_row("SELECT COUNT(*) FROM zcash_swap_bindings", [], |row| {
            row.get(0)
        })
        .expect("count binding rows");
    let journal_rows: i64 = connection
        .query_row("SELECT COUNT(*) FROM chain_events", [], |row| row.get(0))
        .expect("count journal rows");
    assert_eq!(schema_version, 12, "update this audit for every migration");
    assert_eq!(binding_rows, 1);
    assert_eq!(journal_rows, 2);
}

#[tokio::test]
#[ignore = "requires scripts/run-zebra-e2e.sh and two pinned Docker Zebra nodes"]
async fn canonical_funding_is_requeried_across_store_restart_and_real_removal() {
    let primary = client("ZEBRA_RPC_URL");
    let fork = client("ZEBRA_FORK_RPC_URL");
    let database = database_path();

    let funding = fund_on_primary_from_shared_prefix(&primary, &fork).await;
    let (mut store, swap_id, tracker) = commit_canonical_and_restart(&database, &funding);
    let unchanged = canonical_observation(&primary, &funding.transaction, &funding.expected).await;
    assert_eq!(unchanged, funding.canonical);
    assert_eq!(
        tracker
            .propose(&ZcashObservationReconciliation::Canonical(unchanged))
            .expect("exact stable requery is valid"),
        None,
        "restart requery must not append duplicate evidence"
    );
    assert_eq!(store.revision(&swap_id).expect("load revision"), Some(1));

    let removal = detach_funding(&primary, &fork, &funding).await;
    let event = tracker
        .propose(&removal)
        .expect("fresh removal matches the durable tracker head")
        .expect("canonical funding removal is meaningful");
    let removed = apply_zcash_funding_event(&mut store, 1, &swap_id, &event)
        .expect("maker runtime atomically journals the real removal");
    assert_eq!(removed.swap().phase(), Phase::Offered);
    assert_eq!(removed.commit().revision(), 2);
    assert!(!removed.commit().was_replay());
    assert_eq!(
        store
            .load_zcash_events(&swap_id, Participant::Taker)
            .expect("load canonical and removal journal")
            .len(),
        2
    );
    drop(store);
    assert_removal_survives_restart(&database, &swap_id, &funding.binding, &event);
}
