use std::{
    collections::{BTreeMap, VecDeque},
    net::{Ipv4Addr, SocketAddr},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};

use async_trait::async_trait;
use borsh::{BorshDeserialize as _, to_vec};
use indexer_service_protocol::{
    Account as IndexedAccount, AccountId as IndexedAccountId, BedrockStatus, Block, BlockBody,
    BlockHeader, Data as IndexedData, HashType, ProgramId as IndexedProgramId,
    PublicKey as IndexedPublicKey, PublicMessage as IndexedPublicMessage,
    PublicTransaction as IndexedPublicTransaction, Signature as IndexedSignature, Transaction,
    WitnessSet as IndexedWitnessSet,
};
use jsonrpsee::{RpcModule, server::ServerBuilder, types::ErrorObjectOwned};
use lez_bridge_client::{BridgeClient, BridgeClientConfig, SidecarCapability};
use lez_bridge_protocol::{
    DiscoveryWindow, FinalizedWitnessedFundingObservationTarget, Hex32, MessageContext,
    ObserveFinalizedWitnessedFundingRequest, Participant, RequestId, RunId, RuntimeCompatibility,
    RuntimeDescriptor, TransactionId, WitnessedNativeEscrowTerms, WitnessedNativeEscrowTermsInput,
};
use lez_v0_2_sidecar::{
    BridgeRuntime, BridgeRuntimeError, BridgeServerCapability, BridgeServerConfig,
    FinalizedIndexerApi, FinalizedWitnessedFundingObserver, NativeEscrowPlanner,
    NativePrepareError, NonceSource, OfficialNodeRpc, ZecEscrowInstruction, compute_custody_pda,
    compute_metadata_pda, program_id_from_hex, start_bridge_server,
};
use lez_zec_escrow_v02::{ClaimAuthority, EscrowMetadata, EscrowStatus};
use nssa::{
    AccountId, PrivateKey, PublicKey, PublicTransaction,
    public_transaction::{Message, WitnessSet},
};

const BRIDGE_CAPABILITY: &str = "finalized-funding-capability-0001";
const FUNDING_BLOCK_ID: u64 = 10;
const FINALIZED_TIP_ID: u64 = 11;

#[derive(Debug)]
struct FixedNonce;

#[async_trait]
impl NonceSource for FixedNonce {
    async fn account_nonce(&self, _account_id: AccountId) -> Result<u128, NativePrepareError> {
        Ok(0)
    }
}

#[derive(Debug)]
struct MockIndexer {
    tips: Mutex<VecDeque<Option<u64>>>,
    by_id: BTreeMap<u64, Block>,
    by_hash: BTreeMap<[u8; 32], Block>,
    accounts: BTreeMap<([u8; 32], u64), IndexedAccount>,
    calls: Mutex<Vec<String>>,
}

#[async_trait]
impl FinalizedIndexerApi for MockIndexer {
    async fn last_finalized_block_id(&self) -> Result<Option<u64>, BridgeRuntimeError> {
        self.calls.lock().unwrap().push("tip".to_owned());
        let mut tips = self.tips.lock().unwrap();
        Ok(if tips.len() > 1 {
            tips.pop_front().unwrap()
        } else {
            tips.front().copied().flatten()
        })
    }

    async fn block_by_id(&self, block_id: u64) -> Result<Option<Block>, BridgeRuntimeError> {
        self.calls.lock().unwrap().push(format!("id:{block_id}"));
        Ok(self.by_id.get(&block_id).cloned())
    }

    async fn block_by_hash(
        &self,
        block_hash: [u8; 32],
    ) -> Result<Option<Block>, BridgeRuntimeError> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("hash:{}", hex::encode(block_hash)));
        Ok(self.by_hash.get(&block_hash).cloned())
    }

    async fn account_at_block(
        &self,
        account_id: [u8; 32],
        block_id: u64,
    ) -> Result<IndexedAccount, BridgeRuntimeError> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("account:{}:{block_id}", hex::encode(account_id)));
        self.accounts
            .get(&(account_id, block_id))
            .cloned()
            .ok_or(BridgeRuntimeError::Unavailable)
    }
}

#[derive(Debug)]
struct SameHeightMovingTipIndexer {
    base: Arc<MockIndexer>,
    changed_tip: Block,
    tip_id_reads: AtomicUsize,
    serving_changed_tip: AtomicBool,
}

