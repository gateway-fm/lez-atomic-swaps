use std::{
    collections::{BTreeMap, VecDeque},
    net::{Ipv4Addr, SocketAddr},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use async_trait::async_trait;
use borsh::{BorshDeserialize as _, to_vec};
use indexer_service_protocol::{
    Account as IndexedAccount, AccountId as IndexedAccountId, BedrockStatus, Block, BlockBody,
    BlockHeader, Data as IndexedData, HashType, ProgramId as IndexedProgramId,
    PublicMessage as IndexedPublicMessage, PublicTransaction as IndexedPublicTransaction,
    Signature as IndexedSignature, Transaction, WitnessSet as IndexedWitnessSet,
};
use jsonrpsee::{RpcModule, server::ServerBuilder, types::ErrorObjectOwned};
use lez_bridge_client::{BridgeClient, BridgeClientConfig, SidecarCapability};
use lez_bridge_protocol::{
    DiscoveryWindow, Hex32, MessageContext, NativeEscrowAccountObservation,
    NativeRefundObservation, NativeRefundObservationTarget, ObserveNativeRefundRequest,
    Participant, PrepareNativeRefundRequest, RequestId, RunId, RuntimeCompatibility,
    RuntimeDescriptor, WitnessedNativeEscrowTerms, WitnessedNativeEscrowTermsInput,
};
use lez_v0_2_sidecar::{
    BridgeRuntime, BridgeRuntimeError, BridgeServerCapability, BridgeServerConfig,
    FinalizedIndexerApi, FinalizedWitnessedRefundObserver, NativeEscrowPlanner, NativePrepareError,
    NonceSource, OfficialNodeRpc, ZecEscrowInstruction, compute_custody_pda, compute_metadata_pda,
    prepared_from_transaction, program_id_from_hex, start_bridge_server,
};
use lez_zec_escrow_v02::{ClaimAuthority, EscrowMetadata, EscrowStatus};
use nssa::{
    AccountId, PrivateKey, PublicKey, PublicTransaction,
    public_transaction::{Message, WitnessSet},
};

const BRIDGE_CAPABILITY: &str = "finalized-refund-capability-000001";
const REFUND_BLOCK_ID: u64 = 10;
const FINALIZED_TIP_ID: u64 = 11;
const REFUND_AT_MS: u64 = 1_850_000_000_010;

#[derive(Debug)]
struct FixedNonce;

#[async_trait]
impl NonceSource for FixedNonce {
    async fn account_nonce(&self, _account_id: AccountId) -> Result<u128, NativePrepareError> {
        panic!("permissionless refund observation must not request a signer nonce")
    }
}

#[derive(Debug)]
struct MockIndexer {
    tips: Mutex<VecDeque<Option<u64>>>,
    by_id: BTreeMap<u64, Block>,
    by_hash: BTreeMap<[u8; 32], Block>,
    accounts: BTreeMap<([u8; 32], u64), IndexedAccount>,
    tip_calls: AtomicUsize,
}

#[async_trait]
impl FinalizedIndexerApi for MockIndexer {
    async fn last_finalized_block_id(&self) -> Result<Option<u64>, BridgeRuntimeError> {
        self.tip_calls.fetch_add(1, Ordering::SeqCst);
        let mut tips = self.tips.lock().unwrap();
        Ok(if tips.len() > 1 {
            tips.pop_front().unwrap()
        } else {
            tips.front().copied().flatten()
        })
    }

    async fn block_by_id(&self, block_id: u64) -> Result<Option<Block>, BridgeRuntimeError> {
        Ok(self.by_id.get(&block_id).cloned())
    }

    async fn block_by_hash(
        &self,
        block_hash: [u8; 32],
    ) -> Result<Option<Block>, BridgeRuntimeError> {
        Ok(self.by_hash.get(&block_hash).cloned())
    }

    async fn account_at_block(
        &self,
        account_id: [u8; 32],
        block_id: u64,
    ) -> Result<IndexedAccount, BridgeRuntimeError> {
        self.accounts
            .get(&(account_id, block_id))
            .cloned()
            .ok_or(BridgeRuntimeError::Unavailable)
    }
}

struct Fixture {
    runtime: RuntimeDescriptor,
    planner: Arc<NativeEscrowPlanner>,
    request: ObserveNativeRefundRequest,
    blocks: Vec<Block>,
    accounts: BTreeMap<([u8; 32], u64), IndexedAccount>,
}

const fn h(byte: u8) -> Hex32 {
    Hex32::from_bytes([byte; 32])
}

fn program_words(program_id: Hex32) -> [u32; 8] {
    let mut words = [0_u32; 8];
    for (word, bytes) in words.iter_mut().zip(program_id.as_bytes().chunks_exact(4)) {
        *word = u32::from_le_bytes(bytes.try_into().unwrap());
    }
    words
}

#[allow(
    clippy::too_many_lines,
    reason = "the fixture keeps one complete witnessed refund transcript auditable"
)]
async fn fixture() -> Fixture {
    let aggregate_private = PrivateKey::try_new([5; 32]).unwrap();
    let aggregate_public = PublicKey::new_from_private_key(&aggregate_private);
    let aggregate_authority = AccountId::from(&aggregate_public);
    let depositor_private = PrivateKey::try_new([6; 32]).unwrap();
    let depositor = AccountId::from(&PublicKey::new_from_private_key(&depositor_private));
    let claimant_private = PrivateKey::try_new([7; 32]).unwrap();
    let claimant = AccountId::from(&PublicKey::new_from_private_key(&claimant_private));
    let escrow_program = h(4);
    let transfer_program = h(46);
    let terms = WitnessedNativeEscrowTerms::new(WitnessedNativeEscrowTermsInput {
        swap_id: h(40),
        terms_hash: h(41),
        depositor: Participant::Maker,
        depositor_account_id: Hex32::from_bytes(depositor.into_value()),
        claimant: Participant::Taker,
        claimant_account_id: Hex32::from_bytes(claimant.into_value()),
        aggregate_authority_account_id: Hex32::from_bytes(aggregate_authority.into_value()),
        aggregate_x_only_public_key: Hex32::from_bytes(*aggregate_public.value()),
        amount: 75,
        refund_at_ms: REFUND_AT_MS,
        authenticated_transfer_program_id: transfer_program,
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
    let planner = Arc::new(
        NativeEscrowPlanner::new(
            Participant::Maker,
            depositor_private,
            program_words(escrow_program),
            program_words(transfer_program),
            runtime.clone(),
            Arc::new(FixedNonce),
        )
        .unwrap(),
    );
    let prepare = PrepareNativeRefundRequest::new_witnessed(
        MessageContext::new(
            RunId::new("finalized-refund-run-0001").unwrap(),
            RequestId::new("finalized-refund-prepare-0001").unwrap(),
            Participant::Maker,
        ),
        runtime.clone(),
        terms.clone(),
    );
    let prepared = planner.prepare_native_refund(&prepare).await.unwrap();
    let metadata = compute_metadata_pda(
        &program_id_from_hex(escrow_program),
        terms.swap_id().as_bytes(),
    );
    let custody = compute_custody_pda(
        &program_id_from_hex(escrow_program),
        terms.swap_id().as_bytes(),
    );
    let public = PublicTransaction::new(
        Message::try_new(
            program_id_from_hex(escrow_program),
            vec![metadata, custody, depositor],
            Vec::new(),
            ZecEscrowInstruction::RefundNative {
                swap_id: *terms.swap_id().as_bytes(),
            },
        )
        .unwrap(),
        WitnessSet::from_raw_parts(Vec::new()),
    );
    assert_eq!(prepared.refund, prepared_from_transaction(&public).unwrap());
    let request = ObserveNativeRefundRequest::new_witnessed(
        MessageContext::new(
            prepare.context.run_id,
            RequestId::new("finalized-refund-observe-0001").unwrap(),
            Participant::Maker,
        ),
        runtime.clone(),
        terms,
        NativeRefundObservationTarget::Exact {
            refund_transaction_id: prepared.refund.transaction_id,
            window: DiscoveryWindow::new(REFUND_BLOCK_ID, 2).unwrap(),
        },
    );
    let blocks = vec![
        block(
            REFUND_BLOCK_ID,
            [10; 32],
            REFUND_AT_MS,
            vec![Transaction::Public(indexed_public(&public))],
        ),
        block(FINALIZED_TIP_ID, [11; 32], REFUND_AT_MS + 1, Vec::new()),
    ];
    let accounts = refunded_accounts(&request, [REFUND_BLOCK_ID, FINALIZED_TIP_ID]);
    Fixture {
        runtime,
        planner,
        request,
        blocks,
        accounts,
    }
}

fn refunded_accounts(
    request: &ObserveNativeRefundRequest,
    heights: impl IntoIterator<Item = u64>,
) -> BTreeMap<([u8; 32], u64), IndexedAccount> {
    let terms = request.terms.witnessed().unwrap();
    let escrow_program = program_id_from_hex(request.runtime.escrow_program_id);
    let transfer_program = program_id_from_hex(terms.authenticated_transfer_program_id());
    let metadata_id = compute_metadata_pda(&escrow_program, terms.swap_id().as_bytes());
    let custody_id = compute_custody_pda(&escrow_program, terms.swap_id().as_bytes());
    let metadata = EscrowMetadata {
        version: 2,
        swap_id: *terms.swap_id().as_bytes(),
        terms_hash: *terms.terms_hash().as_bytes(),
        claim_authority: ClaimAuthority::AggregateWitness {
            x_only_public_key: *terms.aggregate_x_only_public_key().as_bytes(),
            account_id: AccountId::new(*terms.aggregate_authority_account_id().as_bytes()),
        },
        depositor: AccountId::new(*terms.depositor_account_id().as_bytes()),
        depositor_asset: AccountId::new(*terms.depositor_account_id().as_bytes()),
        claimant: AccountId::new(*terms.claimant_account_id().as_bytes()),
        claimant_asset: AccountId::new(*terms.claimant_account_id().as_bytes()),
        custody: custody_id,
        asset_program: transfer_program,
        custody_program: transfer_program,
        asset_definition: [0; 32],
        amount: terms.amount().as_u128(),
        refund_at: terms.refund_at_ms(),
        status: EscrowStatus::Refunded,
    };
    heights
        .into_iter()
        .flat_map(|height| {
            [
                (
                    (metadata_id.into_value(), height),
                    IndexedAccount {
                        program_owner: IndexedProgramId(escrow_program),
                        balance: 0,
                        data: IndexedData(to_vec(&metadata).unwrap()),
                        nonce: 0,
                    },
                ),
                (
                    (custody_id.into_value(), height),
                    IndexedAccount {
                        program_owner: IndexedProgramId(transfer_program),
                        balance: 0,
                        data: IndexedData(Vec::new()),
                        nonce: 0,
                    },
                ),
            ]
        })
        .collect()
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
            signatures_and_public_keys: Vec::new(),
            proof: None,
        },
    }
}

