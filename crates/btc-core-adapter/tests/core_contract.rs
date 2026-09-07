mod support;

use std::collections::{BTreeMap, VecDeque};
use std::fmt;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use bitcoin::blockdata::constants::genesis_block;
use bitcoin::consensus::{deserialize, serialize};
use bitcoin::{BlockHash, Network, OutPoint, Transaction, Txid};
use corepc_types::v31::{
    GetBlockHash, GetBlockHeaderVerbose, GetBlockchainInfo, GetIndexInfo, GetIndexInfoName,
    GetNetworkInfo, GetRawTransactionVerbose, GetTxSpendingPrevout, GetTxSpendingPrevoutItem,
    MempoolAcceptance, SendRawTransaction, TestMempoolAccept,
};
use lez_btc_core_adapter::{
    AuthorizedClaimSubmission, AuthorizedFundingSubmission, AuthorizedRefundSubmission,
    BitcoinCoreAdapter, BitcoinCoreRpc, ClaimObservation, ClaimSubmissionAcquire,
    ClaimSubmissionAttempt, ClaimSubmissionState, ClaimSubmissionStore, CoreAdapterError,
    CoreConnectivityPolicy, ExactFundingObservation, FundingObservation, RefundObservation,
    SendFailure,
};

use support::{
    REGTEST_GENESIS, REQUIRED_CONFIRMATIONS, raw_verbose, swap_fixture,
    swap_fixture_for_bitcoin_network,
};

const TIP_A: &str = "6f8c2a4d807e31d3f650d7228af87f9e75bfac506bdf9c7730483cf1524e7ac4";
const TIP_B: &str = "5d7c2a4d807e31d3f650d7228af87f9e75bfac506bdf9c7730483cf1524e7ac4";

#[derive(Clone, Debug, Eq, PartialEq)]
enum MockError {
    Transport,
    Rejected,
}

impl fmt::Display for MockError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Transport => "transport failure",
            Self::Rejected => "definitive rejection",
        })
    }
}

impl std::error::Error for MockError {}

#[derive(Clone)]
struct MockRpc {
    inner: Arc<Mutex<MockResponses>>,
}

struct MockResponses {
    calls: Vec<&'static str>,
    network: GetNetworkInfo,
    chains: VecDeque<GetBlockchainInfo>,
    genesis: GetBlockHash,
    indexes: GetIndexInfo,
    raw: VecDeque<Option<GetRawTransactionVerbose>>,
    headers: VecDeque<GetBlockHeaderVerbose>,
    spender: VecDeque<GetTxSpendingPrevout>,
    mempool: VecDeque<Result<TestMempoolAccept, MockError>>,
    send: VecDeque<Result<SendRawTransaction, MockError>>,
}

impl MockRpc {
    fn ready() -> Self {
        Self::ready_at(200)
    }

    fn ready_at(height: u32) -> Self {
        Self {
            inner: Arc::new(Mutex::new(MockResponses {
                calls: Vec::new(),
                network: network_info(310_100, "/Satoshi:31.1.0/", false),
                chains: VecDeque::from([
                    chain_info_at(TIP_A, height),
                    chain_info_at(TIP_A, height),
                ]),
                genesis: GetBlockHash(REGTEST_GENESIS.to_owned()),
                indexes: index_info_at(true, true, height),
                raw: VecDeque::new(),
                headers: VecDeque::from([block_header(
                    TIP_A,
                    height.saturating_sub(REQUIRED_CONFIRMATIONS - 1),
                    REQUIRED_CONFIRMATIONS,
                    1_699_998_900,
                )]),
                spender: VecDeque::new(),
                mempool: VecDeque::new(),
                send: VecDeque::new(),
            })),
        }
    }

    fn calls(&self) -> Vec<&'static str> {
        self.inner.lock().expect("mock lock").calls.clone()
    }

    fn push_raw(&self, value: GetRawTransactionVerbose) {
        self.inner
            .lock()
            .expect("mock lock")
            .raw
            .push_back(Some(value));
    }

    fn push_spender(&self, value: GetTxSpendingPrevout) {
        self.inner
            .lock()
            .expect("mock lock")
            .spender
            .push_back(value);
    }

    fn replace_header(&self, value: GetBlockHeaderVerbose) {
        self.inner.lock().expect("mock lock").headers = VecDeque::from([value]);
    }

    fn push_mempool(&self, value: Result<TestMempoolAccept, MockError>) {
        self.inner
            .lock()
            .expect("mock lock")
            .mempool
            .push_back(value);
    }

    fn push_send(&self, value: Result<SendRawTransaction, MockError>) {
        self.inner.lock().expect("mock lock").send.push_back(value);
    }
}

#[async_trait]
impl BitcoinCoreRpc for MockRpc {
    type Error = MockError;

    async fn get_network_info(&self) -> Result<GetNetworkInfo, Self::Error> {
        let mut inner = self.inner.lock().expect("mock lock");
        inner.calls.push("getnetworkinfo");
        Ok(inner.network.clone())
    }

    async fn get_blockchain_info(&self) -> Result<GetBlockchainInfo, Self::Error> {
        let mut inner = self.inner.lock().expect("mock lock");
        inner.calls.push("getblockchaininfo");
        inner.chains.pop_front().ok_or(MockError::Transport)
    }

    async fn get_genesis_hash(&self) -> Result<GetBlockHash, Self::Error> {
        let mut inner = self.inner.lock().expect("mock lock");
        inner.calls.push("getblockhash");
        Ok(inner.genesis.clone())
    }

    async fn get_index_info(&self) -> Result<GetIndexInfo, Self::Error> {
        let mut inner = self.inner.lock().expect("mock lock");
        inner.calls.push("getindexinfo");
        Ok(inner.indexes.clone())
    }

    async fn get_raw_transaction(
        &self,
        _transaction_id: Txid,
    ) -> Result<Option<GetRawTransactionVerbose>, Self::Error> {
        let mut inner = self.inner.lock().expect("mock lock");
        inner.calls.push("getrawtransaction");
        inner.raw.pop_front().ok_or(MockError::Transport)
    }

    async fn get_block_header(
        &self,
        _block_hash: BlockHash,
    ) -> Result<GetBlockHeaderVerbose, Self::Error> {
        let mut inner = self.inner.lock().expect("mock lock");
        inner.calls.push("getblockheader");
        inner.headers.pop_front().ok_or(MockError::Transport)
    }

    async fn get_tx_spending_prevout(
        &self,
        _outpoint: OutPoint,
    ) -> Result<GetTxSpendingPrevout, Self::Error> {
        let mut inner = self.inner.lock().expect("mock lock");
        inner.calls.push("gettxspendingprevout");
        inner.spender.pop_front().ok_or(MockError::Transport)
    }

    async fn test_mempool_accept(
        &self,
        _transaction: &[u8],
    ) -> Result<TestMempoolAccept, Self::Error> {
        let mut inner = self.inner.lock().expect("mock lock");
        inner.calls.push("testmempoolaccept");
        inner.mempool.pop_front().ok_or(MockError::Transport)?
    }

