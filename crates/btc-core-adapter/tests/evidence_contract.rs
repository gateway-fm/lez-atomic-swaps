mod support;

use std::collections::{BTreeMap, VecDeque};
use std::fmt;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use bitcoin::consensus::serialize;
use bitcoin::{BlockHash, OutPoint, Txid};
use corepc_types::v31::{
    GetBlockHash, GetBlockHeaderVerbose, GetBlockchainInfo, GetIndexInfo, GetIndexInfoName,
    GetNetworkInfo, GetRawTransactionVerbose, GetTxSpendingPrevout, GetTxSpendingPrevoutItem,
    SendRawTransaction, TestMempoolAccept,
};
use lez_btc_core_adapter::{
    BitcoinCoreAdapter, BitcoinCoreEvidenceError, BitcoinCoreEvidenceKind, BitcoinCoreEvidenceV1,
    BitcoinCoreRpc, ClaimObservation, CoreConnectivityPolicy, FundingObservation,
    MAX_BITCOIN_CORE_EVIDENCE_BYTES, RefundObservation, SendFailure,
};
use serde_json::{Value, json};

use support::{REGTEST_GENESIS, REQUIRED_CONFIRMATIONS, raw_verbose, swap_fixture};

const TIP: &str = "6f8c2a4d807e31d3f650d7228af87f9e75bfac506bdf9c7730483cf1524e7ac4";

#[derive(Clone, Copy, Debug)]
struct MockError;

impl fmt::Display for MockError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("mock RPC failure")
    }
}

impl std::error::Error for MockError {}

#[derive(Clone)]
struct MockRpc {
    inner: Arc<Mutex<MockResponses>>,
}

struct MockResponses {
    height: u32,
    chains: VecDeque<GetBlockchainInfo>,
    raw: VecDeque<Option<GetRawTransactionVerbose>>,
    headers: VecDeque<GetBlockHeaderVerbose>,
    spender: VecDeque<GetTxSpendingPrevout>,
}

impl MockRpc {
    fn with_raw(raw: GetRawTransactionVerbose) -> Self {
        Self {
            inner: Arc::new(Mutex::new(MockResponses {
                height: 200,
                chains: VecDeque::from([ready_chain(), ready_chain()]),
                raw: VecDeque::from([Some(raw)]),
                headers: VecDeque::from([ready_funding_header()]),
                spender: VecDeque::new(),
            })),
        }
    }

    fn with_claim(raw: GetRawTransactionVerbose, spender: GetTxSpendingPrevout) -> Self {
        Self {
            inner: Arc::new(Mutex::new(MockResponses {
                height: 200,
                chains: VecDeque::from([ready_chain(), ready_chain()]),
                raw: VecDeque::from([Some(raw)]),
                headers: VecDeque::new(),
                spender: VecDeque::from([spender]),
            })),
        }
    }

    fn with_refund(
        funding: GetRawTransactionVerbose,
        refund: GetRawTransactionVerbose,
        spender: GetTxSpendingPrevout,
    ) -> Self {
        let height = 1_149;
        Self {
            inner: Arc::new(Mutex::new(MockResponses {
                height,
                chains: VecDeque::from([ready_chain_at(height), ready_chain_at(height)]),
                raw: VecDeque::from([Some(funding), Some(refund)]),
                headers: VecDeque::new(),
                spender: VecDeque::from([spender]),
            })),
        }
    }
}

#[async_trait]
impl BitcoinCoreRpc for MockRpc {
    type Error = MockError;

    async fn get_network_info(&self) -> Result<GetNetworkInfo, Self::Error> {
        Ok(serde_json::from_value(json!({
            "version": 310_100,
            "subversion": "/Satoshi:31.1.0/",
            "protocolversion": 70016,
            "localservices": "0000000000000409",
            "localservicesnames": [],
            "localrelay": true,
            "timeoffset": 0,
            "connections": 0,
            "connections_in": 0,
            "connections_out": 0,
            "networkactive": false,
            "networks": [],
            "relayfee": 0.00001,
            "incrementalfee": 0.00001,
            "localaddresses": [],
            "warnings": []
        }))
        .expect("network response"))
    }