#[async_trait]
impl FinalizedIndexerApi for SameHeightMovingTipIndexer {
    async fn last_finalized_block_id(&self) -> Result<Option<u64>, BridgeRuntimeError> {
        self.base.last_finalized_block_id().await
    }

    async fn block_by_id(&self, block_id: u64) -> Result<Option<Block>, BridgeRuntimeError> {
        if block_id == self.changed_tip.header.block_id
            && self.tip_id_reads.fetch_add(1, Ordering::SeqCst) >= 2
        {
            self.serving_changed_tip.store(true, Ordering::SeqCst);
            self.base
                .calls
                .lock()
                .unwrap()
                .push(format!("id:{block_id}"));
            return Ok(Some(self.changed_tip.clone()));
        }
        self.base.block_by_id(block_id).await
    }

    async fn block_by_hash(
        &self,
        block_hash: [u8; 32],
    ) -> Result<Option<Block>, BridgeRuntimeError> {
        if self.serving_changed_tip.load(Ordering::SeqCst)
            && block_hash == self.changed_tip.header.hash.0
        {
            self.base
                .calls
                .lock()
                .unwrap()
                .push(format!("hash:{}", hex::encode(block_hash)));
            return Ok(Some(self.changed_tip.clone()));
        }
        self.base.block_by_hash(block_hash).await
    }

    async fn account_at_block(
        &self,
        account_id: [u8; 32],
        block_id: u64,
    ) -> Result<IndexedAccount, BridgeRuntimeError> {
        self.base.account_at_block(account_id, block_id).await
    }
}

struct Fixture {
    runtime: RuntimeDescriptor,
    request: ObserveFinalizedWitnessedFundingRequest,
    depositor_private: PrivateKey,
    blocks: Vec<Block>,
    accounts: BTreeMap<([u8; 32], u64), IndexedAccount>,
}

fn fixture() -> Fixture {
    let aggregate_private = PrivateKey::try_new([5; 32]).unwrap();
    let aggregate_public = PublicKey::new_from_private_key(&aggregate_private);
    let aggregate_authority = AccountId::from(&aggregate_public);
    let depositor_private = PrivateKey::try_new([6; 32]).unwrap();
    let depositor_public = PublicKey::new_from_private_key(&depositor_private);
    let depositor = AccountId::from(&depositor_public);
    let claimant_private = PrivateKey::try_new([7; 32]).unwrap();
    let claimant = AccountId::from(&PublicKey::new_from_private_key(&claimant_private));
    let escrow_program = h(4);
    let swap_id = h(40);
    let metadata = compute_metadata_pda(&program_id_from_hex(escrow_program), swap_id.as_bytes());
    let custody = compute_custody_pda(&program_id_from_hex(escrow_program), swap_id.as_bytes());
    let message = Message::try_new(
        program_id_from_hex(escrow_program),
        vec![metadata, custody, depositor],
        vec![9_u128.into()],
        ZecEscrowInstruction::FundNative {
            swap_id: *swap_id.as_bytes(),
        },
    )
    .unwrap();
    let signature = nssa::Signature::new(&depositor_private, &message.hash());
    let public = PublicTransaction::new(
        message,
        WitnessSet::from_raw_parts(vec![(signature, depositor_public)]),
    );
    let terms = WitnessedNativeEscrowTerms::new(WitnessedNativeEscrowTermsInput {
        swap_id,
        terms_hash: h(41),
        depositor: Participant::Maker,
        depositor_account_id: Hex32::from_bytes(depositor.into_value()),
        claimant: Participant::Taker,
        claimant_account_id: Hex32::from_bytes(claimant.into_value()),
        aggregate_authority_account_id: Hex32::from_bytes(aggregate_authority.into_value()),
        aggregate_x_only_public_key: Hex32::from_bytes(*aggregate_public.value()),
        amount: 75,
        refund_at_ms: 1_850_000_000_123,
        authenticated_transfer_program_id: h(46),
    })
    .unwrap();
    let runtime = RuntimeDescriptor::new(
        Participant::Maker,
        RuntimeCompatibility::LeeV0_2_0,
        h(1),
        h(2),
        h(3),
        escrow_program,
        terms.depositor_account_id(),
    );
    let request = ObserveFinalizedWitnessedFundingRequest::new(
        MessageContext::new(
            RunId::new("finalized-funding-run-0001").unwrap(),
            RequestId::new("finalized-funding-observe-0001").unwrap(),
            Participant::Maker,
        ),
        runtime.clone(),
        terms,
        TransactionId::from_bytes(public.hash()),
        DiscoveryWindow::new(FUNDING_BLOCK_ID, 2).unwrap(),
    );
    let blocks = vec![
        block(
            FUNDING_BLOCK_ID,
            [10; 32],
            vec![Transaction::Public(indexed_public(&public))],
        ),
        block(FINALIZED_TIP_ID, [11; 32], Vec::new()),
    ];
    let accounts = funded_accounts(&request);
    Fixture {
        runtime,
        request,
        depositor_private,
        blocks,
        accounts,
    }
}