    async fn send_raw_transaction(
        &self,
        _transaction: &[u8],
    ) -> Result<SendRawTransaction, Self::Error> {
        let mut inner = self.inner.lock().expect("mock lock");
        inner.calls.push("sendrawtransaction");
        inner.send.pop_front().ok_or(MockError::Transport)?
    }

    fn classify_send_failure(error: &Self::Error) -> SendFailure {
        match error {
            MockError::Rejected => SendFailure::DefinitiveRejection,
            MockError::Transport => SendFailure::Unknown,
        }
    }
}

fn network_info(version: usize, subversion: &str, network_active: bool) -> GetNetworkInfo {
    serde_json::from_value(serde_json::json!({
        "version": version, "subversion": subversion, "protocolversion": 70016,
        "localservices": "0000000000000409", "localservicesnames": [], "localrelay": true,
        "timeoffset": 0, "connections": 0, "connections_in": 0, "connections_out": 0,
        "networkactive": network_active, "networks": [], "relayfee": 0.00001,
        "incrementalfee": 0.00001, "localaddresses": [], "warnings": []
    }))
    .expect("network response")
}

fn isolated_adapter(rpc: MockRpc) -> BitcoinCoreAdapter<MockRpc> {
    BitcoinCoreAdapter::new(rpc, CoreConnectivityPolicy::IsolatedLocal)
}

fn testnet4_rpc() -> MockRpc {
    let rpc = MockRpc::ready();
    let mut inner = rpc.inner.lock().expect("mock lock");
    inner.network = network_info(310_100, "/Satoshi:31.1.0/", true);
    for chain in &mut inner.chains {
        "testnet4".clone_into(&mut chain.chain);
    }
    inner.genesis = GetBlockHash(genesis_block(Network::Testnet4).block_hash().to_string());
    drop(inner);
    rpc
}

fn chain_info(tip: &str) -> GetBlockchainInfo {
    chain_info_at(tip, 200)
}

fn chain_info_at(tip: &str, height: u32) -> GetBlockchainInfo {
    serde_json::from_value(serde_json::json!({
        "chain": "regtest", "blocks": height, "headers": height, "bestblockhash": tip,
        "bits": "207fffff",
        "target": "7fffff0000000000000000000000000000000000000000000000000000000000",
        "difficulty": 4.656_542_373_906_925e-10, "time": 1_700_000_000,
        "mediantime": 1_699_999_000, "verificationprogress": 1.0,
        "initialblockdownload": false,
        "chainwork": "0000000000000000000000000000000000000000000000000000000000000192",
        "size_on_disk": 4096, "pruned": false, "warnings": []
    }))
    .expect("chain response")
}

fn block_header(
    hash: &str,
    height: u32,
    confirmations: u32,
    median_time: i64,
) -> GetBlockHeaderVerbose {
    serde_json::from_value(serde_json::json!({
        "hash": hash,
        "confirmations": confirmations,
        "height": height,
        "version": 1,
        "versionHex": "00000001",
        "merkleroot": TIP_B,
        "time": 1_699_999_000,
        "mediantime": median_time,
        "nonce": 0,
        "bits": "207fffff",
        "target": "7fffff0000000000000000000000000000000000000000000000000000000000",
        "difficulty": 4.656_542_373_906_925e-10,
        "chainwork": "0000000000000000000000000000000000000000000000000000000000000192",
        "nTx": 1,
        "previousblockhash": TIP_B,
        "nextblockhash": TIP_A
    }))
    .expect("block header response")
}

fn index_info(txindex_synced: bool, spender_synced: bool) -> GetIndexInfo {
    index_info_at(txindex_synced, spender_synced, 200)
}

fn index_info_at(txindex_synced: bool, spender_synced: bool, height: u32) -> GetIndexInfo {
    GetIndexInfo(BTreeMap::from([
        (
            "txindex".to_owned(),
            GetIndexInfoName {
                synced: txindex_synced,
                best_block_height: height,
            },
        ),
        (
            "txospenderindex".to_owned(),
            GetIndexInfoName {
                synced: spender_synced,
                best_block_height: height,
            },
        ),
    ]))
}

fn mempool_allowed(transaction: &Transaction) -> TestMempoolAccept {
    TestMempoolAccept(vec![MempoolAcceptance {
        txid: transaction.compute_txid().to_string(),
        wtxid: transaction.compute_wtxid().to_string(),
        allowed: true,
        vsize: Some(i64::try_from(transaction.vsize()).expect("vsize")),
        fees: None,
        reject_reason: None,
        reject_details: None,
    }])
}

fn mempool_rejected(transaction: &Transaction, reason: &str) -> TestMempoolAccept {
    TestMempoolAccept(vec![MempoolAcceptance {
        txid: transaction.compute_txid().to_string(),
        wtxid: transaction.compute_wtxid().to_string(),
        allowed: false,
        vsize: None,
        fees: None,
        reject_reason: Some(reason.to_owned()),
        reject_details: None,
    }])
}

#[derive(Default)]
struct MemorySubmissionStore {
    state: Mutex<Option<(ClaimSubmissionAttempt, ClaimSubmissionState)>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct StoreError;

impl fmt::Display for StoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("store failure")
    }
}
impl std::error::Error for StoreError {}

impl ClaimSubmissionStore for MemorySubmissionStore {
    type Error = StoreError;

    fn compare_and_mark_started(
        &self,
        attempt: ClaimSubmissionAttempt,
    ) -> Result<ClaimSubmissionAcquire, Self::Error> {
        let mut state = self.state.lock().expect("store lock");
        if let Some((existing_attempt, existing_state)) = state.as_ref() {
            if existing_attempt != &attempt {
                return Err(StoreError);
            }
            return Ok(ClaimSubmissionAcquire::Existing(existing_state.clone()));
        }
        *state = Some((attempt, ClaimSubmissionState::Started));
        Ok(ClaimSubmissionAcquire::Acquired)
    }

    fn record_result(
        &self,
        attempt: &ClaimSubmissionAttempt,
        result: ClaimSubmissionState,
    ) -> Result<(), Self::Error> {
        let mut state = self.state.lock().expect("store lock");
        assert_eq!(state.as_ref().map(|entry| &entry.0), Some(attempt));
        *state = Some((attempt.clone(), result));
        Ok(())
    }
}