    async fn get_blockchain_info(&self) -> Result<GetBlockchainInfo, Self::Error> {
        self.inner
            .lock()
            .expect("mock lock")
            .chains
            .pop_front()
            .ok_or(MockError)
    }

    async fn get_genesis_hash(&self) -> Result<GetBlockHash, Self::Error> {
        Ok(GetBlockHash(REGTEST_GENESIS.to_owned()))
    }

    async fn get_index_info(&self) -> Result<GetIndexInfo, Self::Error> {
        let height = self.inner.lock().expect("mock lock").height;
        Ok(GetIndexInfo(BTreeMap::from([
            (
                "txindex".to_owned(),
                GetIndexInfoName {
                    synced: true,
                    best_block_height: height,
                },
            ),
            (
                "txospenderindex".to_owned(),
                GetIndexInfoName {
                    synced: true,
                    best_block_height: height,
                },
            ),
        ])))
    }

    async fn get_raw_transaction(
        &self,
        _transaction_id: Txid,
    ) -> Result<Option<GetRawTransactionVerbose>, Self::Error> {
        self.inner
            .lock()
            .expect("mock lock")
            .raw
            .pop_front()
            .ok_or(MockError)
    }

    async fn get_block_header(
        &self,
        _block_hash: BlockHash,
    ) -> Result<GetBlockHeaderVerbose, Self::Error> {
        self.inner
            .lock()
            .expect("mock lock")
            .headers
            .pop_front()
            .ok_or(MockError)
    }

    async fn get_tx_spending_prevout(
        &self,
        _outpoint: OutPoint,
    ) -> Result<GetTxSpendingPrevout, Self::Error> {
        self.inner
            .lock()
            .expect("mock lock")
            .spender
            .pop_front()
            .ok_or(MockError)
    }

    async fn test_mempool_accept(
        &self,
        _transaction: &[u8],
    ) -> Result<TestMempoolAccept, Self::Error> {
        Err(MockError)
    }

    async fn send_raw_transaction(
        &self,
        _transaction: &[u8],
    ) -> Result<SendRawTransaction, Self::Error> {
        Err(MockError)
    }

    fn classify_send_failure(_error: &Self::Error) -> SendFailure {
        SendFailure::Unknown
    }
}

fn ready_chain() -> GetBlockchainInfo {
    ready_chain_at(200)
}

fn ready_chain_at(height: u32) -> GetBlockchainInfo {
    serde_json::from_value(json!({
        "chain": "regtest",
        "blocks": height,
        "headers": height,
        "bestblockhash": TIP,
        "bits": "207fffff",
        "target": "7fffff0000000000000000000000000000000000000000000000000000000000",
        "difficulty": 4.656_542_373_906_925e-10,
        "time": 1_700_000_000,
        "mediantime": 1_699_999_000,
        "verificationprogress": 1.0,
        "initialblockdownload": false,
        "chainwork": "0000000000000000000000000000000000000000000000000000000000000192",
        "size_on_disk": 4096,
        "pruned": false,
        "warnings": []
    }))
    .expect("chain response")
}

fn ready_funding_header() -> GetBlockHeaderVerbose {
    serde_json::from_value(json!({
        "hash": TIP,
        "confirmations": REQUIRED_CONFIRMATIONS,
        "height": 195,
        "version": 1,
        "versionHex": "00000001",
        "merkleroot": TIP,
        "time": 1_699_999_000,
        "mediantime": 1_699_998_900,
        "nonce": 0,
        "bits": "207fffff",
        "target": "7fffff0000000000000000000000000000000000000000000000000000000000",
        "difficulty": 4.656_542_373_906_925e-10,
        "chainwork": "0000000000000000000000000000000000000000000000000000000000000192",
        "nTx": 1,
        "previousblockhash": TIP,
        "nextblockhash": TIP
    }))
    .expect("block header response")
}