fn block(block_id: u64, hash: [u8; 32], timestamp: u64, transactions: Vec<Transaction>) -> Block {
    let block_byte = u8::try_from(block_id).unwrap();
    let previous_byte = u8::try_from(block_id.saturating_sub(1)).unwrap();
    Block {
        header: BlockHeader {
            block_id,
            prev_block_hash: HashType([previous_byte; 32]),
            hash: HashType(hash),
            timestamp,
            signature: IndexedSignature([block_byte; 64]),
        },
        body: BlockBody { transactions },
        bedrock_status: BedrockStatus::Finalized,
    }
}

fn indexer(fixture: &Fixture) -> Arc<MockIndexer> {
    Arc::new(MockIndexer {
        tips: Mutex::new(VecDeque::from([
            Some(FINALIZED_TIP_ID),
            Some(FINALIZED_TIP_ID),
        ])),
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
        tip_calls: AtomicUsize::new(0),
    })
}

#[tokio::test]
async fn exact_owned_refund_is_returned_only_from_stable_finalized_deadline_evidence() {
    let fixture = fixture().await;
    let observer = FinalizedWitnessedRefundObserver::new(
        fixture.runtime.clone(),
        Arc::clone(&fixture.planner),
        indexer(&fixture),
    );

    let result = observer.observe(&fixture.request).await.unwrap();

    assert_eq!(result.clock_before, result.clock_after);
    assert_eq!(result.clock_before.height, FINALIZED_TIP_ID);
    assert_eq!(result.clock_before.timestamp_ms, REFUND_AT_MS + 1);
    let NativeEscrowAccountObservation::Found(accounts) = result.accounts else {
        panic!("refunded terminal accounts")
    };
    assert_eq!(
        accounts.metadata.status(),
        lez_bridge_protocol::EscrowState::Refunded
    );
    assert_eq!(accounts.custody.balance.as_u128(), 0);
    let NativeRefundObservation::Found(refund) = result.refund else {
        panic!("exact finalized refund")
    };
    assert!(refund.transaction.signer_account_ids.as_slice().is_empty());
    assert_eq!(refund.transaction.position.height, REFUND_BLOCK_ID);
}