#[tokio::test]
async fn readiness_requires_exact_core_network_genesis_and_synced_indexes() {
    let fixture = swap_fixture();
    let tip = isolated_adapter(MockRpc::ready())
        .ensure_ready(&fixture.agreement)
        .await
        .expect("ready Core 31.1");
    assert_eq!(tip.height(), 200);
    assert_eq!(tip.block_hash().to_string(), TIP_A);

    let wrong_version = MockRpc::ready();
    wrong_version.inner.lock().expect("mock lock").network =
        network_info(310_000, "/Satoshi:31.0.0/", false);
    assert!(matches!(
        isolated_adapter(wrong_version)
            .ensure_ready(&fixture.agreement)
            .await,
        Err(CoreAdapterError::WrongCoreVersion)
    ));

    let wrong_genesis = MockRpc::ready();
    wrong_genesis.inner.lock().expect("mock lock").genesis = GetBlockHash(TIP_B.to_owned());
    assert!(matches!(
        isolated_adapter(wrong_genesis)
            .ensure_ready(&fixture.agreement)
            .await,
        Err(CoreAdapterError::BitcoinGenesisMismatch)
    ));

    let unsynced = MockRpc::ready();
    unsynced.inner.lock().expect("mock lock").indexes = index_info(true, false);
    assert!(matches!(
        isolated_adapter(unsynced)
            .ensure_ready(&fixture.agreement)
            .await,
        Err(CoreAdapterError::RequiredIndexNotReady(_))
    ));

    let connected_local = MockRpc::ready();
    connected_local.inner.lock().expect("mock lock").network =
        network_info(310_100, "/Satoshi:31.1.0/", true);
    assert!(matches!(
        isolated_adapter(connected_local)
            .ensure_ready(&fixture.agreement)
            .await,
        Err(CoreAdapterError::ConnectivityPolicyMismatch)
    ));

    let networked = MockRpc::ready();
    networked.inner.lock().expect("mock lock").network =
        network_info(310_100, "/Satoshi:31.1.0/", true);
    BitcoinCoreAdapter::new(networked, CoreConnectivityPolicy::Networked)
        .ensure_ready(&fixture.agreement)
        .await
        .expect("explicit networked Core route");
}

#[tokio::test]
async fn testnet4_profile_requires_exact_chain_network_and_pinned_genesis() {
    let fixture = swap_fixture_for_bitcoin_network(Network::Testnet4);
    let rpc = testnet4_rpc();
    BitcoinCoreAdapter::new(rpc, CoreConnectivityPolicy::Testnet4Networked)
        .ensure_ready(&fixture.agreement)
        .await
        .expect("exact Testnet4 Core 31.1 route");

    for wrong_chain_name in ["test", "regtest"] {
        let wrong_chain = testnet4_rpc();
        for chain in &mut wrong_chain.inner.lock().expect("mock lock").chains {
            wrong_chain_name.clone_into(&mut chain.chain);
        }
        assert!(matches!(
            BitcoinCoreAdapter::new(wrong_chain, CoreConnectivityPolicy::Testnet4Networked)
                .ensure_ready(&fixture.agreement)
                .await,
            Err(CoreAdapterError::ChainNotReady)
        ));
    }

    let inactive = testnet4_rpc();
    inactive.inner.lock().expect("mock lock").network =
        network_info(310_100, "/Satoshi:31.1.0/", false);
    assert!(matches!(
        BitcoinCoreAdapter::new(inactive, CoreConnectivityPolicy::Testnet4Networked)
            .ensure_ready(&fixture.agreement)
            .await,
        Err(CoreAdapterError::ConnectivityPolicyMismatch)
    ));

    let wrong_node_genesis = testnet4_rpc();
    wrong_node_genesis.inner.lock().expect("mock lock").genesis =
        GetBlockHash(REGTEST_GENESIS.to_owned());
    assert!(matches!(
        BitcoinCoreAdapter::new(
            wrong_node_genesis,
            CoreConnectivityPolicy::Testnet4Networked,
        )
        .ensure_ready(&fixture.agreement)
        .await,
        Err(CoreAdapterError::BitcoinGenesisMismatch)
    ));

    let unsynced_indexes = testnet4_rpc();
    unsynced_indexes.inner.lock().expect("mock lock").indexes = index_info(true, false);
    assert!(matches!(
        BitcoinCoreAdapter::new(unsynced_indexes, CoreConnectivityPolicy::Testnet4Networked,)
            .ensure_ready(&fixture.agreement)
            .await,
        Err(CoreAdapterError::RequiredIndexNotReady("txospenderindex"))
    ));

    let missing_index = testnet4_rpc();
    missing_index
        .inner
        .lock()
        .expect("mock lock")
        .indexes
        .0
        .remove("txindex");
    assert!(matches!(
        BitcoinCoreAdapter::new(missing_index, CoreConnectivityPolicy::Testnet4Networked)
            .ensure_ready(&fixture.agreement)
            .await,
        Err(CoreAdapterError::RequiredIndexNotReady("txindex"))
    ));

    let regtest_agreement = swap_fixture();
    assert!(matches!(
        BitcoinCoreAdapter::new(testnet4_rpc(), CoreConnectivityPolicy::Testnet4Networked)
            .ensure_ready(&regtest_agreement.agreement)
            .await,
        Err(CoreAdapterError::BitcoinGenesisMismatch)
    ));

    assert!(matches!(
        BitcoinCoreAdapter::new(testnet4_rpc(), CoreConnectivityPolicy::Networked)
            .ensure_ready(&fixture.agreement)
            .await,
        Err(CoreAdapterError::ChainNotReady)
    ));
}

#[tokio::test]
async fn funding_observation_is_canonical_metric_checked_and_stable_tip_bracketed() {
    let fixture = swap_fixture();
    let rpc = MockRpc::ready();
    rpc.push_raw(raw_verbose(
        &fixture.funding,
        Some(u64::from(REQUIRED_CONFIRMATIONS)),
        Some(TIP_A),
    ));
    let observed = isolated_adapter(rpc.clone())
        .observe_funding(&fixture.agreement)
        .await
        .expect("funding observation");
    let FundingObservation::Ready(observed) = observed else {
        panic!("expected ready funding");
    };
    assert_eq!(observed.transaction(), &fixture.funding);
    assert_eq!(observed.confirmations(), REQUIRED_CONFIRMATIONS);
    assert_eq!(observed.block_height(), 195);
    assert_eq!(observed.block_median_time_unix_seconds(), 1_699_998_900);
    assert_eq!(
        observed.stable_tip().median_time_unix_seconds(),
        1_699_999_000
    );
    assert_eq!(
        rpc.calls(),
        [
            "getnetworkinfo",
            "getblockchaininfo",
            "getblockhash",
            "getindexinfo",
            "getrawtransaction",
            "getblockheader",
            "getblockchaininfo"
        ]
    );

    let metric_drift = MockRpc::ready();
    let mut wrong_metrics = raw_verbose(&fixture.funding, Some(6), Some(TIP_A));
    wrong_metrics.weight += 1;
    metric_drift.push_raw(wrong_metrics);
    assert!(matches!(
        isolated_adapter(metric_drift)
            .observe_funding(&fixture.agreement)
            .await,
        Err(CoreAdapterError::RawTransactionMetricsMismatch)
    ));

    let input_drift = MockRpc::ready();
    let mut wrong_input = raw_verbose(&fixture.funding, Some(6), Some(TIP_A));
    wrong_input.inputs[0].sequence ^= 1;
    input_drift.push_raw(wrong_input);
    assert!(matches!(
        isolated_adapter(input_drift)
            .observe_funding(&fixture.agreement)
            .await,
        Err(CoreAdapterError::RawTransactionMetricsMismatch)
    ));

    let output_drift = MockRpc::ready();
    let mut wrong_output = raw_verbose(&fixture.funding, Some(6), Some(TIP_A));
    wrong_output.outputs[1].index = 0;
    output_drift.push_raw(wrong_output);
    assert!(matches!(
        isolated_adapter(output_drift)
            .observe_funding(&fixture.agreement)
            .await,
        Err(CoreAdapterError::RawTransactionMetricsMismatch)
    ));

    let unstable = MockRpc::ready();
    unstable.inner.lock().expect("mock lock").chains =
        VecDeque::from([chain_info(TIP_A), chain_info(TIP_B)]);
    unstable.push_raw(raw_verbose(&fixture.funding, Some(6), Some(TIP_A)));
    assert!(matches!(
        isolated_adapter(unstable)
            .observe_funding(&fixture.agreement)
            .await,
        Err(CoreAdapterError::UnstableTip)
    ));

    for invalid_header in [
        block_header(TIP_B, 195, REQUIRED_CONFIRMATIONS, 1_699_998_900),
        block_header(TIP_A, 195, REQUIRED_CONFIRMATIONS - 1, 1_699_998_900),
        block_header(TIP_A, 194, REQUIRED_CONFIRMATIONS, 1_699_998_900),
        block_header(TIP_A, 195, REQUIRED_CONFIRMATIONS, -1),
    ] {
        let rpc = MockRpc::ready();
        rpc.push_raw(raw_verbose(
            &fixture.funding,
            Some(u64::from(REQUIRED_CONFIRMATIONS)),
            Some(TIP_A),
        ));
        rpc.replace_header(invalid_header);
        assert!(matches!(
            isolated_adapter(rpc)
                .observe_funding(&fixture.agreement)
                .await,
            Err(CoreAdapterError::InvalidConfirmationContext
                | CoreAdapterError::MalformedResponse(_))
        ));
    }
}