fn spender(
    outpoint: OutPoint,
    transaction: &bitcoin::Transaction,
    block_hash: Option<&str>,
) -> GetTxSpendingPrevout {
    GetTxSpendingPrevout(vec![GetTxSpendingPrevoutItem {
        txid: outpoint.txid.to_string(),
        vout: outpoint.vout,
        spending_txid: Some(transaction.compute_txid().to_string()),
        spending_tx: Some(hex::encode(serialize(transaction))),
        block_hash: block_hash.map(str::to_owned),
    }])
}

async fn observed_claim(confirmations: u32) -> (support::SwapFixture, ClaimObservation) {
    let fixture = swap_fixture();
    let block_hash = (confirmations > 0).then_some(TIP);
    let rpc = MockRpc::with_claim(
        raw_verbose(
            &fixture.claim,
            (confirmations > 0).then_some(u64::from(confirmations)),
            block_hash,
        ),
        spender(
            fixture.agreement.cooperative_claim().funding_outpoint(),
            &fixture.claim,
            block_hash,
        ),
    );
    let observation = BitcoinCoreAdapter::new(rpc, CoreConnectivityPolicy::IsolatedLocal)
        .observe_claim(&fixture.agreement)
        .await
        .expect("adapter-validated claim");
    (fixture, observation)
}

async fn observed_refund() -> (support::SwapFixture, RefundObservation) {
    let fixture = swap_fixture();
    let rpc = MockRpc::with_refund(
        raw_verbose(&fixture.funding, Some(150), Some(TIP)),
        raw_verbose(&fixture.refund, Some(6), Some(TIP)),
        spender(
            fixture.agreement.bitcoin_refund().funding_outpoint(),
            &fixture.refund,
            Some(TIP),
        ),
    );
    let observation = BitcoinCoreAdapter::new(rpc, CoreConnectivityPolicy::IsolatedLocal)
        .observe_refund(&fixture.agreement)
        .await
        .expect("adapter-validated refund");
    (fixture, observation)
}

#[tokio::test]
async fn finalized_refund_evidence_preserves_containing_height_and_exact_witness() {
    let (fixture, observation) = observed_refund().await;
    let RefundObservation::Finalized(refund) = &observation else {
        panic!("expected finalized refund");
    };
    assert_eq!(refund.block_height(), Some(1_144));
    let evidence = BitcoinCoreEvidenceV1::refund_finalized(&fixture.agreement, &observation)
        .expect("refund evidence");
    assert_eq!(evidence.kind(), BitcoinCoreEvidenceKind::RefundFinalized);
    assert_eq!(evidence.transaction(), &fixture.refund);
    assert_eq!(evidence.confirmed_block_height(), Some(1_144));
    assert_eq!(evidence.claim_public_witness(), None);
    let encoded = evidence.encode().expect("canonical refund evidence");
    let decoded = BitcoinCoreEvidenceV1::decode(&fixture.agreement, &encoded)
        .expect("refund evidence decodes");
    assert_eq!(decoded, evidence);
    assert_eq!(decoded.confirmed_block_height(), Some(1_144));
}