#[tokio::test]
async fn containing_block_one_millisecond_before_deadline_fails_even_with_later_tip() {
    let mut fixture = fixture().await;
    fixture.blocks[0].header.timestamp = REFUND_AT_MS - 1;
    let observer = FinalizedWitnessedRefundObserver::new(
        fixture.runtime.clone(),
        Arc::clone(&fixture.planner),
        indexer(&fixture),
    );

    assert_eq!(
        observer.observe(&fixture.request).await.unwrap_err(),
        BridgeRuntimeError::InvalidObservation
    );
}

fn set_state(
    fixture: &mut Fixture,
    heights: impl IntoIterator<Item = u64>,
    status: EscrowStatus,
    custody_balance: u128,
) {
    let terms = fixture.request.terms.witnessed().unwrap();
    let escrow_program = program_id_from_hex(fixture.runtime.escrow_program_id);
    let metadata_id = compute_metadata_pda(&escrow_program, terms.swap_id().as_bytes());
    let custody_id = compute_custody_pda(&escrow_program, terms.swap_id().as_bytes());
    for height in heights {
        let metadata_account = fixture
            .accounts
            .get_mut(&(metadata_id.into_value(), height))
            .unwrap();
        let mut metadata = EscrowMetadata::try_from_slice(&metadata_account.data.0).unwrap();
        metadata.status = status;
        metadata_account.data = IndexedData(to_vec(&metadata).unwrap());
        fixture
            .accounts
            .get_mut(&(custody_id.into_value(), height))
            .unwrap()
            .balance = custody_balance;
    }
}