#[tokio::test]
async fn exact_funding_observation_proves_current_unspent_state_at_one_stable_tip() {
    let fixture = swap_fixture();
    let outpoint = fixture.agreement.cooperative_claim().funding_outpoint();
    let rpc = MockRpc::ready();
    rpc.push_raw(raw_verbose(
        &fixture.funding,
        Some(u64::from(REQUIRED_CONFIRMATIONS)),
        Some(TIP_A),
    ));
    rpc.push_spender(unspent(outpoint));

    let ExactFundingObservation::Unspent(observed) = isolated_adapter(rpc.clone())
        .observe_exact_funding(&fixture.agreement)
        .await
        .expect("exact confirmed funding is currently unspent")
    else {
        panic!("expected unspent exact funding");
    };
    assert_eq!(observed.transaction(), &fixture.funding);
    assert_eq!(observed.confirmations(), REQUIRED_CONFIRMATIONS);
    assert_eq!(
        rpc.calls(),
        [
            "getnetworkinfo",
            "getblockchaininfo",
            "getblockhash",
            "getindexinfo",
            "getrawtransaction",
            "getblockheader",
            "gettxspendingprevout",
            "getblockchaininfo"
        ]
    );

    let spent_rpc = MockRpc::ready();
    spent_rpc.push_raw(raw_verbose(
        &fixture.funding,
        Some(u64::from(REQUIRED_CONFIRMATIONS)),
        Some(TIP_A),
    ));
    spent_rpc.push_spender(spender(outpoint, &fixture.claim));
    let ExactFundingObservation::Spent {
        funding,
        spender_transaction_id,
    } = isolated_adapter(spent_rpc)
        .observe_exact_funding(&fixture.agreement)
        .await
        .expect("spent funding is distinct from absence")
    else {
        panic!("expected spent exact funding");
    };
    assert_eq!(funding.transaction(), &fixture.funding);
    assert_eq!(spender_transaction_id, fixture.claim.compute_txid());

    let forged_spender_rpc = MockRpc::ready();
    forged_spender_rpc.push_raw(raw_verbose(
        &fixture.funding,
        Some(u64::from(REQUIRED_CONFIRMATIONS)),
        Some(TIP_A),
    ));
    forged_spender_rpc.push_spender(spender(outpoint, &fixture.funding));
    assert!(matches!(
        isolated_adapter(forged_spender_rpc)
            .observe_exact_funding(&fixture.agreement)
            .await,
        Err(CoreAdapterError::SpenderResponseMismatch)
    ));

    let pending_rpc = MockRpc::ready();
    pending_rpc.push_raw(raw_verbose(&fixture.funding, None, None));
    let ExactFundingObservation::Pending {
        transaction,
        confirmations,
        ..
    } = isolated_adapter(pending_rpc)
        .observe_exact_funding(&fixture.agreement)
        .await
        .expect("pending exact bytes are observable but ineligible")
    else {
        panic!("expected pending exact funding");
    };
    assert_eq!(transaction, fixture.funding);
    assert_eq!(confirmations, 0);

    let unstable_rpc = MockRpc::ready();
    unstable_rpc.inner.lock().expect("mock lock").chains =
        VecDeque::from([chain_info(TIP_A), chain_info(TIP_B)]);
    unstable_rpc.push_raw(raw_verbose(
        &fixture.funding,
        Some(u64::from(REQUIRED_CONFIRMATIONS)),
        Some(TIP_A),
    ));
    unstable_rpc.push_spender(unspent(outpoint));
    assert!(matches!(
        isolated_adapter(unstable_rpc)
            .observe_exact_funding(&fixture.agreement)
            .await,
        Err(CoreAdapterError::UnstableTip)
    ));
}

#[tokio::test]
async fn claim_observation_requires_exact_spender_bytes_and_one_item_witness() {
    let fixture = swap_fixture();
    let outpoint = fixture.agreement.cooperative_claim().funding_outpoint();
    let rpc = MockRpc::ready();
    rpc.push_spender(spender(outpoint, &fixture.claim));
    rpc.push_raw(raw_verbose(&fixture.claim, Some(1), Some(TIP_A)));
    let observation = isolated_adapter(rpc)
        .observe_claim(&fixture.agreement)
        .await
        .expect("claim observation");
    let ClaimObservation::Confirming(claimed) = observation else {
        panic!("expected confirming observation");
    };
    assert_eq!(claimed.transaction(), &fixture.claim);
    assert_eq!(claimed.transaction_id(), fixture.claim.compute_txid());
    assert_eq!(claimed.confirmations(), 1);
    assert_eq!(
        claimed.block_hash().map(|hash| hash.to_string()).as_deref(),
        Some(TIP_A)
    );

    let mempool_rpc = MockRpc::ready();
    let mut mempool_spender = spender(outpoint, &fixture.claim);
    mempool_spender.0[0].block_hash = None;
    mempool_rpc.push_spender(mempool_spender);
    mempool_rpc.push_raw(raw_verbose(&fixture.claim, None, None));
    assert!(matches!(
        isolated_adapter(mempool_rpc)
            .observe_claim(&fixture.agreement)
            .await
            .expect("mempool claim"),
        ClaimObservation::Revealed(_)
    ));

    let block_drift_rpc = MockRpc::ready();
    let mut block_drift_spender = spender(outpoint, &fixture.claim);
    block_drift_spender.0[0].block_hash = Some(TIP_B.to_owned());
    block_drift_rpc.push_spender(block_drift_spender);
    block_drift_rpc.push_raw(raw_verbose(&fixture.claim, Some(1), Some(TIP_A)));
    assert!(matches!(
        isolated_adapter(block_drift_rpc)
            .observe_claim(&fixture.agreement)
            .await,
        Err(CoreAdapterError::SpenderResponseMismatch)
    ));

    let wrong_rpc = MockRpc::ready();
    let mut wrong_claim: Transaction =
        deserialize(&serialize(&fixture.claim)).expect("claim transaction");
    wrong_claim.input[0].witness = bitcoin::Witness::from_slice(&[[0x55; 64]]);
    wrong_rpc.push_spender(spender(outpoint, &wrong_claim));
    wrong_rpc.push_raw(raw_verbose(&wrong_claim, Some(1), Some(TIP_A)));
    assert!(matches!(
        isolated_adapter(wrong_rpc)
            .observe_claim(&fixture.agreement)
            .await,
        Err(CoreAdapterError::ClaimTransactionMismatch)
    ));
}