#[tokio::test]
async fn funding_ready_evidence_is_canonical_bounded_and_agreement_bound() {
    let fixture = swap_fixture();
    let rpc = MockRpc::with_raw(raw_verbose(
        &fixture.funding,
        Some(u64::from(REQUIRED_CONFIRMATIONS)),
        Some(TIP),
    ));
    let FundingObservation::Ready(observed) =
        BitcoinCoreAdapter::new(rpc, CoreConnectivityPolicy::IsolatedLocal)
            .observe_funding(&fixture.agreement)
            .await
            .expect("adapter-validated funding")
    else {
        panic!("expected ready funding");
    };

    let evidence = BitcoinCoreEvidenceV1::funding_ready(&fixture.agreement, &observed)
        .expect("funding evidence");
    assert_eq!(evidence.kind(), BitcoinCoreEvidenceKind::FundingReady);
    assert_eq!(
        evidence.agreement_commitment(),
        fixture.agreement.agreement_commitment()
    );
    assert_eq!(evidence.transaction(), &fixture.funding);
    assert_eq!(evidence.transaction_id(), fixture.funding.compute_txid());
    assert_eq!(
        evidence.witness_transaction_id(),
        fixture.funding.compute_wtxid()
    );
    assert_eq!(evidence.confirmations(), REQUIRED_CONFIRMATIONS);
    assert_eq!(
        evidence
            .block_hash()
            .map(|hash| hash.to_string())
            .as_deref(),
        Some(TIP)
    );
    assert_eq!(evidence.stable_tip().height(), 200);
    assert_eq!(evidence.claim_public_witness(), None);

    let encoded = evidence.encode().expect("canonical evidence");
    assert!(encoded.len() <= MAX_BITCOIN_CORE_EVIDENCE_BYTES);
    let decoded = BitcoinCoreEvidenceV1::decode(&fixture.agreement, &encoded)
        .expect("canonical evidence decodes");
    assert_eq!(decoded, evidence);
    assert_eq!(decoded.encode().expect("re-encode"), encoded);

    let mut unknown: Value = serde_json::from_slice(&encoded).expect("evidence JSON");
    unknown["unknown"] = json!(true);
    let unknown = serde_json::to_vec(&unknown).expect("mutated JSON");
    assert!(matches!(
        BitcoinCoreEvidenceV1::decode(&fixture.agreement, &unknown),
        Err(BitcoinCoreEvidenceError::Malformed)
    ));

    let mut wrong_commitment: Value = serde_json::from_slice(&encoded).expect("evidence JSON");
    wrong_commitment["agreement_commitment"] = json!("00".repeat(32));
    let wrong_commitment = serde_json::to_vec(&wrong_commitment).expect("mutated JSON");
    assert!(matches!(
        BitcoinCoreEvidenceV1::decode(&fixture.agreement, &wrong_commitment),
        Err(BitcoinCoreEvidenceError::AgreementMismatch)
    ));

    let mut wrong_id: Value = serde_json::from_slice(&encoded).expect("evidence JSON");
    wrong_id["transaction"]["transaction_id"] = json!("00".repeat(32));
    let wrong_id = serde_json::to_vec(&wrong_id).expect("mutated JSON");
    assert!(matches!(
        BitcoinCoreEvidenceV1::decode(&fixture.agreement, &wrong_id),
        Err(BitcoinCoreEvidenceError::TransactionMismatch)
    ));

    let mut cross_kind: Value = serde_json::from_slice(&encoded).expect("evidence JSON");
    cross_kind["kind"] = json!("claim_finalized");
    let cross_kind = serde_json::to_vec(&cross_kind).expect("mutated JSON");
    assert!(BitcoinCoreEvidenceV1::decode(&fixture.agreement, &cross_kind).is_err());
}