#[tokio::test]
async fn state_only_brackets_funded_state_before_and_at_the_deadline() {
    for timestamp in [REFUND_AT_MS - 1, REFUND_AT_MS] {
        let mut fixture = fixture().await;
        fixture.request.target = NativeRefundObservationTarget::StateOnly;
        fixture.blocks[1].header.timestamp = timestamp;
        let amount = fixture
            .request
            .terms
            .witnessed()
            .unwrap()
            .amount()
            .as_u128();
        set_state(
            &mut fixture,
            [FINALIZED_TIP_ID],
            EscrowStatus::Funded,
            amount,
        );
        let observer = FinalizedWitnessedRefundObserver::new(
            fixture.runtime.clone(),
            Arc::clone(&fixture.planner),
            indexer(&fixture),
        );

        let result = observer.observe(&fixture.request).await.unwrap();

        assert_eq!(result.clock_before.timestamp_ms, timestamp);
        assert_eq!(result.refund, NativeRefundObservation::NotRequested);
        let NativeEscrowAccountObservation::Found(accounts) = result.accounts else {
            panic!("funded state-only accounts")
        };
        assert_eq!(
            accounts.metadata.status(),
            lez_bridge_protocol::EscrowState::Funded
        );
        assert_eq!(accounts.custody.balance.as_u128(), amount);
    }
}