fn unspent(outpoint: OutPoint) -> GetTxSpendingPrevout {
    GetTxSpendingPrevout(vec![GetTxSpendingPrevoutItem {
        txid: outpoint.txid.to_string(),
        vout: outpoint.vout,
        spending_txid: None,
        spending_tx: None,
        block_hash: None,
    }])
}

fn spender(outpoint: OutPoint, transaction: &Transaction) -> GetTxSpendingPrevout {
    GetTxSpendingPrevout(vec![GetTxSpendingPrevoutItem {
        txid: outpoint.txid.to_string(),
        vout: outpoint.vout,
        spending_txid: Some(transaction.compute_txid().to_string()),
        spending_tx: Some(hex::encode(serialize(transaction))),
        block_hash: Some(TIP_A.to_owned()),
    }])
}

#[tokio::test]
async fn refund_observation_uses_signed_anchor_and_next_block_csv_boundary() {
    let fixture = swap_fixture();
    let outpoint = fixture.agreement.bitcoin_refund().funding_outpoint();

    let immature_rpc = MockRpc::ready_at(1_142);
    immature_rpc.push_raw(raw_verbose(&fixture.funding, Some(143), Some(TIP_A)));
    immature_rpc.push_spender(unspent(outpoint));
    let RefundObservation::Immature(immature) = isolated_adapter(immature_rpc)
        .observe_refund(&fixture.agreement)
        .await
        .expect("immature refund")
    else {
        panic!("expected immature refund");
    };
    assert_eq!(immature.funding_block_height(), 1_000);
    assert_eq!(immature.first_valid_block_height(), 1_144);

    let eligible_rpc = MockRpc::ready_at(1_143);
    eligible_rpc.push_raw(raw_verbose(&fixture.funding, Some(144), Some(TIP_A)));
    eligible_rpc.push_spender(unspent(outpoint));
    let RefundObservation::Eligible(eligible) = isolated_adapter(eligible_rpc)
        .observe_refund(&fixture.agreement)
        .await
        .expect("eligible refund")
    else {
        panic!("expected eligible refund");
    };
    assert_eq!(eligible.funding_block_height(), 1_000);
    assert_eq!(eligible.first_valid_block_height(), 1_144);
}

#[tokio::test]
async fn refund_observation_classifies_exact_finality_conflict_and_anchor_drift() {
    let fixture = swap_fixture();
    let outpoint = fixture.agreement.bitcoin_refund().funding_outpoint();

    let finalized_rpc = MockRpc::ready_at(1_149);
    finalized_rpc.push_raw(raw_verbose(&fixture.funding, Some(150), Some(TIP_A)));
    finalized_rpc.push_spender(spender(outpoint, &fixture.refund));
    finalized_rpc.push_raw(raw_verbose(&fixture.refund, Some(6), Some(TIP_A)));
    let RefundObservation::Finalized(finalized) = isolated_adapter(finalized_rpc)
        .observe_refund(&fixture.agreement)
        .await
        .expect("finalized refund")
    else {
        panic!("expected finalized refund");
    };
    assert_eq!(finalized.transaction(), &fixture.refund);
    assert_eq!(finalized.block_height(), Some(1_144));
    assert_eq!(finalized.confirmations(), 6);

    let conflicting_rpc = MockRpc::ready_at(1_149);
    conflicting_rpc.push_raw(raw_verbose(&fixture.funding, Some(150), Some(TIP_A)));
    conflicting_rpc.push_spender(spender(outpoint, &fixture.claim));
    conflicting_rpc.push_raw(raw_verbose(&fixture.claim, Some(6), Some(TIP_A)));
    assert_eq!(
        isolated_adapter(conflicting_rpc)
            .observe_refund(&fixture.agreement)
            .await
            .expect("conflicting spend is a typed terminal observation"),
        RefundObservation::ConflictingSpend
    );

    // Below the signed anchor is impossible for the planned funding: rejected.
    let below_anchor_rpc = MockRpc::ready_at(1_143);
    below_anchor_rpc.push_raw(raw_verbose(&fixture.funding, Some(145), Some(TIP_A)));
    assert!(matches!(
        isolated_adapter(below_anchor_rpc)
            .observe_refund(&fixture.agreement)
            .await,
        Err(CoreAdapterError::FundingAnchorMismatch)
    ));
    // Confirmed one block after the anchor: BIP-68 counts from the actual
    // funding block, so the refund matures one block later than planned.
    let later_funding_rpc = MockRpc::ready_at(1_143);
    later_funding_rpc.push_raw(raw_verbose(&fixture.funding, Some(143), Some(TIP_A)));
    later_funding_rpc.push_spender(unspent(outpoint));
    let RefundObservation::Immature(shifted) = isolated_adapter(later_funding_rpc)
        .observe_refund(&fixture.agreement)
        .await
        .expect("later funding is observable")
    else {
        panic!("expected an immature refund one block before the shifted boundary");
    };
    assert_eq!(shifted.funding_block_height(), 1_001);
    assert_eq!(shifted.first_valid_block_height(), 1_145);

    let early_rpc = MockRpc::ready_at(1_143);
    early_rpc.push_raw(raw_verbose(&fixture.funding, Some(144), Some(TIP_A)));
    early_rpc.push_spender(spender(outpoint, &fixture.refund));
    early_rpc.push_raw(raw_verbose(&fixture.refund, Some(1), Some(TIP_A)));
    assert!(matches!(
        isolated_adapter(early_rpc)
            .observe_refund(&fixture.agreement)
            .await,
        Err(CoreAdapterError::RefundTransactionMismatch)
    ));
}