#[tokio::test]
async fn claim_evidence_preserves_exact_public_witness_and_state_classification() {
    for (confirmations, expected_kind) in [
        (0, BitcoinCoreEvidenceKind::ClaimRevealed),
        (1, BitcoinCoreEvidenceKind::ClaimConfirming),
        (
            REQUIRED_CONFIRMATIONS,
            BitcoinCoreEvidenceKind::ClaimFinalized,
        ),
    ] {
        let (fixture, observation) = observed_claim(confirmations).await;
        let evidence =
            BitcoinCoreEvidenceV1::claim(&fixture.agreement, &observation).expect("claim evidence");
        assert_eq!(evidence.kind(), expected_kind);
        let expected_witness: [u8; 64] = fixture.claim.input[0]
            .witness
            .iter()
            .next()
            .expect("claim witness")
            .try_into()
            .expect("64-byte claim witness");
        assert_eq!(evidence.claim_public_witness(), Some(&expected_witness));
        assert_eq!(evidence.confirmations(), confirmations);
        assert_eq!(evidence.block_hash().is_some(), confirmations > 0);

        let encoded = evidence.encode().expect("canonical claim evidence");
        let decoded = BitcoinCoreEvidenceV1::decode(&fixture.agreement, &encoded)
            .expect("claim evidence decodes");
        assert_eq!(decoded, evidence);
        assert_eq!(decoded.claim_public_witness(), Some(&expected_witness));
    }

    let fixture = swap_fixture();
    assert!(matches!(
        BitcoinCoreEvidenceV1::claim(&fixture.agreement, &ClaimObservation::Unspent),
        Err(BitcoinCoreEvidenceError::UnsupportedObservation)
    ));
}

#[tokio::test]
async fn decoding_rejects_noncanonical_malformed_oversize_and_mutated_claims() {
    let (fixture, observation) = observed_claim(1).await;
    let evidence =
        BitcoinCoreEvidenceV1::claim(&fixture.agreement, &observation).expect("claim evidence");
    let encoded = evidence.encode().expect("canonical claim evidence");

    let mut padded = b" ".to_vec();
    padded.extend_from_slice(&encoded);
    assert!(matches!(
        BitcoinCoreEvidenceV1::decode(&fixture.agreement, &padded),
        Err(BitcoinCoreEvidenceError::Noncanonical)
    ));
    assert!(matches!(
        BitcoinCoreEvidenceV1::decode(&fixture.agreement, b"{"),
        Err(BitcoinCoreEvidenceError::Malformed)
    ));
    assert!(matches!(
        BitcoinCoreEvidenceV1::decode(
            &fixture.agreement,
            &vec![b'x'; MAX_BITCOIN_CORE_EVIDENCE_BYTES + 1],
        ),
        Err(BitcoinCoreEvidenceError::Oversized { .. })
    ));

    let mut wrong_witness: Value = serde_json::from_slice(&encoded).expect("evidence JSON");
    wrong_witness["public_claim_witness"] = json!("00".repeat(64));
    let wrong_witness = serde_json::to_vec(&wrong_witness).expect("mutated JSON");
    assert!(matches!(
        BitcoinCoreEvidenceV1::decode(&fixture.agreement, &wrong_witness),
        Err(BitcoinCoreEvidenceError::TransactionMismatch)
    ));

    let mut wrong_raw: Value = serde_json::from_slice(&encoded).expect("evidence JSON");
    wrong_raw["transaction"]["consensus_hex"] = json!("00");
    let wrong_raw = serde_json::to_vec(&wrong_raw).expect("mutated JSON");
    assert!(matches!(
        BitcoinCoreEvidenceV1::decode(&fixture.agreement, &wrong_raw),
        Err(BitcoinCoreEvidenceError::TransactionMismatch)
    ));

    let mut wrong_state: Value = serde_json::from_slice(&encoded).expect("evidence JSON");
    wrong_state["kind"] = json!("claim_finalized");
    let wrong_state = serde_json::to_vec(&wrong_state).expect("mutated JSON");
    assert!(matches!(
        BitcoinCoreEvidenceV1::decode(&fixture.agreement, &wrong_state),
        Err(BitcoinCoreEvidenceError::ObservationStateMismatch)
    ));

    let mut uppercase: Value = serde_json::from_slice(&encoded).expect("evidence JSON");
    uppercase["transaction"]["consensus_hex"] = json!(hex::encode_upper(serialize(&fixture.claim)));
    let uppercase = serde_json::to_vec(&uppercase).expect("mutated JSON");
    assert!(BitcoinCoreEvidenceV1::decode(&fixture.agreement, &uppercase).is_err());
}