fn funded_accounts(
    request: &ObserveFinalizedWitnessedFundingRequest,
) -> BTreeMap<([u8; 32], u64), IndexedAccount> {
    let escrow_program = program_id_from_hex(request.runtime.escrow_program_id);
    let transfer_program = program_id_from_hex(request.terms.authenticated_transfer_program_id());
    let metadata_id = compute_metadata_pda(&escrow_program, request.terms.swap_id().as_bytes());
    let custody_id = compute_custody_pda(&escrow_program, request.terms.swap_id().as_bytes());
    let metadata = EscrowMetadata {
        version: 2,
        swap_id: *request.terms.swap_id().as_bytes(),
        terms_hash: *request.terms.terms_hash().as_bytes(),
        claim_authority: ClaimAuthority::AggregateWitness {
            x_only_public_key: *request.terms.aggregate_x_only_public_key().as_bytes(),
            account_id: AccountId::new(*request.terms.aggregate_authority_account_id().as_bytes()),
        },
        depositor: AccountId::new(*request.terms.depositor_account_id().as_bytes()),
        depositor_asset: AccountId::new(*request.terms.depositor_account_id().as_bytes()),
        claimant: AccountId::new(*request.terms.claimant_account_id().as_bytes()),
        claimant_asset: AccountId::new(*request.terms.claimant_account_id().as_bytes()),
        custody: custody_id,
        asset_program: transfer_program,
        custody_program: transfer_program,
        asset_definition: [0; 32],
        amount: request.terms.amount().as_u128(),
        refund_at: request.terms.refund_at_ms(),
        status: EscrowStatus::Funded,
    };
    BTreeMap::from([
        (
            (metadata_id.into_value(), FUNDING_BLOCK_ID),
            IndexedAccount {
                program_owner: IndexedProgramId(escrow_program),
                balance: 0,
                data: IndexedData(to_vec(&metadata).unwrap()),
                nonce: 0,
            },
        ),
        (
            (custody_id.into_value(), FUNDING_BLOCK_ID),
            IndexedAccount {
                program_owner: IndexedProgramId(transfer_program),
                balance: request.terms.amount().as_u128(),
                data: IndexedData(Vec::new()),
                nonce: 0,
            },
        ),
    ])
}

fn mutate_metadata(fixture: &mut Fixture, mutate: impl FnOnce(&mut EscrowMetadata)) {
    let escrow_program = program_id_from_hex(fixture.runtime.escrow_program_id);
    let metadata_id =
        compute_metadata_pda(&escrow_program, fixture.request.terms.swap_id().as_bytes());
    let account = fixture
        .accounts
        .get_mut(&(metadata_id.into_value(), FUNDING_BLOCK_ID))
        .unwrap();
    let mut metadata = EscrowMetadata::try_from_slice(&account.data.0).unwrap();
    mutate(&mut metadata);
    account.data = IndexedData(to_vec(&metadata).unwrap());
}

fn indexed_public(public: &PublicTransaction) -> IndexedPublicTransaction {
    IndexedPublicTransaction {
        hash: HashType(public.hash()),
        message: IndexedPublicMessage {
            program_id: IndexedProgramId(public.message().program_id),
            account_ids: public
                .message()
                .account_ids
                .iter()
                .map(|account| IndexedAccountId {
                    value: account.into_value(),
                })
                .collect(),
            nonces: public
                .message()
                .nonces
                .iter()
                .map(|nonce| u128::from(*nonce))
                .collect(),
            instruction_data: public.message().instruction_data.clone(),
        },
        witness_set: IndexedWitnessSet {
            signatures_and_public_keys: public
                .witness_set()
                .signatures_and_public_keys()
                .iter()
                .map(|(signature, key)| {
                    (
                        IndexedSignature(signature.value),
                        IndexedPublicKey(*key.value()),
                    )
                })
                .collect(),
            proof: None,
        },
    }
}