#[tokio::test]
async fn authorized_refund_requires_exact_post_send_witness_readback() {
    let fixture = swap_fixture();
    let refund_bytes = serialize(&fixture.refund);
    let outpoint = fixture.agreement.bitcoin_refund().funding_outpoint();

    let accepted_rpc = MockRpc::ready();
    accepted_rpc.push_mempool(Ok(mempool_allowed(&fixture.refund)));
    accepted_rpc.push_send(Ok(SendRawTransaction(
        fixture.refund.compute_txid().to_string(),
    )));
    accepted_rpc.push_spender(spender(outpoint, &fixture.refund));
    assert_eq!(
        isolated_adapter(accepted_rpc.clone())
            .submit_authorized_refund(
                &fixture.agreement,
                &refund_bytes,
                fixture.refund.compute_txid(),
            )
            .await
            .expect("exact refund accepted"),
        AuthorizedRefundSubmission::Accepted {
            transaction_id: fixture.refund.compute_txid(),
            witness_transaction_id: fixture.refund.compute_wtxid(),
        }
    );

    let mut conflicting_witness = fixture.refund.clone();
    let script = fixture.agreement.p2tr_contract().refund_script_bytes();
    let control = fixture
        .agreement
        .p2tr_contract()
        .refund_control_block_bytes();
    conflicting_witness.input[0].witness =
        bitcoin::Witness::from_slice(&[&[0x55; 64], script, control.as_slice()]);
    assert_eq!(
        conflicting_witness.compute_txid(),
        fixture.refund.compute_txid()
    );
    assert_ne!(
        conflicting_witness.compute_wtxid(),
        fixture.refund.compute_wtxid()
    );
    let raced_rpc = MockRpc::ready();
    raced_rpc.push_mempool(Ok(mempool_allowed(&fixture.refund)));
    raced_rpc.push_send(Ok(SendRawTransaction(
        fixture.refund.compute_txid().to_string(),
    )));
    raced_rpc.push_spender(spender(outpoint, &conflicting_witness));
    assert_eq!(
        isolated_adapter(raced_rpc.clone())
            .submit_authorized_refund(
                &fixture.agreement,
                &refund_bytes,
                fixture.refund.compute_txid(),
            )
            .await
            .expect("same txid different witness is unknown"),
        AuthorizedRefundSubmission::Unknown
    );
    assert_eq!(
        raced_rpc
            .calls()
            .iter()
            .filter(|call| **call == "sendrawtransaction")
            .count(),
        1
    );

    let invalid_rpc = MockRpc::ready();
    assert!(matches!(
        isolated_adapter(invalid_rpc.clone())
            .submit_authorized_refund(
                &fixture.agreement,
                &serialize(&conflicting_witness),
                conflicting_witness.compute_txid(),
            )
            .await,
        Err(CoreAdapterError::RefundTransactionMismatch)
    ));
    assert!(invalid_rpc.calls().is_empty());
}

#[tokio::test]
async fn authorized_funding_is_one_send_and_requires_exact_post_send_bytes() {
    let fixture = swap_fixture();
    let funding_bytes = serialize(&fixture.funding);

    let accepted_rpc = MockRpc::ready();
    accepted_rpc.push_mempool(Ok(mempool_allowed(&fixture.funding)));
    accepted_rpc.push_send(Ok(SendRawTransaction(
        fixture.funding.compute_txid().to_string(),
    )));
    accepted_rpc.push_raw(raw_verbose(&fixture.funding, None, None));
    assert_eq!(
        isolated_adapter(accepted_rpc.clone())
            .submit_authorized_funding(
                &fixture.agreement,
                &funding_bytes,
                fixture.funding.compute_txid(),
            )
            .await
            .expect("exact funding accepted"),
        AuthorizedFundingSubmission::Accepted {
            transaction_id: fixture.funding.compute_txid(),
            witness_transaction_id: fixture.funding.compute_wtxid(),
        }
    );
    assert_eq!(
        accepted_rpc
            .calls()
            .iter()
            .filter(|call| **call == "sendrawtransaction")
            .count(),
        1
    );

    let missing_readback_rpc = MockRpc::ready();
    missing_readback_rpc.push_mempool(Ok(mempool_allowed(&fixture.funding)));
    missing_readback_rpc.push_send(Ok(SendRawTransaction(
        fixture.funding.compute_txid().to_string(),
    )));
    missing_readback_rpc
        .inner
        .lock()
        .expect("mock lock")
        .raw
        .push_back(None);
    assert_eq!(
        isolated_adapter(missing_readback_rpc.clone())
            .submit_authorized_funding(
                &fixture.agreement,
                &funding_bytes,
                fixture.funding.compute_txid(),
            )
            .await
            .expect("missing readback is conservative"),
        AuthorizedFundingSubmission::Unknown
    );
    assert_eq!(
        missing_readback_rpc
            .calls()
            .iter()
            .filter(|call| **call == "sendrawtransaction")
            .count(),
        1
    );

    let rejected_rpc = MockRpc::ready();
    rejected_rpc.push_mempool(Ok(mempool_allowed(&fixture.funding)));
    rejected_rpc.push_send(Err(MockError::Rejected));
    assert_eq!(
        isolated_adapter(rejected_rpc)
            .submit_authorized_funding(
                &fixture.agreement,
                &funding_bytes,
                fixture.funding.compute_txid(),
            )
            .await
            .expect("definitive funding rejection"),
        AuthorizedFundingSubmission::Rejected
    );
}