#[tokio::test]
async fn finalized_refund_requires_zero_custody_at_the_containing_block() {
    let mut fixture = fixture().await;
    fixture
        .accounts
        .values_mut()
        .find(|account| account.program_owner.0 == program_id_from_hex(h(46)))
        .unwrap()
        .balance = 1;
    let observer = FinalizedWitnessedRefundObserver::new(
        fixture.runtime.clone(),
        Arc::clone(&fixture.planner),
        indexer(&fixture),
    );

    assert_eq!(
        observer.observe(&fixture.request).await.unwrap_err(),
        BridgeRuntimeError::InvalidObservation
    );
}

fn claimant_observer(
    fixture: &Fixture,
    indexer: Arc<MockIndexer>,
) -> (ObserveNativeRefundRequest, FinalizedWitnessedRefundObserver) {
    let terms = fixture.request.terms.witnessed().unwrap().clone();
    let claimant_key = PrivateKey::try_new([7; 32]).unwrap();
    let claimant = AccountId::from(&PublicKey::new_from_private_key(&claimant_key));
    assert_eq!(
        Hex32::from_bytes(claimant.into_value()),
        terms.claimant_account_id()
    );
    let runtime = RuntimeDescriptor::new(
        Participant::Taker,
        RuntimeCompatibility::LeeV0_2_0,
        fixture.runtime.chain_id,
        fixture.runtime.channel_id,
        fixture.runtime.genesis_block_hash,
        fixture.runtime.escrow_program_id,
        terms.claimant_account_id(),
    );
    let planner = Arc::new(
        NativeEscrowPlanner::new(
            Participant::Taker,
            claimant_key,
            program_words(runtime.escrow_program_id),
            program_words(terms.authenticated_transfer_program_id()),
            runtime.clone(),
            Arc::new(FixedNonce),
        )
        .unwrap(),
    );
    let request = ObserveNativeRefundRequest::new_witnessed(
        MessageContext::new(
            fixture.request.context.run_id.clone(),
            RequestId::new("finalized-refund-discover-0001").unwrap(),
            Participant::Taker,
        ),
        runtime.clone(),
        terms,
        NativeRefundObservationTarget::DiscoverByTerms {
            window: DiscoveryWindow::new(REFUND_BLOCK_ID, 2).unwrap(),
        },
    );
    (
        request,
        FinalizedWitnessedRefundObserver::new(runtime, planner, indexer),
    )
}

#[tokio::test]
async fn claimant_discovers_unique_refund_and_complete_absence_but_not_incomplete_absence() {
    let found_fixture = fixture().await;
    let (request, observer) = claimant_observer(&found_fixture, indexer(&found_fixture));
    let found = observer.observe(&request).await.unwrap();
    assert!(matches!(found.refund, NativeRefundObservation::Found(_)));

    let mut absent_fixture = fixture().await;
    absent_fixture.blocks[0].body.transactions.clear();
    let (request, observer) = claimant_observer(&absent_fixture, indexer(&absent_fixture));
    let absent = observer.observe(&request).await.unwrap();
    assert_eq!(absent.refund, NativeRefundObservation::Absent);

    let incomplete_fixture = fixture().await;
    let (mut request, observer) =
        claimant_observer(&incomplete_fixture, indexer(&incomplete_fixture));
    request.target = NativeRefundObservationTarget::DiscoverByTerms {
        window: DiscoveryWindow::new(REFUND_BLOCK_ID, 3).unwrap(),
    };
    assert_eq!(
        observer.observe(&request).await.unwrap_err(),
        BridgeRuntimeError::Unavailable
    );
}