fn block(block_id: u64, hash: [u8; 32], transactions: Vec<Transaction>) -> Block {
    let block_byte = u8::try_from(block_id).expect("fixture block ID fits one byte");
    let previous_byte =
        u8::try_from(block_id.saturating_sub(1)).expect("fixture block ID fits one byte");
    Block {
        header: BlockHeader {
            block_id,
            prev_block_hash: HashType([previous_byte; 32]),
            hash: HashType(hash),
            timestamp: 1_850_000_000_000 + block_id,
            signature: IndexedSignature([block_byte; 64]),
        },
        body: BlockBody { transactions },
        bedrock_status: BedrockStatus::Finalized,
    }
}

fn indexer(fixture: &Fixture, tips: impl IntoIterator<Item = Option<u64>>) -> Arc<MockIndexer> {
    Arc::new(MockIndexer {
        tips: Mutex::new(tips.into_iter().collect()),
        by_id: fixture
            .blocks
            .iter()
            .map(|block| (block.header.block_id, block.clone()))
            .collect(),
        by_hash: fixture
            .blocks
            .iter()
            .map(|block| (block.header.hash.0, block.clone()))
            .collect(),
        accounts: fixture.accounts.clone(),
        calls: Mutex::new(Vec::new()),
    })
}

fn peerless_request(fixture: &Fixture) -> ObserveFinalizedWitnessedFundingRequest {
    ObserveFinalizedWitnessedFundingRequest::discover_by_terms(
        fixture.request.context.clone(),
        fixture.request.runtime.clone(),
        fixture.request.terms.clone(),
        fixture.request.window,
    )
}

fn assert_has_no_peer_transaction_id(request: &ObserveFinalizedWitnessedFundingRequest) {
    assert!(
        !serde_json::to_string(request)
            .unwrap()
            .contains("funding_transaction_id")
    );
}

fn replace_funding_message(fixture: &mut Fixture, mutate: impl FnOnce(&mut IndexedPublicMessage)) {
    let Transaction::Public(indexed) = &mut fixture.blocks[0].body.transactions[0] else {
        panic!("public fixture")
    };
    mutate(&mut indexed.message);
    let public_key = PublicKey::new_from_private_key(&fixture.depositor_private);
    let message = Message::new_preserialized(
        indexed.message.program_id.0,
        indexed
            .message
            .account_ids
            .iter()
            .map(|account| AccountId::new(account.value))
            .collect(),
        indexed
            .message
            .nonces
            .iter()
            .copied()
            .map(Into::into)
            .collect(),
        indexed.message.instruction_data.clone(),
    );
    let signature = nssa::Signature::new(&fixture.depositor_private, &message.hash());
    *indexed = indexed_public(&PublicTransaction::new(
        message,
        WitnessSet::from_raw_parts(vec![(signature, public_key)]),
    ));
}

fn retarget_exact_request(fixture: &mut Fixture) {
    let transaction_id = TransactionId::from_bytes(fixture.blocks[0].body.transactions[0].hash().0);
    fixture.request.target = FinalizedWitnessedFundingObservationTarget::Exact {
        funding_transaction_id: transaction_id,
    };
}