#[tokio::test]
async fn authorized_funding_validates_agreement_and_identity_before_rpc() {
    let fixture = swap_fixture();
    let wrong_identity_rpc = MockRpc::ready();
    assert!(matches!(
        isolated_adapter(wrong_identity_rpc.clone())
            .submit_authorized_funding(
                &fixture.agreement,
                &serialize(&fixture.funding),
                fixture.claim.compute_txid(),
            )
            .await,
        Err(CoreAdapterError::FundingTransactionMismatch)
    ));
    assert!(wrong_identity_rpc.calls().is_empty());

    let mut wrong_output = fixture.funding.clone();
    let contract_output = usize::try_from(fixture.agreement.funding_terms().output_index())
        .expect("contract output index");
    wrong_output.output[contract_output].value = bitcoin::Amount::from_sat(
        wrong_output.output[contract_output]
            .value
            .to_sat()
            .saturating_add(1),
    );
    let wrong_output_rpc = MockRpc::ready();
    assert!(matches!(
        isolated_adapter(wrong_output_rpc.clone())
            .submit_authorized_funding(
                &fixture.agreement,
                &serialize(&wrong_output),
                wrong_output.compute_txid(),
            )
            .await,
        Err(CoreAdapterError::FundingOutputMismatch)
    ));
    assert!(wrong_output_rpc.calls().is_empty());
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn claim_submission_is_durable_once_and_unknown_never_retries() {
    let fixture = swap_fixture();
    let claim_bytes = serialize(&fixture.claim);
    let rpc = MockRpc::ready();
    rpc.push_mempool(Ok(mempool_allowed(&fixture.claim)));
    rpc.push_send(Ok(SendRawTransaction(
        fixture.claim.compute_txid().to_string(),
    )));
    rpc.push_spender(spender(
        fixture.agreement.cooperative_claim().funding_outpoint(),
        &fixture.claim,
    ));
    let adapter = isolated_adapter(rpc.clone());
    let store = MemorySubmissionStore::default();
    let accepted = adapter
        .submit_claim(&fixture.agreement, &claim_bytes, &store)
        .await
        .expect("accepted claim");
    assert_eq!(
        accepted,
        ClaimSubmissionState::Accepted {
            transaction_id: fixture.claim.compute_txid()
        }
    );
    assert_eq!(
        rpc.calls()
            .iter()
            .filter(|call| **call == "testmempoolaccept")
            .count(),
        1
    );
    assert_eq!(
        rpc.calls()
            .iter()
            .filter(|call| **call == "sendrawtransaction")
            .count(),
        1
    );
    assert_eq!(
        adapter
            .submit_claim(&fixture.agreement, &claim_bytes, &store)
            .await
            .expect("durable replay"),
        accepted
    );
    assert_eq!(
        rpc.calls()
            .iter()
            .filter(|call| **call == "sendrawtransaction")
            .count(),
        1
    );

    let unknown_rpc = MockRpc::ready();
    unknown_rpc.push_mempool(Ok(mempool_allowed(&fixture.claim)));
    unknown_rpc.push_send(Err(MockError::Transport));
    let unknown_adapter = isolated_adapter(unknown_rpc.clone());
    let unknown_store = MemorySubmissionStore::default();
    assert_eq!(
        unknown_adapter
            .submit_claim(&fixture.agreement, &claim_bytes, &unknown_store)
            .await
            .expect("unknown outcome"),
        ClaimSubmissionState::Unknown
    );
    assert_eq!(
        unknown_adapter
            .submit_claim(&fixture.agreement, &claim_bytes, &unknown_store)
            .await
            .expect("unknown replay"),
        ClaimSubmissionState::Unknown
    );
    assert_eq!(
        unknown_rpc
            .calls()
            .iter()
            .filter(|call| **call == "sendrawtransaction")
            .count(),
        1
    );

    let malformed_success_rpc = MockRpc::ready();
    malformed_success_rpc.push_mempool(Ok(mempool_allowed(&fixture.claim)));
    malformed_success_rpc.push_send(Ok(SendRawTransaction("not-a-txid".to_owned())));
    let malformed_success_adapter = isolated_adapter(malformed_success_rpc);
    let malformed_success_store = MemorySubmissionStore::default();
    assert!(matches!(
        malformed_success_adapter
            .submit_claim(&fixture.agreement, &claim_bytes, &malformed_success_store)
            .await,
        Err(CoreAdapterError::BroadcastIdentityMismatch)
    ));
    assert_eq!(
        malformed_success_store
            .state
            .lock()
            .expect("store lock")
            .as_ref()
            .map(|entry| &entry.1),
        Some(&ClaimSubmissionState::Unknown)
    );

    let rejected_rpc = MockRpc::ready();
    rejected_rpc.push_mempool(Ok(mempool_allowed(&fixture.claim)));
    rejected_rpc.push_send(Err(MockError::Rejected));
    let rejected_adapter = isolated_adapter(rejected_rpc.clone());
    let rejected_store = MemorySubmissionStore::default();
    assert_eq!(
        rejected_adapter
            .submit_claim(&fixture.agreement, &claim_bytes, &rejected_store)
            .await
            .expect("definitive rejection"),
        ClaimSubmissionState::Rejected
    );
    assert_eq!(
        rejected_adapter
            .submit_claim(&fixture.agreement, &claim_bytes, &rejected_store)
            .await
            .expect("rejected replay"),
        ClaimSubmissionState::Rejected
    );
    assert_eq!(
        rejected_rpc
            .calls()
            .iter()
            .filter(|call| **call == "sendrawtransaction")
            .count(),
        1
    );
}

#[tokio::test]
async fn already_known_and_contradictory_mempool_results_never_become_rejected() {
    for reason in [
        "txn-already-in-mempool",
        "txn-same-nonwitness-data-in-mempool",
    ] {
        let fixture = swap_fixture();
        let claim_bytes = serialize(&fixture.claim);
        let rpc = MockRpc::ready();
        rpc.push_mempool(Ok(mempool_rejected(&fixture.claim, reason)));
        let store = MemorySubmissionStore::default();
        assert_eq!(
            isolated_adapter(rpc.clone())
                .submit_claim(&fixture.agreement, &claim_bytes, &store)
                .await
                .expect("already-known preflight is conservative"),
            ClaimSubmissionState::Unknown
        );
        assert!(!rpc.calls().contains(&"sendrawtransaction"));
    }

    let fixture = swap_fixture();
    let claim_bytes = serialize(&fixture.claim);
    let contradictory_rpc = MockRpc::ready();
    let mut contradictory = mempool_rejected(&fixture.claim, "mandatory-script-verify-flag-failed");
    contradictory.0[0].vsize = Some(i64::try_from(fixture.claim.vsize()).expect("vsize"));
    contradictory_rpc.push_mempool(Ok(contradictory));
    let contradictory_store = MemorySubmissionStore::default();
    assert!(matches!(
        isolated_adapter(contradictory_rpc.clone())
            .submit_claim(&fixture.agreement, &claim_bytes, &contradictory_store)
            .await,
        Err(CoreAdapterError::MempoolResponseMismatch)
    ));
    assert_eq!(
        contradictory_store
            .state
            .lock()
            .expect("store lock")
            .as_ref()
            .map(|entry| &entry.1),
        Some(&ClaimSubmissionState::Unknown)
    );
    assert!(!contradictory_rpc.calls().contains(&"sendrawtransaction"));

    let allowed_rpc = MockRpc::ready();
    let mut contradictory_allowed = mempool_allowed(&fixture.claim);
    contradictory_allowed.0[0].reject_reason = Some("contradictory".to_owned());
    allowed_rpc.push_mempool(Ok(contradictory_allowed));
    assert!(matches!(
        isolated_adapter(allowed_rpc.clone())
            .submit_claim(
                &fixture.agreement,
                &claim_bytes,
                &MemorySubmissionStore::default(),
            )
            .await,
        Err(CoreAdapterError::MempoolResponseMismatch)
    ));
    assert!(!allowed_rpc.calls().contains(&"sendrawtransaction"));
}

#[tokio::test]
async fn durable_attempt_binds_wtxid_and_exact_raw_transaction_digest() {
    let first = swap_fixture();
    let second = swap_fixture();
    assert_eq!(first.agreement, second.agreement);
    assert_eq!(first.claim.compute_txid(), second.claim.compute_txid());
    assert_ne!(first.claim.compute_wtxid(), second.claim.compute_wtxid());

    let first_bytes = serialize(&first.claim);
    let rpc = MockRpc::ready();
    rpc.push_mempool(Ok(mempool_allowed(&first.claim)));
    rpc.push_send(Err(MockError::Transport));
    let adapter = isolated_adapter(rpc.clone());
    let store = MemorySubmissionStore::default();
    assert_eq!(
        adapter
            .submit_claim(&first.agreement, &first_bytes, &store)
            .await
            .expect("first attempt becomes unknown"),
        ClaimSubmissionState::Unknown
    );
    let first_attempt = store
        .state
        .lock()
        .expect("store lock")
        .as_ref()
        .expect("attempt")
        .0
        .clone();
    assert_eq!(
        first_attempt.witness_transaction_id(),
        first.claim.compute_wtxid()
    );
    assert_ne!(first_attempt.raw_transaction_digest(), &[0_u8; 32]);

    assert!(matches!(
        adapter
            .submit_claim(&second.agreement, &serialize(&second.claim), &store)
            .await,
        Err(CoreAdapterError::Store(StoreError))
    ));
    assert_eq!(
        rpc.calls()
            .iter()
            .filter(|call| **call == "sendrawtransaction")
            .count(),
        1
    );
}

#[tokio::test]
async fn caller_authorized_claim_submission_returns_only_chain_outcomes() {
    let fixture = swap_fixture();
    let claim_bytes = serialize(&fixture.claim);

    let accepted_rpc = MockRpc::ready();
    accepted_rpc.push_mempool(Ok(mempool_allowed(&fixture.claim)));
    accepted_rpc.push_send(Ok(SendRawTransaction(
        fixture.claim.compute_txid().to_string(),
    )));
    accepted_rpc.push_spender(spender(
        fixture.agreement.cooperative_claim().funding_outpoint(),
        &fixture.claim,
    ));
    assert_eq!(
        isolated_adapter(accepted_rpc.clone())
            .submit_authorized_claim(
                &fixture.agreement,
                &claim_bytes,
                fixture.claim.compute_txid(),
            )
            .await
            .expect("authorized claim accepted"),
        AuthorizedClaimSubmission::Accepted {
            transaction_id: fixture.claim.compute_txid(),
        }
    );
    assert_eq!(
        accepted_rpc
            .calls()
            .iter()
            .filter(|call| **call == "sendrawtransaction")
            .count(),
        1
    );

    let rejected_rpc = MockRpc::ready();
    rejected_rpc.push_mempool(Ok(mempool_allowed(&fixture.claim)));
    rejected_rpc.push_send(Err(MockError::Rejected));
    assert_eq!(
        isolated_adapter(rejected_rpc.clone())
            .submit_authorized_claim(
                &fixture.agreement,
                &claim_bytes,
                fixture.claim.compute_txid(),
            )
            .await
            .expect("authorized claim definitively rejected"),
        AuthorizedClaimSubmission::Rejected
    );
    assert_eq!(
        rejected_rpc
            .calls()
            .iter()
            .filter(|call| **call == "sendrawtransaction")
            .count(),
        1
    );

    let unknown_rpc = MockRpc::ready();
    unknown_rpc.push_mempool(Ok(mempool_allowed(&fixture.claim)));
    unknown_rpc.push_send(Err(MockError::Transport));
    assert_eq!(
        isolated_adapter(unknown_rpc.clone())
            .submit_authorized_claim(
                &fixture.agreement,
                &claim_bytes,
                fixture.claim.compute_txid(),
            )
            .await
            .expect("authorized claim outcome unknown"),
        AuthorizedClaimSubmission::Unknown
    );
    assert_eq!(
        unknown_rpc
            .calls()
            .iter()
            .filter(|call| **call == "sendrawtransaction")
            .count(),
        1
    );

    let preflight_unknown_rpc = MockRpc::ready();
    preflight_unknown_rpc.push_mempool(Err(MockError::Transport));
    assert_eq!(
        isolated_adapter(preflight_unknown_rpc.clone())
            .submit_authorized_claim(
                &fixture.agreement,
                &claim_bytes,
                fixture.claim.compute_txid(),
            )
            .await
            .expect("authorized preflight outcome unknown"),
        AuthorizedClaimSubmission::Unknown
    );
    assert!(
        !preflight_unknown_rpc
            .calls()
            .contains(&"sendrawtransaction")
    );
}

#[tokio::test]
async fn claim_send_success_with_another_witness_is_unknown() {
    let first = swap_fixture();
    let second = swap_fixture();
    assert_eq!(first.claim.compute_txid(), second.claim.compute_txid());
    assert_ne!(first.claim.compute_wtxid(), second.claim.compute_wtxid());
    let rpc = MockRpc::ready();
    rpc.push_mempool(Ok(mempool_allowed(&first.claim)));
    rpc.push_send(Ok(SendRawTransaction(
        first.claim.compute_txid().to_string(),
    )));
    rpc.push_spender(spender(
        first.agreement.cooperative_claim().funding_outpoint(),
        &second.claim,
    ));
    assert_eq!(
        isolated_adapter(rpc.clone())
            .submit_authorized_claim(
                &first.agreement,
                &serialize(&first.claim),
                first.claim.compute_txid(),
            )
            .await
            .expect("broadcast race is conservative"),
        AuthorizedClaimSubmission::Unknown
    );
    assert_eq!(
        rpc.calls()
            .iter()
            .filter(|call| **call == "sendrawtransaction")
            .count(),
        1
    );
}

#[tokio::test]
async fn caller_authorized_claim_validates_identity_and_witness_before_send() {
    let fixture = swap_fixture();
    let claim_bytes = serialize(&fixture.claim);

    let wrong_identity_rpc = MockRpc::ready();
    assert!(matches!(
        isolated_adapter(wrong_identity_rpc.clone())
            .submit_authorized_claim(
                &fixture.agreement,
                &claim_bytes,
                fixture.funding.compute_txid(),
            )
            .await,
        Err(CoreAdapterError::ClaimTransactionMismatch)
    ));
    assert!(wrong_identity_rpc.calls().is_empty());

    let malformed_bytes_rpc = MockRpc::ready();
    let mut malformed_bytes = claim_bytes.clone();
    malformed_bytes.push(0);
    assert!(matches!(
        isolated_adapter(malformed_bytes_rpc.clone())
            .submit_authorized_claim(
                &fixture.agreement,
                &malformed_bytes,
                fixture.claim.compute_txid(),
            )
            .await,
        Err(CoreAdapterError::MalformedRawTransaction)
    ));
    assert!(malformed_bytes_rpc.calls().is_empty());

    let wrong_claim_rpc = MockRpc::ready();
    let mut wrong_claim = fixture.claim.clone();
    wrong_claim.input[0].witness = bitcoin::Witness::from_slice(&[[0x55; 64]]);
    assert!(matches!(
        isolated_adapter(wrong_claim_rpc.clone())
            .submit_authorized_claim(
                &fixture.agreement,
                &serialize(&wrong_claim),
                wrong_claim.compute_txid(),
            )
            .await,
        Err(CoreAdapterError::ClaimTransactionMismatch)
    ));
    assert!(wrong_claim_rpc.calls().is_empty());

    let conflicting_witness_rpc = MockRpc::ready();
    let mut conflicting_witness = mempool_allowed(&fixture.claim);
    conflicting_witness.0[0].wtxid = swap_fixture().claim.compute_wtxid().to_string();
    assert_ne!(
        conflicting_witness.0[0].wtxid,
        fixture.claim.compute_wtxid().to_string()
    );
    conflicting_witness_rpc.push_mempool(Ok(conflicting_witness));
    assert!(matches!(
        isolated_adapter(conflicting_witness_rpc.clone())
            .submit_authorized_claim(
                &fixture.agreement,
                &claim_bytes,
                fixture.claim.compute_txid(),
            )
            .await,
        Err(CoreAdapterError::MempoolResponseMismatch)
    ));
    assert!(
        !conflicting_witness_rpc
            .calls()
            .contains(&"sendrawtransaction")
    );

    let already_known_rpc = MockRpc::ready();
    already_known_rpc.push_mempool(Ok(mempool_rejected(
        &fixture.claim,
        "txn-same-nonwitness-data-in-mempool",
    )));
    assert_eq!(
        isolated_adapter(already_known_rpc.clone())
            .submit_authorized_claim(
                &fixture.agreement,
                &claim_bytes,
                fixture.claim.compute_txid(),
            )
            .await
            .expect("already-known result remains conservative"),
        AuthorizedClaimSubmission::Unknown
    );
    assert!(!already_known_rpc.calls().contains(&"sendrawtransaction"));
}