async fn start_sequencer(
    submission_calls: Arc<AtomicUsize>,
) -> (String, jsonrpsee::server::ServerHandle) {
    let server = ServerBuilder::default().build("127.0.0.1:0").await.unwrap();
    let address = server.local_addr().unwrap();
    let mut rpc = RpcModule::new(submission_calls);
    rpc.register_method("checkHealth", |_, _, _| Ok::<_, ErrorObjectOwned>(()))
        .unwrap();
    rpc.register_method("getChannelId", |_, _, _| {
        Ok::<_, ErrorObjectOwned>(hex::encode([2_u8; 32]))
    })
    .unwrap();
    rpc.register_method("sendTransaction", |_, calls, _| {
        calls.fetch_add(1, Ordering::SeqCst);
        Ok::<_, ErrorObjectOwned>([0_u8; 32])
    })
    .unwrap();
    let handle = server.start(rpc);
    (format!("http://{address}"), handle)
}

fn bridge_client(endpoint: &str, runtime: RuntimeDescriptor, run_id: RunId) -> BridgeClient {
    BridgeClient::connect(BridgeClientConfig::new(
        endpoint,
        SidecarCapability::new(BRIDGE_CAPABILITY).unwrap(),
        run_id,
        runtime,
        Duration::from_secs(2),
    ))
    .unwrap()
}

#[tokio::test]
async fn authenticated_observation_is_repeatable_and_never_submits() {
    let fixture = fixture().await;
    let indexer = indexer(&fixture);
    let submission_calls = Arc::new(AtomicUsize::new(0));
    let (sequencer_endpoint, sequencer_handle) =
        start_sequencer(Arc::clone(&submission_calls)).await;
    let node = Arc::new(OfficialNodeRpc::connect(&sequencer_endpoint).unwrap());
    let runtime = Arc::new(BridgeRuntime::new(
        fixture.runtime.clone(),
        Arc::clone(&fixture.planner),
        node,
        Arc::clone(&indexer) as Arc<dyn FinalizedIndexerApi>,
    ));
    let state_directory = tempfile::tempdir().unwrap();
    let bridge = start_bridge_server(
        BridgeServerConfig::new(
            fixture.request.context.run_id.clone(),
            BridgeServerCapability::new(BRIDGE_CAPABILITY).unwrap(),
            state_directory.path().join("idempotency.json"),
            SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        ),
        runtime,
    )
    .await
    .unwrap();

    let first = bridge_client(
        bridge.endpoint(),
        fixture.runtime.clone(),
        fixture.request.context.run_id.clone(),
    )
    .observe_native_refund(fixture.request.clone())
    .await
    .unwrap();
    let second = bridge_client(
        bridge.endpoint(),
        fixture.runtime.clone(),
        fixture.request.context.run_id.clone(),
    )
    .observe_native_refund(fixture.request)
    .await
    .unwrap();

    assert_eq!(first, second);
    assert_eq!(indexer.tip_calls.load(Ordering::SeqCst), 4);
    assert_eq!(submission_calls.load(Ordering::SeqCst), 0);

    bridge.stop().await.unwrap();
    sequencer_handle.stop().unwrap();
    sequencer_handle.stopped().await;
}

#[tokio::test]
async fn contradictory_blocks_broken_ancestry_and_tip_movement_fail_closed() {
    let contradictory_fixture = fixture().await;
    let mut contradictory = indexer(&contradictory_fixture);
    Arc::get_mut(&mut contradictory)
        .unwrap()
        .by_hash
        .insert([10; 32], contradictory_fixture.blocks[1].clone());
    let observer = FinalizedWitnessedRefundObserver::new(
        contradictory_fixture.runtime.clone(),
        Arc::clone(&contradictory_fixture.planner),
        contradictory,
    );
    assert_eq!(
        observer
            .observe(&contradictory_fixture.request)
            .await
            .unwrap_err(),
        BridgeRuntimeError::InvalidObservation
    );

    let mut ancestry_fixture = fixture().await;
    ancestry_fixture.blocks[1].header.prev_block_hash = HashType([99; 32]);
    let observer = FinalizedWitnessedRefundObserver::new(
        ancestry_fixture.runtime.clone(),
        Arc::clone(&ancestry_fixture.planner),
        indexer(&ancestry_fixture),
    );
    assert_eq!(
        observer
            .observe(&ancestry_fixture.request)
            .await
            .unwrap_err(),
        BridgeRuntimeError::InvalidObservation
    );

    let moving_fixture = fixture().await;
    let moving = indexer(&moving_fixture);
    *moving.tips.lock().unwrap() =
        VecDeque::from([Some(FINALIZED_TIP_ID), Some(FINALIZED_TIP_ID + 1)]);
    let observer = FinalizedWitnessedRefundObserver::new(
        moving_fixture.runtime.clone(),
        Arc::clone(&moving_fixture.planner),
        moving,
    );
    assert_eq!(
        observer.observe(&moving_fixture.request).await.unwrap_err(),
        BridgeRuntimeError::MovingTip
    );
}