#[tokio::test]
async fn exact_funding_is_returned_only_after_dual_lookup_and_stable_finalized_tip() {
    let fixture = fixture();
    let indexer = indexer(&fixture, [Some(FINALIZED_TIP_ID), Some(FINALIZED_TIP_ID)]);
    let observer = FinalizedWitnessedFundingObserver::new(fixture.runtime, indexer.clone());

    let result = observer.observe(&fixture.request).await.unwrap();

    assert_eq!(
        result.funding.transaction.transaction_id,
        match fixture.request.target {
            FinalizedWitnessedFundingObservationTarget::Exact {
                funding_transaction_id,
            } => funding_transaction_id,
            FinalizedWitnessedFundingObservationTarget::DiscoverByTerms => panic!("exact fixture"),
        }
    );
    assert_eq!(
        result.funding.instruction.swap_id,
        fixture.request.terms.swap_id()
    );
    assert_eq!(result.funding.containing_block.block_id, FUNDING_BLOCK_ID);
    assert_eq!(
        result.funding.metadata.status,
        lez_bridge_protocol::EscrowState::Funded
    );
    assert_eq!(
        result.funding.custody.balance.as_u128(),
        fixture.request.terms.amount().as_u128()
    );
    assert_eq!(result.finalized_tip.height, FINALIZED_TIP_ID);
    assert_eq!(
        *indexer.calls.lock().unwrap(),
        vec![
            "tip".to_owned(),
            format!("id:{FINALIZED_TIP_ID}"),
            format!("hash:{}", hex::encode([11; 32])),
            format!("id:{FUNDING_BLOCK_ID}"),
            format!("hash:{}", hex::encode([10; 32])),
            format!("id:{FINALIZED_TIP_ID}"),
            format!("hash:{}", hex::encode([11; 32])),
            format!(
                "account:{}:{FUNDING_BLOCK_ID}",
                hex::encode(result.funding.metadata.account_id.as_bytes())
            ),
            format!(
                "account:{}:{FUNDING_BLOCK_ID}",
                hex::encode(result.funding.custody.account_id.as_bytes())
            ),
            "tip".to_owned(),
            format!("id:{FINALIZED_TIP_ID}"),
            format!("hash:{}", hex::encode([11; 32])),
        ]
    );
}

#[tokio::test]
async fn unique_funding_is_discovered_from_terms_without_peer_transaction_id() {
    let fixture = fixture();
    let request = peerless_request(&fixture);
    assert_has_no_peer_transaction_id(&request);
    let observer = FinalizedWitnessedFundingObserver::new(
        fixture.runtime.clone(),
        indexer(&fixture, [Some(FINALIZED_TIP_ID), Some(FINALIZED_TIP_ID)]),
    );

    let result = observer.observe(&request).await.unwrap();

    assert_eq!(result.funding.instruction.swap_id, request.terms.swap_id());
    assert_eq!(result.funding.containing_block.block_id, FUNDING_BLOCK_ID);
    assert_eq!(
        result.funding.custody.balance.as_u128(),
        request.terms.amount().as_u128()
    );
}

#[tokio::test]
async fn authenticated_bridge_server_observation_is_repeatable_and_never_submits() {
    let fixture = fixture();
    let expected_funding_transaction_id =
        TransactionId::from_bytes(fixture.blocks[0].body.transactions[0].hash().0);
    let request = peerless_request(&fixture);
    assert_has_no_peer_transaction_id(&request);
    let run_id = fixture.request.context.run_id.clone();
    let runtime_descriptor = fixture.runtime.clone();
    let indexer = indexer(&fixture, [Some(FINALIZED_TIP_ID)]);

    let submission_calls = Arc::new(AtomicUsize::new(0));
    let sequencer = ServerBuilder::default().build("127.0.0.1:0").await.unwrap();
    let sequencer_address = sequencer.local_addr().unwrap();
    let mut sequencer_rpc = RpcModule::new(Arc::clone(&submission_calls));
    sequencer_rpc
        .register_method("checkHealth", |_, _, _| Ok::<_, ErrorObjectOwned>(()))
        .unwrap();
    sequencer_rpc
        .register_method("getChannelId", |_, _, _| {
            Ok::<_, ErrorObjectOwned>(hex::encode([2_u8; 32]))
        })
        .unwrap();
    sequencer_rpc
        .register_method("sendTransaction", |_, calls, _| {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok::<_, ErrorObjectOwned>([0_u8; 32])
        })
        .unwrap();
    let sequencer_handle = sequencer.start(sequencer_rpc);

    let node = Arc::new(
        OfficialNodeRpc::connect(&format!("http://{sequencer_address}"))
            .expect("isolated official-node loopback endpoint"),
    );
    let planner = Arc::new(
        NativeEscrowPlanner::new(
            Participant::Maker,
            fixture.depositor_private.clone(),
            program_id_from_hex(runtime_descriptor.escrow_program_id),
            program_id_from_hex(fixture.request.terms.authenticated_transfer_program_id()),
            runtime_descriptor.clone(),
            Arc::new(FixedNonce),
        )
        .unwrap(),
    );
    let runtime = Arc::new(BridgeRuntime::new(
        runtime_descriptor.clone(),
        planner,
        node,
        indexer.clone(),
    ));
    let state_directory = tempfile::tempdir().unwrap();
    let bridge = start_bridge_server(
        BridgeServerConfig::new(
            run_id.clone(),
            BridgeServerCapability::new(BRIDGE_CAPABILITY).unwrap(),
            state_directory.path().join("idempotency.json"),
            SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        ),
        runtime,
    )
    .await
    .unwrap();

    let observe = |endpoint: &str| {
        BridgeClient::connect(BridgeClientConfig::new(
            endpoint,
            SidecarCapability::new(BRIDGE_CAPABILITY).unwrap(),
            run_id.clone(),
            runtime_descriptor.clone(),
            Duration::from_secs(2),
        ))
        .unwrap()
    };
    let first = observe(bridge.endpoint())
        .observe_finalized_witnessed_funding(request.clone())
        .await
        .unwrap();
    let second = observe(bridge.endpoint())
        .observe_finalized_witnessed_funding(request)
        .await
        .unwrap();

    assert_eq!(first, second);
    assert_eq!(
        first.funding.transaction.transaction_id,
        expected_funding_transaction_id
    );
    assert_eq!(
        indexer
            .calls
            .lock()
            .unwrap()
            .iter()
            .filter(|call| call.as_str() == "tip")
            .count(),
        4,
        "repeatable observation must execute a fresh stable-tip read"
    );
    assert_eq!(submission_calls.load(Ordering::SeqCst), 0);

    bridge.stop().await.unwrap();
    sequencer_handle.stop().unwrap();
    sequencer_handle.stopped().await;
}