#[tokio::test]
async fn nonce_mutation_and_cross_role_request_fail_before_projection() {
    let mut mutated_fixture = fixture().await;
    let Transaction::Public(public) = &mut mutated_fixture.blocks[0].body.transactions[0] else {
        panic!("public refund fixture")
    };
    public.message.nonces.push(1);
    let observer = FinalizedWitnessedRefundObserver::new(
        mutated_fixture.runtime.clone(),
        Arc::clone(&mutated_fixture.planner),
        indexer(&mutated_fixture),
    );
    assert_eq!(
        observer
            .observe(&mutated_fixture.request)
            .await
            .unwrap_err(),
        BridgeRuntimeError::InvalidObservation
    );

    let role_fixture = fixture().await;
    let role_indexer = indexer(&role_fixture);
    let observer = FinalizedWitnessedRefundObserver::new(
        role_fixture.runtime.clone(),
        Arc::clone(&role_fixture.planner),
        Arc::clone(&role_indexer) as Arc<dyn FinalizedIndexerApi>,
    );
    let mut request = role_fixture.request;
    request.context.sidecar_role = Participant::Taker;
    assert_eq!(
        observer.observe(&request).await.unwrap_err(),
        BridgeRuntimeError::Planner
    );
    assert_eq!(role_indexer.tip_calls.load(Ordering::SeqCst), 0);

    let wrong_id_fixture = fixture().await;
    let wrong_indexer = indexer(&wrong_id_fixture);
    let observer = FinalizedWitnessedRefundObserver::new(
        wrong_id_fixture.runtime.clone(),
        Arc::clone(&wrong_id_fixture.planner),
        Arc::clone(&wrong_indexer) as Arc<dyn FinalizedIndexerApi>,
    );
    let mut request = wrong_id_fixture.request;
    request.target = NativeRefundObservationTarget::Exact {
        refund_transaction_id: lez_bridge_protocol::TransactionId::from_bytes([99; 32]),
        window: DiscoveryWindow::new(REFUND_BLOCK_ID, 2).unwrap(),
    };
    assert_eq!(
        observer.observe(&request).await.unwrap_err(),
        BridgeRuntimeError::Planner
    );
    assert_eq!(wrong_indexer.tip_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn discovery_distinguishes_duplicate_and_conflicting_refunds() {
    let mut duplicate_fixture = fixture().await;
    let duplicate = duplicate_fixture.blocks[0].body.transactions[0].clone();
    duplicate_fixture.blocks[0]
        .body
        .transactions
        .push(duplicate);
    let (request, observer) = claimant_observer(&duplicate_fixture, indexer(&duplicate_fixture));
    assert_eq!(
        observer.observe(&request).await.unwrap_err(),
        BridgeRuntimeError::AmbiguousDiscovery
    );

    let mut conflict_fixture = fixture().await;
    let Transaction::Public(public) = &mut conflict_fixture.blocks[0].body.transactions[0] else {
        panic!("public refund fixture")
    };
    public.message.account_ids[2].value = [99; 32];
    let (request, observer) = claimant_observer(&conflict_fixture, indexer(&conflict_fixture));
    assert_eq!(
        observer.observe(&request).await.unwrap_err(),
        BridgeRuntimeError::ConflictingDiscovery
    );
}