#[tokio::test]
async fn pending_or_incompletely_covered_window_fails_closed() {
    let mut pending_fixture = fixture();
    pending_fixture.blocks[0].bedrock_status = BedrockStatus::Pending;
    let observer = FinalizedWitnessedFundingObserver::new(
        pending_fixture.runtime.clone(),
        indexer(
            &pending_fixture,
            [Some(FINALIZED_TIP_ID), Some(FINALIZED_TIP_ID)],
        ),
    );
    assert_eq!(
        observer
            .observe(&pending_fixture.request)
            .await
            .unwrap_err(),
        BridgeRuntimeError::InvalidObservation
    );

    let fixture = fixture();
    let observer = FinalizedWitnessedFundingObserver::new(
        fixture.runtime.clone(),
        indexer(&fixture, [Some(FUNDING_BLOCK_ID), Some(FUNDING_BLOCK_ID)]),
    );
    assert_eq!(
        observer.observe(&fixture.request).await.unwrap_err(),
        BridgeRuntimeError::Unavailable
    );
}

#[tokio::test]
async fn moved_tip_or_by_id_hash_disagreement_fails_closed() {
    let fixture = fixture();
    let observer = FinalizedWitnessedFundingObserver::new(
        fixture.runtime.clone(),
        indexer(&fixture, [Some(FINALIZED_TIP_ID), Some(12)]),
    );
    assert_eq!(
        observer.observe(&fixture.request).await.unwrap_err(),
        BridgeRuntimeError::MovingTip
    );

    let good = indexer(&fixture, [Some(FINALIZED_TIP_ID), Some(FINALIZED_TIP_ID)]);
    let mut different = fixture.blocks[0].clone();
    different.header.timestamp += 1;
    let bad = Arc::new(MockIndexer {
        tips: Mutex::new(
            [Some(FINALIZED_TIP_ID), Some(FINALIZED_TIP_ID)]
                .into_iter()
                .collect(),
        ),
        by_id: good.by_id.clone(),
        by_hash: good
            .by_hash
            .iter()
            .map(|(hash, block)| {
                (
                    *hash,
                    if *hash == [10; 32] {
                        different.clone()
                    } else {
                        block.clone()
                    },
                )
            })
            .collect(),
        accounts: good.accounts.clone(),
        calls: Mutex::new(Vec::new()),
    });
    let observer = FinalizedWitnessedFundingObserver::new(fixture.runtime, bad);
    assert_eq!(
        observer.observe(&fixture.request).await.unwrap_err(),
        BridgeRuntimeError::InvalidObservation
    );
}

#[tokio::test]
async fn same_height_tip_with_changed_consistent_block_identity_fails_closed() {
    let fixture = fixture();
    let base = indexer(&fixture, [Some(FINALIZED_TIP_ID), Some(FINALIZED_TIP_ID)]);
    let mut changed_tip = fixture.blocks[1].clone();
    changed_tip.header.hash = HashType([12; 32]);
    changed_tip.header.timestamp += 1;
    let indexer = Arc::new(SameHeightMovingTipIndexer {
        base,
        changed_tip,
        tip_id_reads: AtomicUsize::new(0),
        serving_changed_tip: AtomicBool::new(false),
    });
    let observer = FinalizedWitnessedFundingObserver::new(fixture.runtime, indexer);

    assert_eq!(
        observer.observe(&fixture.request).await.unwrap_err(),
        BridgeRuntimeError::MovingTip
    );
}

#[tokio::test]
async fn discovery_distinguishes_duplicate_conflicting_and_absent_funding() {
    let mut ambiguous = fixture();
    ambiguous.blocks[1].body.transactions = ambiguous.blocks[0].body.transactions.clone();
    let request = peerless_request(&ambiguous);
    let observer = FinalizedWitnessedFundingObserver::new(
        ambiguous.runtime.clone(),
        indexer(&ambiguous, [Some(FINALIZED_TIP_ID), Some(FINALIZED_TIP_ID)]),
    );
    assert_eq!(
        observer.observe(&request).await.unwrap_err(),
        BridgeRuntimeError::AmbiguousDiscovery
    );

    let mut conflicting = fixture();
    replace_funding_message(&mut conflicting, |message| {
        message.account_ids.swap(0, 1);
    });
    let request = peerless_request(&conflicting);
    let observer = FinalizedWitnessedFundingObserver::new(
        conflicting.runtime.clone(),
        indexer(
            &conflicting,
            [Some(FINALIZED_TIP_ID), Some(FINALIZED_TIP_ID)],
        ),
    );
    assert_eq!(
        observer.observe(&request).await.unwrap_err(),
        BridgeRuntimeError::ConflictingDiscovery
    );

    let mut absent = fixture();
    absent.blocks[0].body.transactions.clear();
    let request = peerless_request(&absent);
    let observer = FinalizedWitnessedFundingObserver::new(
        absent.runtime.clone(),
        indexer(&absent, [Some(FINALIZED_TIP_ID), Some(FINALIZED_TIP_ID)]),
    );
    assert_eq!(
        observer.observe(&request).await.unwrap_err(),
        BridgeRuntimeError::Unavailable
    );
}

#[tokio::test]
async fn malformed_transaction_signature_or_account_order_fails_closed() {
    let mut malformed = fixture();
    replace_funding_message(&mut malformed, |message| {
        message.instruction_data.push(99);
    });
    retarget_exact_request(&mut malformed);
    let observer = FinalizedWitnessedFundingObserver::new(
        malformed.runtime.clone(),
        indexer(&malformed, [Some(FINALIZED_TIP_ID), Some(FINALIZED_TIP_ID)]),
    );
    assert_eq!(
        observer.observe(&malformed.request).await.unwrap_err(),
        BridgeRuntimeError::InvalidObservation
    );

    let mut bad_signature = fixture();
    let Transaction::Public(indexed) = &mut bad_signature.blocks[0].body.transactions[0] else {
        panic!("public fixture")
    };
    indexed.witness_set.signatures_and_public_keys[0].0.0[0] ^= 1;
    let observer = FinalizedWitnessedFundingObserver::new(
        bad_signature.runtime.clone(),
        indexer(
            &bad_signature,
            [Some(FINALIZED_TIP_ID), Some(FINALIZED_TIP_ID)],
        ),
    );
    assert_eq!(
        observer.observe(&bad_signature.request).await.unwrap_err(),
        BridgeRuntimeError::InvalidObservation
    );

    let mut wrong_accounts = fixture();
    replace_funding_message(&mut wrong_accounts, |message| {
        message.account_ids.swap(0, 1);
    });
    retarget_exact_request(&mut wrong_accounts);
    let observer = FinalizedWitnessedFundingObserver::new(
        wrong_accounts.runtime.clone(),
        indexer(
            &wrong_accounts,
            [Some(FINALIZED_TIP_ID), Some(FINALIZED_TIP_ID)],
        ),
    );
    assert_eq!(
        observer.observe(&wrong_accounts.request).await.unwrap_err(),
        BridgeRuntimeError::InvalidObservation
    );
}

async fn assert_invalid_historical_state(fixture: Fixture) {
    let observer = FinalizedWitnessedFundingObserver::new(
        fixture.runtime.clone(),
        indexer(&fixture, [Some(FINALIZED_TIP_ID), Some(FINALIZED_TIP_ID)]),
    );
    assert_eq!(
        observer.observe(&fixture.request).await.unwrap_err(),
        BridgeRuntimeError::InvalidObservation
    );
}

#[tokio::test]
async fn historical_state_must_be_funded_with_exact_custody_at_containing_block() {
    let mut case = fixture();
    mutate_metadata(&mut case, |metadata| metadata.amount += 1);
    assert_invalid_historical_state(case).await;

    let mut case = fixture();
    mutate_metadata(&mut case, |metadata| metadata.refund_at += 1);
    assert_invalid_historical_state(case).await;

    let mut case = fixture();
    mutate_metadata(&mut case, |metadata| {
        metadata.depositor = AccountId::new([88; 32]);
    });
    assert_invalid_historical_state(case).await;

    let mut case = fixture();
    mutate_metadata(&mut case, |metadata| metadata.terms_hash = [89; 32]);
    assert_invalid_historical_state(case).await;

    let mut case = fixture();
    mutate_metadata(&mut case, |metadata| {
        metadata.status = EscrowStatus::Claimed;
    });
    assert_invalid_historical_state(case).await;

    let mut case = fixture();
    let escrow_program = program_id_from_hex(case.runtime.escrow_program_id);
    let custody = compute_custody_pda(&escrow_program, case.request.terms.swap_id().as_bytes());
    case.accounts
        .get_mut(&(custody.into_value(), FUNDING_BLOCK_ID))
        .unwrap()
        .balance -= 1;
    assert_invalid_historical_state(case).await;

    let mut case = fixture();
    let custody = compute_custody_pda(
        &program_id_from_hex(case.runtime.escrow_program_id),
        case.request.terms.swap_id().as_bytes(),
    );
    case.accounts
        .get_mut(&(custody.into_value(), FUNDING_BLOCK_ID))
        .unwrap()
        .program_owner = IndexedProgramId(program_id_from_hex(h(91)));
    assert_invalid_historical_state(case).await;

    let mut case = fixture();
    let metadata = compute_metadata_pda(
        &program_id_from_hex(case.runtime.escrow_program_id),
        case.request.terms.swap_id().as_bytes(),
    );
    case.accounts
        .get_mut(&(metadata.into_value(), FUNDING_BLOCK_ID))
        .unwrap()
        .program_owner = IndexedProgramId(program_id_from_hex(h(90)));
    assert_invalid_historical_state(case).await;
}

#[tokio::test]
async fn either_bound_actor_can_observe_but_cross_role_signer_is_rejected_before_indexer_reads() {
    let mut claimant_fixture = fixture();
    claimant_fixture.runtime.sidecar_role = Participant::Taker;
    claimant_fixture.runtime.signer_account_id =
        claimant_fixture.request.terms.claimant_account_id();
    claimant_fixture.request.context.sidecar_role = Participant::Taker;
    claimant_fixture.request.runtime = claimant_fixture.runtime.clone();
    claimant_fixture.request.target = FinalizedWitnessedFundingObservationTarget::DiscoverByTerms;
    let observer = FinalizedWitnessedFundingObserver::new(
        claimant_fixture.runtime.clone(),
        indexer(
            &claimant_fixture,
            [Some(FINALIZED_TIP_ID), Some(FINALIZED_TIP_ID)],
        ),
    );
    let _ = observer.observe(&claimant_fixture.request).await.unwrap();

    claimant_fixture.runtime.signer_account_id = h(99);
    claimant_fixture.request.runtime = claimant_fixture.runtime.clone();
    let indexer = indexer(
        &claimant_fixture,
        [Some(FINALIZED_TIP_ID), Some(FINALIZED_TIP_ID)],
    );
    let observer =
        FinalizedWitnessedFundingObserver::new(claimant_fixture.runtime, indexer.clone());
    assert_eq!(
        observer
            .observe(&claimant_fixture.request)
            .await
            .unwrap_err(),
        BridgeRuntimeError::Planner
    );
    assert!(indexer.calls.lock().unwrap().is_empty());
}

const fn h(byte: u8) -> Hex32 {
    Hex32::from_bytes([byte; 32])
}
