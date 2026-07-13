use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

use common::{HashType, block::Block, transaction::NSSATransaction};
use jsonrpsee::{server::ServerBuilder, types::ErrorObjectOwned};
use lez_bridge_protocol::{
    DiscoveryWindow, EscrowObservationTarget, FundingObservation, Hex32, InitializationObservation,
    MessageContext, NativeEscrowAccountObservation, NativeEscrowTerms, NativeEscrowTermsInput,
    NativeRefundObservation, NativeRefundObservationTarget, ObserveEscrowRequest,
    ObserveNativeRefundRequest, ObserveRevealingClaimRequest, Participant,
    PrepareNativeEscrowRequest, PrepareNativeRefundRequest, PrepareRevealingClaimRequest,
    RequestId, RevealingClaimObservation, RevealingClaimObservationTarget, RevealingPreimage,
    RunId, RuntimeCompatibility, RuntimeDescriptor, SubmissionOutcome, SubmitTransactionRequest,
    TransactionId,
};
use lez_v0_1_2_sidecar::{
    ExactTransactionSubmitter, NativeEscrowPlanner, OfficialExactObservation, OfficialNodeRpc,
    OfficialSettlement, SidecarError,
};
use lez_zec_escrow_compat::{EscrowMetadata, EscrowStatus};
use nssa::{
    AccountId, Data, PrivateKey, PublicKey, PublicTransaction, program::Program,
    public_transaction::WitnessSet,
};
use sequencer_service_protocol::{Account, BlockId, Commitment, MembershipProof, Nonce, ProgramId};
use sequencer_service_rpc::RpcServer;
use sha2::{Digest as _, Sha256};

#[derive(Clone, Copy, Debug)]
enum SubmitMode {
    Accept,
    WrongHash,
    Reject,
    UnexpectedCallError,
}

#[derive(Clone, Debug)]
struct MockNode {
    expected_account: AccountId,
    nonces: Arc<Mutex<Vec<Nonce>>>,
    submit_mode: Arc<Mutex<SubmitMode>>,
    nonce_requests: Arc<Mutex<Vec<Vec<AccountId>>>>,
    submitted: Arc<Mutex<Vec<NSSATransaction>>>,
    blocks: Arc<Mutex<Vec<Block>>>,
    accounts: Arc<Mutex<BTreeMap<AccountId, Account>>>,
    tip_overrides: Arc<Mutex<Vec<BlockId>>>,
}

impl MockNode {
    fn new(expected_account: AccountId, nonce: u128) -> Self {
        Self {
            expected_account,
            nonces: Arc::new(Mutex::new(vec![Nonce(nonce)])),
            submit_mode: Arc::new(Mutex::new(SubmitMode::Accept)),
            nonce_requests: Arc::new(Mutex::new(Vec::new())),
            submitted: Arc::new(Mutex::new(Vec::new())),
            blocks: Arc::new(Mutex::new(Vec::new())),
            accounts: Arc::new(Mutex::new(BTreeMap::new())),
            tip_overrides: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

#[async_trait::async_trait]
impl RpcServer for MockNode {
    async fn send_transaction(
        &self,
        transaction: NSSATransaction,
    ) -> Result<HashType, ErrorObjectOwned> {
        let hash = transaction.hash();
        self.submitted.lock().unwrap().push(transaction);
        match *self.submit_mode.lock().unwrap() {
            SubmitMode::Accept => Ok(hash),
            SubmitMode::WrongHash => Ok(HashType([0x99; 32])),
            SubmitMode::Reject => Err(ErrorObjectOwned::owned(
                -32602,
                "definitive stateless rejection",
                None::<()>,
            )),
            SubmitMode::UnexpectedCallError => Err(ErrorObjectOwned::owned(
                -32603,
                "unexpected server failure",
                None::<()>,
            )),
        }
    }

    async fn check_health(&self) -> Result<(), ErrorObjectOwned> {
        Ok(())
    }

    async fn get_block(&self, _block_id: BlockId) -> Result<Option<Block>, ErrorObjectOwned> {
        Ok(self
            .blocks
            .lock()
            .unwrap()
            .iter()
            .find(|block| block.header.block_id == _block_id)
            .cloned())
    }

    async fn get_block_range(
        &self,
        _start_block_id: BlockId,
        _end_block_id: BlockId,
    ) -> Result<Vec<Block>, ErrorObjectOwned> {
        Ok(self
            .blocks
            .lock()
            .unwrap()
            .iter()
            .filter(|block| (_start_block_id..=_end_block_id).contains(&block.header.block_id))
            .cloned()
            .collect())
    }

    async fn get_last_block_id(&self) -> Result<BlockId, ErrorObjectOwned> {
        if !self.tip_overrides.lock().unwrap().is_empty() {
            return Ok(self.tip_overrides.lock().unwrap().remove(0));
        }
        Ok(self
            .blocks
            .lock()
            .unwrap()
            .last()
            .map_or(0, |block| block.header.block_id))
    }

    async fn get_account_balance(&self, _account_id: AccountId) -> Result<u128, ErrorObjectOwned> {
        Ok(0)
    }

    async fn get_transaction(
        &self,
        _tx_hash: HashType,
    ) -> Result<Option<NSSATransaction>, ErrorObjectOwned> {
        Ok(None)
    }

    async fn get_accounts_nonces(
        &self,
        account_ids: Vec<AccountId>,
    ) -> Result<Vec<Nonce>, ErrorObjectOwned> {
        assert_eq!(account_ids, [self.expected_account]);
        self.nonce_requests.lock().unwrap().push(account_ids);
        Ok(self.nonces.lock().unwrap().clone())
    }

    async fn get_proof_for_commitment(
        &self,
        _commitment: Commitment,
    ) -> Result<Option<MembershipProof>, ErrorObjectOwned> {
        Ok(None)
    }

    async fn get_account(&self, account_id: AccountId) -> Result<Account, ErrorObjectOwned> {
        Ok(self
            .accounts
            .lock()
            .unwrap()
            .get(&account_id)
            .cloned()
            .unwrap_or_default())
    }

    async fn get_program_ids(&self) -> Result<BTreeMap<String, ProgramId>, ErrorObjectOwned> {
        Ok(BTreeMap::new())
    }
}

fn keyed_account(byte: u8) -> (AccountId, PrivateKey) {
    let key = PrivateKey::try_new([byte; 32]).unwrap();
    let account = AccountId::from(&PublicKey::new_from_private_key(&key));
    (account, key)
}

fn h(byte: u8) -> Hex32 {
    Hex32::from_bytes([byte; 32])
}

fn program_hex(program_id: [u32; 8]) -> Hex32 {
    let mut bytes = [0_u8; 32];
    for (chunk, word) in bytes.chunks_exact_mut(4).zip(program_id) {
        chunk.copy_from_slice(&word.to_le_bytes());
    }
    Hex32::from_bytes(bytes)
}

fn context(role: Participant, request_id: &str) -> MessageContext {
    MessageContext::new(
        RunId::new("rpc-test-run-0001").unwrap(),
        RequestId::new(request_id).unwrap(),
        role,
    )
}

fn runtime(role: Participant, signer: AccountId, escrow_program: [u32; 8]) -> RuntimeDescriptor {
    RuntimeDescriptor::new(
        role,
        RuntimeCompatibility::NssaV0_1_2,
        h(1),
        h(2),
        h(3),
        program_hex(escrow_program),
        Hex32::from_bytes(signer.into_value()),
    )
}

fn prepare_request(
    role: Participant,
    signer: AccountId,
    claimant: AccountId,
    escrow_program: [u32; 8],
) -> PrepareNativeEscrowRequest {
    let terms = NativeEscrowTerms::new(NativeEscrowTermsInput {
        swap_id: h(4),
        terms_hash: h(5),
        secret_digest: Hex32::from_bytes(Sha256::digest([42_u8; 32]).into()),
        depositor: role,
        depositor_account_id: Hex32::from_bytes(signer.into_value()),
        claimant: Participant::Taker,
        claimant_account_id: Hex32::from_bytes(claimant.into_value()),
        amount: 91,
        refund_at_ms: 1_750_000_000_123,
        authenticated_transfer_program_id: program_hex(
            Program::authenticated_transfer_program().id(),
        ),
    })
    .unwrap();
    PrepareNativeEscrowRequest::new(
        context(role, "prepare-rpc-0001"),
        runtime(role, signer, escrow_program),
        terms,
    )
}

fn funded_native_accounts(
    terms: &NativeEscrowTerms,
    escrow_program: [u32; 8],
) -> BTreeMap<AccountId, Account> {
    native_accounts(terms, escrow_program, EscrowStatus::Funded)
}

fn native_accounts(
    terms: &NativeEscrowTerms,
    escrow_program: [u32; 8],
    status: EscrowStatus,
) -> BTreeMap<AccountId, Account> {
    let swap_id = *terms.swap_id().as_bytes();
    let metadata_id = spel_framework_core::pda::compute_pda(&escrow_program, &[&swap_id]);
    let custody_label = spel_framework_core::pda::seed_from_str("custody");
    let custody_id =
        spel_framework_core::pda::compute_pda(&escrow_program, &[&custody_label, &swap_id]);
    let authenticated_transfer = Program::authenticated_transfer_program().id();
    let metadata = EscrowMetadata {
        version: 1,
        swap_id,
        terms_hash: *terms.terms_hash().as_bytes(),
        secret_digest: *terms.secret_digest().as_bytes(),
        depositor: AccountId::new(*terms.depositor_account_id().as_bytes()),
        depositor_asset: AccountId::new(*terms.depositor_account_id().as_bytes()),
        claimant: AccountId::new(*terms.claimant_account_id().as_bytes()),
        claimant_asset: AccountId::new(*terms.claimant_account_id().as_bytes()),
        custody: custody_id,
        asset_program: authenticated_transfer,
        custody_program: authenticated_transfer,
        asset_definition: [0; 32],
        amount: terms.amount().as_u128(),
        refund_at: terms.refund_at_ms(),
        status,
    };
    BTreeMap::from([
        (
            metadata_id,
            Account {
                program_owner: escrow_program,
                data: Data::try_from(borsh::to_vec(&metadata).unwrap()).unwrap(),
                ..Account::default()
            },
        ),
        (
            custody_id,
            Account {
                program_owner: authenticated_transfer,
                balance: u128::from(matches!(status, EscrowStatus::Funded))
                    * terms.amount().as_u128(),
                ..Account::default()
            },
        ),
    ])
}

fn counterparty_claim_terms(
    depositor: AccountId,
    claimant: AccountId,
    preimage: [u8; 32],
) -> NativeEscrowTerms {
    NativeEscrowTerms::new(NativeEscrowTermsInput {
        swap_id: h(0x31),
        terms_hash: h(0x32),
        secret_digest: Hex32::from_bytes(Sha256::digest(preimage).into()),
        depositor: Participant::Maker,
        depositor_account_id: Hex32::from_bytes(depositor.into_value()),
        claimant: Participant::Taker,
        claimant_account_id: Hex32::from_bytes(claimant.into_value()),
        amount: 888,
        refund_at_ms: 1_790_000_000_123,
        authenticated_transfer_program_id: program_hex(
            Program::authenticated_transfer_program().id(),
        ),
    })
    .unwrap()
}

async fn start_mock(mock: MockNode) -> (String, jsonrpsee::server::ServerHandle) {
    let server = ServerBuilder::default().build("127.0.0.1:0").await.unwrap();
    let address = server.local_addr().unwrap();
    let handle = server.start(mock.into_rpc());
    (format!("http://{address}"), handle)
}

#[tokio::test]
async fn uses_official_nonce_and_submits_only_the_cached_exact_transaction() {
    let (depositor, key) = keyed_account(61);
    let (claimant, _) = keyed_account(62);
    let escrow_program = [0x1234_5678; 8];
    let descriptor = runtime(Participant::Maker, depositor, escrow_program);
    let mock = MockNode::new(depositor, 77);
    let (endpoint, handle) = start_mock(mock.clone()).await;
    let rpc = Arc::new(
        OfficialNodeRpc::connect(&endpoint, Participant::Maker, depositor, descriptor.clone())
            .unwrap(),
    );
    let planner = NativeEscrowPlanner::new(
        Participant::Maker,
        key,
        escrow_program,
        descriptor.clone(),
        Arc::clone(&rpc),
    )
    .unwrap();
    let prepared = planner
        .prepare(prepare_request(
            Participant::Maker,
            depositor,
            claimant,
            escrow_program,
        ))
        .await
        .unwrap();

    assert_eq!(
        mock.nonce_requests.lock().unwrap().as_slice(),
        [&[depositor][..]]
    );
    let request = SubmitTransactionRequest::new(
        context(Participant::Maker, "submit-rpc-0001"),
        descriptor.clone(),
        prepared.initialization.clone(),
    );
    let result = rpc.submit_exact(&planner, &request).await.unwrap();

    assert_eq!(
        result.transaction_id,
        prepared.initialization.transaction_id
    );
    assert_eq!(result.outcome, SubmissionOutcome::Accepted);
    let submitted = mock.submitted.lock().unwrap();
    assert_eq!(submitted.len(), 1);
    assert_eq!(
        submitted[0].hash().0,
        *prepared.initialization.transaction_id.as_bytes()
    );
    assert!(!format!("{rpc:?}").contains(&endpoint));
    handle.stop().unwrap();
}

#[tokio::test]
async fn rejects_nonlocal_endpoints_and_wrong_runtime_identity() {
    let (depositor, _) = keyed_account(71);
    let escrow_program = [0x8765_4321; 8];
    let descriptor = runtime(Participant::Maker, depositor, escrow_program);
    let mock = MockNode::new(depositor, 88);
    let (endpoint, handle) = start_mock(mock.clone()).await;
    assert_eq!(
        OfficialNodeRpc::connect(&endpoint, Participant::Taker, depositor, descriptor.clone(),)
            .unwrap_err(),
        SidecarError::WrongSidecarRole
    );
    let (wrong_signer, _) = keyed_account(73);
    assert_eq!(
        OfficialNodeRpc::connect(
            &endpoint,
            Participant::Maker,
            wrong_signer,
            descriptor.clone(),
        )
        .unwrap_err(),
        SidecarError::WrongSigner
    );
    for invalid in [
        "http://localhost:1234",
        "http://192.0.2.1:1234",
        "https://127.0.0.1:1234",
        "http://user:secret@127.0.0.1:1234",
        "http://127.0.0.1:1234/rpc",
        "http://127.0.0.1:1234/?proxy=1",
    ] {
        assert_eq!(
            OfficialNodeRpc::connect(invalid, Participant::Maker, depositor, descriptor.clone(),)
                .unwrap_err(),
            SidecarError::InvalidNodeEndpoint
        );
    }
    handle.stop().unwrap();
}

#[tokio::test]
async fn fails_closed_on_nonce_shape_hash_rejection_and_transport() {
    let (depositor, key) = keyed_account(71);
    let (claimant, _) = keyed_account(72);
    let escrow_program = [0x8765_4321; 8];
    let descriptor = runtime(Participant::Maker, depositor, escrow_program);
    let mock = MockNode::new(depositor, 88);
    let (endpoint, handle) = start_mock(mock.clone()).await;

    let rpc = Arc::new(
        OfficialNodeRpc::connect(&endpoint, Participant::Maker, depositor, descriptor.clone())
            .unwrap(),
    );
    *mock.nonces.lock().unwrap() = vec![Nonce(88), Nonce(89)];
    assert_eq!(
        lez_v0_1_2_sidecar::NonceSource::account_nonce(&*rpc, depositor)
            .await
            .unwrap_err(),
        SidecarError::NonceUnavailable
    );
    *mock.nonces.lock().unwrap() = vec![Nonce(u128::MAX); 225_000];
    assert_eq!(
        lez_v0_1_2_sidecar::NonceSource::account_nonce(&*rpc, depositor)
            .await
            .unwrap_err(),
        SidecarError::NonceUnavailable
    );
    *mock.nonces.lock().unwrap() = vec![Nonce(88)];
    let planner = NativeEscrowPlanner::new(
        Participant::Maker,
        key,
        escrow_program,
        descriptor.clone(),
        Arc::clone(&rpc),
    )
    .unwrap();
    let mut request = prepare_request(Participant::Maker, depositor, claimant, escrow_program);
    request.runtime = descriptor.clone();
    let prepared = planner.prepare(request).await.unwrap();
    let request = SubmitTransactionRequest::new(
        context(Participant::Maker, "submit-rpc-0002"),
        descriptor,
        prepared.funding,
    );

    let mut wrong_runtime_request = request.clone();
    wrong_runtime_request.runtime.channel_id = h(0xaa);
    assert_eq!(
        rpc.submit_exact(&planner, &wrong_runtime_request)
            .await
            .unwrap_err(),
        SidecarError::WrongRuntimeIdentity
    );
    let mut uncached_request = request.clone();
    uncached_request.transaction.transaction_id = TransactionId::from_bytes([0x44; 32]);
    assert_eq!(
        rpc.submit_exact(&planner, &uncached_request)
            .await
            .unwrap_err(),
        SidecarError::TransactionNotPrepared
    );
    assert!(mock.submitted.lock().unwrap().is_empty());

    *mock.submit_mode.lock().unwrap() = SubmitMode::WrongHash;
    assert_eq!(
        rpc.submit_exact(&planner, &request).await.unwrap_err(),
        SidecarError::UnknownSubmissionOutcome
    );
    *mock.submit_mode.lock().unwrap() = SubmitMode::Reject;
    assert_eq!(
        rpc.submit_exact(&planner, &request).await.unwrap_err(),
        SidecarError::NodeRejected
    );
    *mock.submit_mode.lock().unwrap() = SubmitMode::UnexpectedCallError;
    assert_eq!(
        rpc.submit_exact(&planner, &request).await.unwrap_err(),
        SidecarError::UnknownSubmissionOutcome
    );
    handle.stop().unwrap();
    handle.stopped().await;
    assert_eq!(
        rpc.submit_exact(&planner, &request).await.unwrap_err(),
        SidecarError::UnknownSubmissionOutcome
    );
}

#[tokio::test]
async fn scans_a_bounded_linked_range_for_only_the_exact_persisted_transaction() {
    let (depositor, key) = keyed_account(81);
    let (claimant, _) = keyed_account(82);
    let escrow_program = [0x1111_2222; 8];
    let genesis = common::test_utils::produce_dummy_block(0, None, Vec::new());
    let mut descriptor = runtime(Participant::Maker, depositor, escrow_program);
    descriptor.genesis_block_hash = Hex32::from_bytes(genesis.header.hash.0);
    let mock = MockNode::new(depositor, 90);
    let (endpoint, handle) = start_mock(mock.clone()).await;
    let rpc = Arc::new(
        OfficialNodeRpc::connect(&endpoint, Participant::Maker, depositor, descriptor.clone())
            .unwrap(),
    );
    let planner = NativeEscrowPlanner::new(
        Participant::Maker,
        key,
        escrow_program,
        descriptor.clone(),
        Arc::clone(&rpc),
    )
    .unwrap();
    let mut request = prepare_request(Participant::Maker, depositor, claimant, escrow_program);
    request.runtime = descriptor;
    let prepared = planner.prepare(request).await.unwrap();
    let initialization = planner
        .decode_exact_for_submission(&prepared.initialization, Participant::Maker)
        .await
        .unwrap();
    let block =
        common::test_utils::produce_dummy_block(1, Some(genesis.header.hash), vec![initialization]);
    *mock.blocks.lock().unwrap() = vec![genesis, block.clone()];

    let found = rpc
        .scan_exact(
            &prepared.initialization,
            DiscoveryWindow::new(0, 2).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(found.tip_before, found.tip_after);
    let OfficialExactObservation::Found(found) = found.observation else {
        panic!("exact initialization must be found");
    };
    assert_eq!(
        found.transaction.transaction_id,
        prepared.initialization.transaction_id
    );
    assert_eq!(
        found.transaction.exact_bytes,
        prepared.initialization.exact_bytes
    );
    assert_eq!(found.transaction.position.height, 1);
    assert_eq!(found.transaction.position.transaction_index, 0);
    assert_eq!(found.settlement, OfficialSettlement::Pending);

    let absent = rpc
        .scan_exact(&prepared.funding, DiscoveryWindow::new(0, 2).unwrap())
        .await
        .unwrap();
    assert!(matches!(
        absent.observation,
        OfficialExactObservation::NotFoundInWindow
    ));
    let future = rpc
        .scan_exact(&prepared.funding, DiscoveryWindow::new(2, 1).unwrap())
        .await
        .unwrap();
    assert!(matches!(
        future.observation,
        OfficialExactObservation::NotYetCovered
    ));

    mock.blocks.lock().unwrap()[1].header.prev_block_hash = HashType([0xee; 32]);
    assert_eq!(
        rpc.scan_exact(
            &prepared.initialization,
            DiscoveryWindow::new(0, 2).unwrap(),
        )
        .await
        .unwrap_err(),
        SidecarError::InvalidNodeResponse
    );
    handle.stop().unwrap();
}

struct OwnedMutationFixture<'a> {
    rpc: &'a OfficialNodeRpc,
    planner: &'a NativeEscrowPlanner,
    request: &'a ObserveEscrowRequest,
    terms: &'a NativeEscrowTerms,
    mock: &'a MockNode,
    escrow_program: [u32; 8],
    genesis: &'a Block,
    initialization_tx: NSSATransaction,
    funding_tx: NSSATransaction,
    canonical_blocks: Vec<Block>,
    canonical_accounts: BTreeMap<AccountId, Account>,
}

fn changed_amount_and_deadline(terms: &NativeEscrowTerms) -> NativeEscrowTerms {
    NativeEscrowTerms::new(NativeEscrowTermsInput {
        swap_id: terms.swap_id(),
        terms_hash: terms.terms_hash(),
        secret_digest: terms.secret_digest(),
        depositor: terms.depositor(),
        depositor_account_id: terms.depositor_account_id(),
        claimant: terms.claimant(),
        claimant_account_id: terms.claimant_account_id(),
        amount: terms.amount().as_u128() + 1,
        refund_at_ms: terms.refund_at_ms() + 1,
        authenticated_transfer_program_id: terms.authenticated_transfer_program_id(),
    })
    .unwrap()
}

async fn assert_owned_observation_mutations(fixture: OwnedMutationFixture<'_>) {
    let mut wrong_runtime = fixture.request.clone();
    wrong_runtime.runtime.channel_id = h(0xf1);
    assert_eq!(
        fixture
            .rpc
            .observe_native_escrow(fixture.planner, &wrong_runtime)
            .await
            .unwrap_err(),
        SidecarError::WrongRuntimeIdentity
    );
    let mut wrong_role = fixture.request.clone();
    wrong_role.context.sidecar_role = Participant::Taker;
    assert_eq!(
        fixture
            .rpc
            .observe_native_escrow(fixture.planner, &wrong_role)
            .await
            .unwrap_err(),
        SidecarError::WrongSidecarRole
    );
    let mut wrong_terms = fixture.request.clone();
    wrong_terms.terms = changed_amount_and_deadline(fixture.terms);
    assert_eq!(
        fixture
            .rpc
            .observe_native_escrow(fixture.planner, &wrong_terms)
            .await
            .unwrap_err(),
        SidecarError::TransactionNotPrepared
    );

    let metadata_id = spel_framework_core::pda::compute_pda(
        &fixture.escrow_program,
        &[fixture.terms.swap_id().as_bytes()],
    );
    fixture
        .mock
        .accounts
        .lock()
        .unwrap()
        .get_mut(&metadata_id)
        .unwrap()
        .program_owner = [0; 8];
    assert_eq!(
        fixture
            .rpc
            .observe_native_escrow(fixture.planner, fixture.request)
            .await
            .unwrap_err(),
        SidecarError::InvalidNodeResponse
    );
    *fixture.mock.accounts.lock().unwrap() = fixture.canonical_accounts.clone();
    let custody_label = spel_framework_core::pda::seed_from_str("custody");
    let custody_id = spel_framework_core::pda::compute_pda(
        &fixture.escrow_program,
        &[&custody_label, fixture.terms.swap_id().as_bytes()],
    );
    fixture
        .mock
        .accounts
        .lock()
        .unwrap()
        .get_mut(&custody_id)
        .unwrap()
        .balance += 1;
    assert_eq!(
        fixture
            .rpc
            .observe_native_escrow(fixture.planner, fixture.request)
            .await
            .unwrap_err(),
        SidecarError::InvalidNodeResponse
    );
    *fixture.mock.accounts.lock().unwrap() = fixture.canonical_accounts;
    let reversed = common::test_utils::produce_dummy_block(
        1,
        Some(fixture.genesis.header.hash),
        vec![fixture.funding_tx, fixture.initialization_tx],
    );
    *fixture.mock.blocks.lock().unwrap() = vec![fixture.genesis.clone(), reversed];
    assert_eq!(
        fixture
            .rpc
            .observe_native_escrow(fixture.planner, fixture.request)
            .await
            .unwrap_err(),
        SidecarError::InvalidNodeResponse
    );
    *fixture.mock.blocks.lock().unwrap() = fixture.canonical_blocks;
    *fixture.mock.tip_overrides.lock().unwrap() = vec![1, 1, 0];
    assert_eq!(
        fixture
            .rpc
            .observe_native_escrow(fixture.planner, fixture.request)
            .await
            .unwrap_err(),
        SidecarError::MovingTip
    );
}

async fn assert_discovery_absence_semantics(
    rpc: &OfficialNodeRpc,
    planner: &NativeEscrowPlanner,
    mock: &MockNode,
    runtime: RuntimeDescriptor,
    terms: NativeEscrowTerms,
    genesis: Block,
) {
    *mock.blocks.lock().unwrap() = vec![genesis];
    mock.accounts.lock().unwrap().clear();
    let full_absence = ObserveEscrowRequest::new(
        context(Participant::Taker, "observe-discovery-rpc-0002"),
        runtime.clone(),
        terms.clone(),
        EscrowObservationTarget::DiscoverByTerms {
            window: DiscoveryWindow::new(0, 1).unwrap(),
        },
    );
    let absent = rpc
        .observe_native_escrow(planner, &full_absence)
        .await
        .unwrap();
    assert_eq!(absent.initialization, InitializationObservation::Absent);
    assert_eq!(absent.funding, FundingObservation::Absent);

    let partial = ObserveEscrowRequest::new(
        context(Participant::Taker, "observe-discovery-rpc-0003"),
        runtime,
        terms,
        EscrowObservationTarget::DiscoverByTerms {
            window: DiscoveryWindow::new(0, 2).unwrap(),
        },
    );
    let unknown = rpc.observe_native_escrow(planner, &partial).await.unwrap();
    assert_eq!(
        unknown.initialization,
        InitializationObservation::UnknownOrPending
    );
    assert_eq!(unknown.funding, FundingObservation::UnknownOrPending);
}

async fn assert_ambiguous_discovery(
    rpc: &OfficialNodeRpc,
    planner: &NativeEscrowPlanner,
    mock: &MockNode,
    request: &ObserveEscrowRequest,
    genesis: &Block,
    initialization: NSSATransaction,
    funding: NSSATransaction,
) {
    let ambiguous = common::test_utils::produce_dummy_block(
        1,
        Some(genesis.header.hash),
        vec![initialization.clone(), initialization, funding],
    );
    *mock.blocks.lock().unwrap() = vec![genesis.clone(), ambiguous];
    assert_eq!(
        rpc.observe_native_escrow(planner, request)
            .await
            .unwrap_err(),
        SidecarError::AmbiguousDiscovery
    );
}

#[tokio::test]
async fn observes_the_owned_exact_native_pair_through_the_official_core() {
    let (depositor, key) = keyed_account(91);
    let (claimant, _) = keyed_account(92);
    let escrow_program = [0x3333_4444; 8];
    let genesis = common::test_utils::produce_dummy_block(0, None, Vec::new());
    let mut descriptor = runtime(Participant::Maker, depositor, escrow_program);
    descriptor.genesis_block_hash = Hex32::from_bytes(genesis.header.hash.0);
    let mock = MockNode::new(depositor, 101);
    let (endpoint, handle) = start_mock(mock.clone()).await;
    let rpc = Arc::new(
        OfficialNodeRpc::connect(&endpoint, Participant::Maker, depositor, descriptor.clone())
            .unwrap(),
    );
    let planner = NativeEscrowPlanner::new(
        Participant::Maker,
        key,
        escrow_program,
        descriptor.clone(),
        Arc::clone(&rpc),
    )
    .unwrap();
    let mut prepare = prepare_request(Participant::Maker, depositor, claimant, escrow_program);
    prepare.runtime = descriptor.clone();
    let prepared = planner.prepare(prepare.clone()).await.unwrap();
    let initialization_tx = planner
        .decode_exact_for_submission(&prepared.initialization, Participant::Maker)
        .await
        .unwrap();
    let funding_tx = planner
        .decode_exact_for_submission(&prepared.funding, Participant::Maker)
        .await
        .unwrap();
    let funded = common::test_utils::produce_dummy_block(
        1,
        Some(genesis.header.hash),
        vec![initialization_tx.clone(), funding_tx.clone()],
    );
    let canonical_blocks = vec![genesis.clone(), funded];
    let canonical_accounts = funded_native_accounts(&prepare.terms, escrow_program);
    *mock.blocks.lock().unwrap() = canonical_blocks.clone();
    *mock.accounts.lock().unwrap() = canonical_accounts.clone();
    let request = ObserveEscrowRequest::new(
        context(Participant::Maker, "observe-exact-rpc-0001"),
        descriptor,
        prepare.terms.clone(),
        EscrowObservationTarget::Exact {
            initialization_transaction_id: prepared.initialization.transaction_id,
            funding_transaction_id: prepared.funding.transaction_id,
        },
    );

    let result = rpc.observe_native_escrow(&planner, &request).await.unwrap();

    assert_eq!(result.tip_before, result.tip_after);
    let InitializationObservation::Found(initialization) = result.initialization else {
        panic!("exact initialization must be found");
    };
    let FundingObservation::Found(funding) = result.funding else {
        panic!("exact funding must be found");
    };
    assert_eq!(initialization.transaction.position.transaction_index, 0);
    assert_eq!(funding.transaction.position.transaction_index, 1);
    assert_eq!(
        funding.custody.balance.as_u128(),
        prepare.terms.amount().as_u128()
    );
    assert_eq!(funding.metadata.terms_hash, prepare.terms.terms_hash());

    assert_owned_observation_mutations(OwnedMutationFixture {
        rpc: rpc.as_ref(),
        planner: &planner,
        request: &request,
        terms: &prepare.terms,
        mock: &mock,
        escrow_program,
        genesis: &genesis,
        initialization_tx,
        funding_tx,
        canonical_blocks,
        canonical_accounts,
    })
    .await;
    handle.stop().unwrap();
}

#[tokio::test]
async fn discovers_a_counterparty_pair_and_only_reports_absence_for_a_full_stable_window() {
    let (depositor, depositor_key) = keyed_account(101);
    let (claimant, claimant_key) = keyed_account(102);
    let escrow_program = [0x5555_6666; 8];
    let genesis = common::test_utils::produce_dummy_block(0, None, Vec::new());
    let mut depositor_runtime = runtime(Participant::Maker, depositor, escrow_program);
    depositor_runtime.genesis_block_hash = Hex32::from_bytes(genesis.header.hash.0);
    let mut claimant_runtime = runtime(Participant::Taker, claimant, escrow_program);
    claimant_runtime.genesis_block_hash = Hex32::from_bytes(genesis.header.hash.0);
    let mock = MockNode::new(depositor, 111);
    let (endpoint, handle) = start_mock(mock.clone()).await;
    let depositor_rpc = Arc::new(
        OfficialNodeRpc::connect(
            &endpoint,
            Participant::Maker,
            depositor,
            depositor_runtime.clone(),
        )
        .unwrap(),
    );
    let depositor_planner = NativeEscrowPlanner::new(
        Participant::Maker,
        depositor_key,
        escrow_program,
        depositor_runtime.clone(),
        Arc::clone(&depositor_rpc),
    )
    .unwrap();
    let claimant_rpc = Arc::new(
        OfficialNodeRpc::connect(
            &endpoint,
            Participant::Taker,
            claimant,
            claimant_runtime.clone(),
        )
        .unwrap(),
    );
    let claimant_planner = NativeEscrowPlanner::new(
        Participant::Taker,
        claimant_key,
        escrow_program,
        claimant_runtime.clone(),
        Arc::clone(&claimant_rpc),
    )
    .unwrap();
    let mut prepare = prepare_request(Participant::Maker, depositor, claimant, escrow_program);
    prepare.runtime = depositor_runtime;
    let prepared = depositor_planner.prepare(prepare.clone()).await.unwrap();
    let initialization = depositor_planner
        .decode_exact_for_submission(&prepared.initialization, Participant::Maker)
        .await
        .unwrap();
    let funding = depositor_planner
        .decode_exact_for_submission(&prepared.funding, Participant::Maker)
        .await
        .unwrap();
    let funded = common::test_utils::produce_dummy_block(
        1,
        Some(genesis.header.hash),
        vec![initialization.clone(), funding.clone()],
    );
    *mock.blocks.lock().unwrap() = vec![genesis.clone(), funded];
    *mock.accounts.lock().unwrap() = funded_native_accounts(&prepare.terms, escrow_program);

    let discovery = ObserveEscrowRequest::new(
        context(Participant::Taker, "observe-discovery-rpc-0001"),
        claimant_runtime.clone(),
        prepare.terms.clone(),
        EscrowObservationTarget::DiscoverByTerms {
            window: DiscoveryWindow::new(0, 2).unwrap(),
        },
    );
    let found = claimant_rpc
        .observe_native_escrow(&claimant_planner, &discovery)
        .await
        .unwrap();
    assert!(matches!(
        found.initialization,
        InitializationObservation::Found(_)
    ));
    assert!(matches!(found.funding, FundingObservation::Found(_)));

    assert_ambiguous_discovery(
        claimant_rpc.as_ref(),
        &claimant_planner,
        &mock,
        &discovery,
        &genesis,
        initialization,
        funding,
    )
    .await;

    assert_discovery_absence_semantics(
        claimant_rpc.as_ref(),
        &claimant_planner,
        &mock,
        claimant_runtime,
        prepare.terms,
        genesis,
    )
    .await;
    handle.stop().unwrap();
}

struct ExactClaimEdgeFixture<'a> {
    rpc: &'a OfficialNodeRpc,
    planner: &'a NativeEscrowPlanner,
    mock: &'a MockNode,
    request: &'a ObserveRevealingClaimRequest,
    terms: &'a NativeEscrowTerms,
    escrow_program: [u32; 8],
    genesis: Block,
    canonical_blocks: Vec<Block>,
    canonical_accounts: BTreeMap<AccountId, Account>,
}

async fn assert_exact_claim_edges(fixture: ExactClaimEdgeFixture<'_>) {
    *fixture.mock.accounts.lock().unwrap() =
        native_accounts(fixture.terms, fixture.escrow_program, EscrowStatus::Funded);
    assert_eq!(
        fixture
            .rpc
            .observe_revealing_claim(fixture.planner, fixture.request)
            .await
            .unwrap_err(),
        SidecarError::InvalidNodeResponse
    );
    *fixture.mock.accounts.lock().unwrap() = fixture.canonical_accounts.clone();
    fixture.mock.accounts.lock().unwrap().pop_first();
    assert_eq!(
        fixture
            .rpc
            .observe_revealing_claim(fixture.planner, fixture.request)
            .await
            .unwrap_err(),
        SidecarError::InvalidNodeResponse
    );
    *fixture.mock.accounts.lock().unwrap() = fixture.canonical_accounts;
    *fixture.mock.blocks.lock().unwrap() = vec![fixture.genesis];
    let missed = fixture
        .rpc
        .observe_revealing_claim(fixture.planner, fixture.request)
        .await
        .unwrap();
    assert!(matches!(
        missed.claim,
        RevealingClaimObservation::UnknownOrPending
    ));
    *fixture.mock.blocks.lock().unwrap() = fixture.canonical_blocks;
    *fixture.mock.tip_overrides.lock().unwrap() = vec![1, 1, 1, 0];
    assert_eq!(
        fixture
            .rpc
            .observe_revealing_claim(fixture.planner, fixture.request)
            .await
            .unwrap_err(),
        SidecarError::MovingTip
    );
}

#[tokio::test]
async fn observes_exact_claim_with_terminal_accounts_and_never_infers_absence() {
    let (claimant, key) = keyed_account(111);
    let (depositor, _) = keyed_account(112);
    let escrow_program = [0x7788_99aa; 8];
    let preimage = [0x5a; 32];
    let genesis = common::test_utils::produce_dummy_block(0, None, Vec::new());
    let mut descriptor = runtime(Participant::Taker, claimant, escrow_program);
    descriptor.genesis_block_hash = Hex32::from_bytes(genesis.header.hash.0);
    let terms = NativeEscrowTerms::new(NativeEscrowTermsInput {
        swap_id: h(0x21),
        terms_hash: h(0x22),
        secret_digest: Hex32::from_bytes(Sha256::digest(preimage).into()),
        depositor: Participant::Maker,
        depositor_account_id: Hex32::from_bytes(depositor.into_value()),
        claimant: Participant::Taker,
        claimant_account_id: Hex32::from_bytes(claimant.into_value()),
        amount: 777,
        refund_at_ms: 1_780_000_000_123,
        authenticated_transfer_program_id: program_hex(
            Program::authenticated_transfer_program().id(),
        ),
    })
    .unwrap();
    let mock = MockNode::new(claimant, 121);
    let (endpoint, handle) = start_mock(mock.clone()).await;
    let rpc = Arc::new(
        OfficialNodeRpc::connect(&endpoint, Participant::Taker, claimant, descriptor.clone())
            .unwrap(),
    );
    let planner = NativeEscrowPlanner::new(
        Participant::Taker,
        key,
        escrow_program,
        descriptor.clone(),
        Arc::clone(&rpc),
    )
    .unwrap();
    let prepared = planner
        .prepare_revealing_claim(&PrepareRevealingClaimRequest::new(
            context(Participant::Taker, "claim-observe-prepare-0001"),
            descriptor.clone(),
            terms.clone(),
            TransactionId::from_bytes([0x81; 32]),
            RevealingPreimage::new(preimage),
        ))
        .await
        .unwrap();
    let claim_tx = planner
        .decode_exact_for_submission(&prepared.claim, Participant::Taker)
        .await
        .unwrap();
    let claimed =
        common::test_utils::produce_dummy_block(1, Some(genesis.header.hash), vec![claim_tx]);
    let canonical_blocks = vec![genesis.clone(), claimed];
    let canonical_accounts = native_accounts(&terms, escrow_program, EscrowStatus::Claimed);
    *mock.blocks.lock().unwrap() = canonical_blocks.clone();
    *mock.accounts.lock().unwrap() = canonical_accounts.clone();
    let observe = ObserveRevealingClaimRequest::new(
        context(Participant::Taker, "claim-observe-exact-0001"),
        descriptor,
        terms.clone(),
        RevealingClaimObservationTarget::Exact {
            claim_transaction_id: prepared.claim.transaction_id,
        },
    );

    let found = rpc
        .observe_revealing_claim(&planner, &observe)
        .await
        .unwrap();
    assert_eq!(found.tip_before, found.tip_after);
    let RevealingClaimObservation::Found(facts) = found.claim else {
        panic!("exact revealing claim must be found");
    };
    assert_eq!(
        facts.transaction.transaction_id,
        prepared.claim.transaction_id
    );
    assert_eq!(facts.transaction.exact_bytes, prepared.claim.exact_bytes);
    assert_eq!(facts.instruction.preimage.expose_secret(), &preimage);
    assert_eq!(
        facts.metadata.status,
        lez_bridge_protocol::EscrowState::Claimed
    );
    assert_eq!(facts.custody.balance.as_u128(), 0);
    assert_exact_claim_edges(ExactClaimEdgeFixture {
        rpc: rpc.as_ref(),
        planner: &planner,
        mock: &mock,
        request: &observe,
        terms: &terms,
        escrow_program,
        genesis,
        canonical_blocks,
        canonical_accounts,
    })
    .await;
    handle.stop().unwrap();
}

struct ClaimDiscoveryEdgeFixture<'a> {
    rpc: &'a OfficialNodeRpc,
    planner: &'a NativeEscrowPlanner,
    mock: &'a MockNode,
    request: &'a ObserveRevealingClaimRequest,
    genesis: Block,
    claim: NSSATransaction,
    canonical_blocks: Vec<Block>,
    canonical_accounts: BTreeMap<AccountId, Account>,
}

async fn assert_claim_discovery_edges(fixture: ClaimDiscoveryEdgeFixture<'_>) {
    let mut wrong_digest = fixture.request.clone();
    wrong_digest.terms = counterparty_claim_terms(
        AccountId::new(*fixture.request.terms.depositor_account_id().as_bytes()),
        AccountId::new(*fixture.request.terms.claimant_account_id().as_bytes()),
        [0x7c; 32],
    );
    assert_eq!(
        fixture
            .rpc
            .observe_revealing_claim(fixture.planner, &wrong_digest)
            .await
            .unwrap_err(),
        SidecarError::InvalidNodeResponse
    );

    let ambiguous = common::test_utils::produce_dummy_block(
        1,
        Some(fixture.genesis.header.hash),
        vec![fixture.claim.clone(), fixture.claim],
    );
    *fixture.mock.blocks.lock().unwrap() = vec![fixture.genesis.clone(), ambiguous];
    assert_eq!(
        fixture
            .rpc
            .observe_revealing_claim(fixture.planner, fixture.request)
            .await
            .unwrap_err(),
        SidecarError::AmbiguousDiscovery
    );

    *fixture.mock.blocks.lock().unwrap() = vec![fixture.genesis];
    fixture.mock.accounts.lock().unwrap().clear();
    let mut full = fixture.request.clone();
    full.target = RevealingClaimObservationTarget::DiscoverByTerms {
        window: DiscoveryWindow::new(0, 1).unwrap(),
    };
    let absent = fixture
        .rpc
        .observe_revealing_claim(fixture.planner, &full)
        .await
        .unwrap();
    assert_eq!(absent.claim, RevealingClaimObservation::Absent);
    let partial = fixture
        .rpc
        .observe_revealing_claim(fixture.planner, fixture.request)
        .await
        .unwrap();
    assert_eq!(partial.claim, RevealingClaimObservation::UnknownOrPending);

    *fixture.mock.blocks.lock().unwrap() = fixture.canonical_blocks;
    *fixture.mock.accounts.lock().unwrap() = fixture.canonical_accounts;
    *fixture.mock.tip_overrides.lock().unwrap() = vec![1, 0];
    assert_eq!(
        fixture
            .rpc
            .observe_revealing_claim(fixture.planner, fixture.request)
            .await
            .unwrap_err(),
        SidecarError::MovingTip
    );
}

#[tokio::test]
async fn depositor_discovers_a_counterparty_claim_and_revealed_preimage() {
    let (depositor, depositor_key) = keyed_account(121);
    let (claimant, claimant_key) = keyed_account(122);
    let escrow_program = [0xaabb_ccdd; 8];
    let preimage = [0x6b; 32];
    let genesis = common::test_utils::produce_dummy_block(0, None, Vec::new());
    let mut claimant_runtime = runtime(Participant::Taker, claimant, escrow_program);
    claimant_runtime.genesis_block_hash = Hex32::from_bytes(genesis.header.hash.0);
    let mut depositor_runtime = runtime(Participant::Maker, depositor, escrow_program);
    depositor_runtime.genesis_block_hash = claimant_runtime.genesis_block_hash;
    let terms = counterparty_claim_terms(depositor, claimant, preimage);
    let mock = MockNode::new(claimant, 131);
    let (endpoint, handle) = start_mock(mock.clone()).await;
    let claimant_rpc = Arc::new(
        OfficialNodeRpc::connect(
            &endpoint,
            Participant::Taker,
            claimant,
            claimant_runtime.clone(),
        )
        .unwrap(),
    );
    let claimant_planner = NativeEscrowPlanner::new(
        Participant::Taker,
        claimant_key,
        escrow_program,
        claimant_runtime.clone(),
        Arc::clone(&claimant_rpc),
    )
    .unwrap();
    let prepared = claimant_planner
        .prepare_revealing_claim(&PrepareRevealingClaimRequest::new(
            context(Participant::Taker, "claim-discovery-prepare-0001"),
            claimant_runtime,
            terms.clone(),
            TransactionId::from_bytes([0x91; 32]),
            RevealingPreimage::new(preimage),
        ))
        .await
        .unwrap();
    let claim = claimant_planner
        .decode_exact_for_submission(&prepared.claim, Participant::Taker)
        .await
        .unwrap();
    let claimed =
        common::test_utils::produce_dummy_block(1, Some(genesis.header.hash), vec![claim.clone()]);
    let canonical_blocks = vec![genesis.clone(), claimed];
    let canonical_accounts = native_accounts(&terms, escrow_program, EscrowStatus::Claimed);
    *mock.blocks.lock().unwrap() = canonical_blocks.clone();
    *mock.accounts.lock().unwrap() = canonical_accounts.clone();
    let depositor_rpc = Arc::new(
        OfficialNodeRpc::connect(
            &endpoint,
            Participant::Maker,
            depositor,
            depositor_runtime.clone(),
        )
        .unwrap(),
    );
    let depositor_planner = NativeEscrowPlanner::new(
        Participant::Maker,
        depositor_key,
        escrow_program,
        depositor_runtime.clone(),
        Arc::clone(&depositor_rpc),
    )
    .unwrap();
    let observe = ObserveRevealingClaimRequest::new(
        context(Participant::Maker, "claim-discovery-observe-0001"),
        depositor_runtime,
        terms,
        RevealingClaimObservationTarget::DiscoverByTerms {
            window: DiscoveryWindow::new(0, 2).unwrap(),
        },
    );

    let found = depositor_rpc
        .observe_revealing_claim(&depositor_planner, &observe)
        .await
        .unwrap();
    let RevealingClaimObservation::Found(facts) = found.claim else {
        panic!("counterparty revealing claim must be discovered");
    };
    assert_eq!(
        facts.transaction.transaction_id,
        prepared.claim.transaction_id
    );
    assert_eq!(facts.instruction.preimage.expose_secret(), &preimage);
    assert_claim_discovery_edges(ClaimDiscoveryEdgeFixture {
        rpc: depositor_rpc.as_ref(),
        planner: &depositor_planner,
        mock: &mock,
        request: &observe,
        genesis,
        claim,
        canonical_blocks,
        canonical_accounts,
    })
    .await;
    handle.stop().unwrap();
}

#[tokio::test]
async fn observes_stable_native_refund_state_and_consensus_clock_without_scanning() {
    let (depositor, key) = keyed_account(141);
    let (claimant, _) = keyed_account(142);
    let escrow_program = [0x5060_7080; 8];
    let genesis = common::test_utils::produce_dummy_block(0, None, Vec::new());
    let tip = common::test_utils::produce_dummy_block(1, Some(genesis.header.hash), Vec::new());
    let mut descriptor = runtime(Participant::Maker, depositor, escrow_program);
    descriptor.genesis_block_hash = Hex32::from_bytes(genesis.header.hash.0);
    let mock = MockNode::new(depositor, 161);
    *mock.blocks.lock().unwrap() = vec![genesis, tip.clone()];
    let (endpoint, handle) = start_mock(mock.clone()).await;
    let rpc = Arc::new(
        OfficialNodeRpc::connect(&endpoint, Participant::Maker, depositor, descriptor.clone())
            .unwrap(),
    );
    let planner = NativeEscrowPlanner::new(
        Participant::Maker,
        key,
        escrow_program,
        descriptor.clone(),
        Arc::clone(&rpc),
    )
    .unwrap();
    let prepare = prepare_request(Participant::Maker, depositor, claimant, escrow_program);
    let terms = prepare.terms;
    *mock.accounts.lock().unwrap() = funded_native_accounts(&terms, escrow_program);
    let request = ObserveNativeRefundRequest::new(
        context(Participant::Maker, "refund-state-only-rpc-0001"),
        descriptor,
        terms,
        NativeRefundObservationTarget::StateOnly,
    );

    let observed = rpc.observe_native_refund(&planner, &request).await.unwrap();
    assert_eq!(observed.clock_before, observed.clock_after);
    assert_eq!(observed.clock_after.height, 1);
    assert_eq!(observed.clock_after.timestamp_ms, tip.header.timestamp);
    assert!(matches!(
        observed.accounts,
        NativeEscrowAccountObservation::Found(_)
    ));
    assert_eq!(observed.refund, NativeRefundObservation::NotRequested);

    mock.accounts.lock().unwrap().clear();
    let absent = rpc.observe_native_refund(&planner, &request).await.unwrap();
    assert_eq!(absent.accounts, NativeEscrowAccountObservation::Absent);
    assert_eq!(absent.refund, NativeRefundObservation::NotRequested);
    *mock.accounts.lock().unwrap() = funded_native_accounts(&request.terms, escrow_program);
    mock.accounts.lock().unwrap().pop_first();
    assert_eq!(
        rpc.observe_native_refund(&planner, &request)
            .await
            .unwrap_err(),
        SidecarError::InvalidNodeResponse
    );
    *mock.accounts.lock().unwrap() = funded_native_accounts(&request.terms, escrow_program);
    *mock.tip_overrides.lock().unwrap() = vec![1, 0];
    assert_eq!(
        rpc.observe_native_refund(&planner, &request)
            .await
            .unwrap_err(),
        SidecarError::MovingTip
    );
    handle.stop().unwrap();
}

struct ExactRefundEdgeFixture<'a> {
    rpc: &'a OfficialNodeRpc,
    planner: &'a NativeEscrowPlanner,
    mock: &'a MockNode,
    request: &'a ObserveNativeRefundRequest,
    escrow_program: [u32; 8],
    genesis: Block,
    refund_tx: NSSATransaction,
    canonical_blocks: Vec<Block>,
    canonical_accounts: BTreeMap<AccountId, Account>,
}

async fn assert_exact_refund_edges(fixture: ExactRefundEdgeFixture<'_>) {
    *fixture.mock.accounts.lock().unwrap() = native_accounts(
        &fixture.request.terms,
        fixture.escrow_program,
        EscrowStatus::Funded,
    );
    assert_eq!(
        fixture
            .rpc
            .observe_native_refund(fixture.planner, fixture.request)
            .await
            .unwrap_err(),
        SidecarError::InvalidNodeResponse
    );
    *fixture.mock.accounts.lock().unwrap() = fixture.canonical_accounts.clone();
    let duplicate = common::test_utils::produce_dummy_block(
        1,
        Some(fixture.genesis.header.hash),
        vec![fixture.refund_tx.clone(), fixture.refund_tx],
    );
    *fixture.mock.blocks.lock().unwrap() = vec![fixture.genesis.clone(), duplicate];
    assert_eq!(
        fixture
            .rpc
            .observe_native_refund(fixture.planner, fixture.request)
            .await
            .unwrap_err(),
        SidecarError::InvalidNodeResponse
    );

    *fixture.mock.blocks.lock().unwrap() = vec![fixture.genesis];
    let mut full_miss = fixture.request.clone();
    let NativeRefundObservationTarget::Exact {
        refund_transaction_id,
        ..
    } = full_miss.target
    else {
        unreachable!();
    };
    full_miss.target = NativeRefundObservationTarget::Exact {
        refund_transaction_id,
        window: DiscoveryWindow::new(0, 1).unwrap(),
    };
    let missed = fixture
        .rpc
        .observe_native_refund(fixture.planner, &full_miss)
        .await
        .unwrap();
    assert_eq!(missed.refund, NativeRefundObservation::UnknownOrPending);
    full_miss.target = NativeRefundObservationTarget::Exact {
        refund_transaction_id: TransactionId::from_bytes([0xee; 32]),
        window: DiscoveryWindow::new(0, 1).unwrap(),
    };
    assert_eq!(
        fixture
            .rpc
            .observe_native_refund(fixture.planner, &full_miss)
            .await
            .unwrap_err(),
        SidecarError::TransactionNotPrepared
    );

    *fixture.mock.blocks.lock().unwrap() = fixture.canonical_blocks;
    *fixture.mock.tip_overrides.lock().unwrap() = vec![1, 0];
    assert_eq!(
        fixture
            .rpc
            .observe_native_refund(fixture.planner, fixture.request)
            .await
            .unwrap_err(),
        SidecarError::MovingTip
    );
}

#[tokio::test]
async fn observes_owned_exact_permissionless_refund_with_terminal_accounts() {
    let (depositor, key) = keyed_account(151);
    let (claimant, _) = keyed_account(152);
    let escrow_program = [0x90a0_b0c0; 8];
    let genesis = common::test_utils::produce_dummy_block(0, None, Vec::new());
    let mut descriptor = runtime(Participant::Maker, depositor, escrow_program);
    descriptor.genesis_block_hash = Hex32::from_bytes(genesis.header.hash.0);
    let mock = MockNode::new(depositor, 171);
    let (endpoint, handle) = start_mock(mock.clone()).await;
    let rpc = Arc::new(
        OfficialNodeRpc::connect(&endpoint, Participant::Maker, depositor, descriptor.clone())
            .unwrap(),
    );
    let planner = NativeEscrowPlanner::new(
        Participant::Maker,
        key,
        escrow_program,
        descriptor.clone(),
        Arc::clone(&rpc),
    )
    .unwrap();
    let terms = prepare_request(Participant::Maker, depositor, claimant, escrow_program).terms;
    let prepared = planner
        .prepare_native_refund(PrepareNativeRefundRequest::new(
            context(Participant::Maker, "refund-prepare-rpc-0001"),
            descriptor.clone(),
            terms.clone(),
        ))
        .await
        .unwrap();
    let refund_tx = planner
        .decode_exact_for_submission(&prepared.refund, Participant::Maker)
        .await
        .unwrap();
    let refunded = common::test_utils::produce_dummy_block(
        1,
        Some(genesis.header.hash),
        vec![refund_tx.clone()],
    );
    let canonical_blocks = vec![genesis.clone(), refunded];
    let canonical_accounts = native_accounts(&terms, escrow_program, EscrowStatus::Refunded);
    *mock.blocks.lock().unwrap() = canonical_blocks.clone();
    *mock.accounts.lock().unwrap() = canonical_accounts.clone();
    let request = ObserveNativeRefundRequest::new(
        context(Participant::Maker, "refund-observe-exact-rpc-0001"),
        descriptor,
        terms,
        NativeRefundObservationTarget::Exact {
            refund_transaction_id: prepared.refund.transaction_id,
            window: DiscoveryWindow::new(0, 2).unwrap(),
        },
    );

    let observed = rpc.observe_native_refund(&planner, &request).await.unwrap();
    assert_eq!(observed.clock_before, observed.clock_after);
    let NativeRefundObservation::Found(facts) = observed.refund else {
        panic!("owned exact refund must be found");
    };
    assert_eq!(
        facts.transaction.transaction_id,
        prepared.refund.transaction_id
    );
    assert_eq!(facts.transaction.exact_bytes, prepared.refund.exact_bytes);
    assert_eq!(facts.transaction.position.transaction_index, 0);
    assert!(facts.transaction.signer_account_ids.as_slice().is_empty());
    assert_eq!(facts.instruction.swap_id, request.terms.swap_id());
    assert_exact_refund_edges(ExactRefundEdgeFixture {
        rpc: rpc.as_ref(),
        planner: &planner,
        mock: &mock,
        request: &request,
        escrow_program,
        genesis,
        refund_tx,
        canonical_blocks,
        canonical_accounts,
    })
    .await;
    handle.stop().unwrap();
}

struct RefundDiscoveryEdgeFixture<'a> {
    rpc: &'a OfficialNodeRpc,
    planner: &'a NativeEscrowPlanner,
    mock: &'a MockNode,
    request: &'a ObserveNativeRefundRequest,
    escrow_program: [u32; 8],
    genesis: Block,
    refund_tx: NSSATransaction,
    canonical_blocks: Vec<Block>,
    canonical_accounts: BTreeMap<AccountId, Account>,
}

fn refund_with_forbidden_nonce(transaction: &NSSATransaction) -> NSSATransaction {
    let NSSATransaction::Public(public) = transaction else {
        unreachable!();
    };
    let mut message = public.message().clone();
    message.nonces = vec![191_u128.into()];
    let key = PrivateKey::try_new([0xaf; 32]).unwrap();
    let witnesses = WitnessSet::for_message(&message, &[&key]);
    NSSATransaction::Public(PublicTransaction::new(message, witnesses))
}

async fn assert_refund_discovery_edges(fixture: RefundDiscoveryEdgeFixture<'_>) {
    *fixture.mock.accounts.lock().unwrap() = native_accounts(
        &fixture.request.terms,
        fixture.escrow_program,
        EscrowStatus::Funded,
    );
    assert_eq!(
        fixture
            .rpc
            .observe_native_refund(fixture.planner, fixture.request)
            .await
            .unwrap_err(),
        SidecarError::InvalidNodeResponse
    );
    *fixture.mock.accounts.lock().unwrap() = fixture.canonical_accounts.clone();
    let malformed = common::test_utils::produce_dummy_block(
        1,
        Some(fixture.genesis.header.hash),
        vec![refund_with_forbidden_nonce(&fixture.refund_tx)],
    );
    *fixture.mock.blocks.lock().unwrap() = vec![fixture.genesis.clone(), malformed];
    assert_eq!(
        fixture
            .rpc
            .observe_native_refund(fixture.planner, fixture.request)
            .await
            .unwrap_err(),
        SidecarError::InvalidTransactionBytes
    );
    let ambiguous = common::test_utils::produce_dummy_block(
        1,
        Some(fixture.genesis.header.hash),
        vec![fixture.refund_tx.clone(), fixture.refund_tx],
    );
    *fixture.mock.blocks.lock().unwrap() = vec![fixture.genesis.clone(), ambiguous];
    assert_eq!(
        fixture
            .rpc
            .observe_native_refund(fixture.planner, fixture.request)
            .await
            .unwrap_err(),
        SidecarError::AmbiguousDiscovery
    );

    *fixture.mock.blocks.lock().unwrap() = vec![fixture.genesis];
    *fixture.mock.accounts.lock().unwrap() = native_accounts(
        &fixture.request.terms,
        fixture.escrow_program,
        EscrowStatus::Funded,
    );
    let mut full = fixture.request.clone();
    full.target = NativeRefundObservationTarget::DiscoverByTerms {
        window: DiscoveryWindow::new(0, 1).unwrap(),
    };
    let absent = fixture
        .rpc
        .observe_native_refund(fixture.planner, &full)
        .await
        .unwrap();
    assert_eq!(absent.refund, NativeRefundObservation::Absent);
    let partial = fixture
        .rpc
        .observe_native_refund(fixture.planner, fixture.request)
        .await
        .unwrap();
    assert_eq!(partial.refund, NativeRefundObservation::UnknownOrPending);

    *fixture.mock.blocks.lock().unwrap() = fixture.canonical_blocks;
    *fixture.mock.accounts.lock().unwrap() = fixture.canonical_accounts;
    *fixture.mock.tip_overrides.lock().unwrap() = vec![1, 0];
    assert_eq!(
        fixture
            .rpc
            .observe_native_refund(fixture.planner, fixture.request)
            .await
            .unwrap_err(),
        SidecarError::MovingTip
    );
}

#[tokio::test]
async fn claimant_discovers_counterparty_permissionless_refund_by_terms() {
    let (depositor, depositor_key) = keyed_account(161);
    let (claimant, claimant_key) = keyed_account(162);
    let escrow_program = [0xd0e0_f001; 8];
    let genesis = common::test_utils::produce_dummy_block(0, None, Vec::new());
    let mut depositor_runtime = runtime(Participant::Maker, depositor, escrow_program);
    depositor_runtime.genesis_block_hash = Hex32::from_bytes(genesis.header.hash.0);
    let mut claimant_runtime = runtime(Participant::Taker, claimant, escrow_program);
    claimant_runtime.genesis_block_hash = depositor_runtime.genesis_block_hash;
    let terms = prepare_request(Participant::Maker, depositor, claimant, escrow_program).terms;
    let mock = MockNode::new(depositor, 181);
    let (endpoint, handle) = start_mock(mock.clone()).await;
    let depositor_rpc = Arc::new(
        OfficialNodeRpc::connect(
            &endpoint,
            Participant::Maker,
            depositor,
            depositor_runtime.clone(),
        )
        .unwrap(),
    );
    let depositor_planner = NativeEscrowPlanner::new(
        Participant::Maker,
        depositor_key,
        escrow_program,
        depositor_runtime.clone(),
        Arc::clone(&depositor_rpc),
    )
    .unwrap();
    let prepared = depositor_planner
        .prepare_native_refund(PrepareNativeRefundRequest::new(
            context(Participant::Maker, "refund-discovery-prepare-rpc-0001"),
            depositor_runtime,
            terms.clone(),
        ))
        .await
        .unwrap();
    let refund_tx = depositor_planner
        .decode_exact_for_submission(&prepared.refund, Participant::Maker)
        .await
        .unwrap();
    let refunded = common::test_utils::produce_dummy_block(
        1,
        Some(genesis.header.hash),
        vec![refund_tx.clone()],
    );
    let canonical_blocks = vec![genesis.clone(), refunded];
    let canonical_accounts = native_accounts(&terms, escrow_program, EscrowStatus::Refunded);
    *mock.blocks.lock().unwrap() = canonical_blocks.clone();
    *mock.accounts.lock().unwrap() = canonical_accounts.clone();
    let claimant_rpc = Arc::new(
        OfficialNodeRpc::connect(
            &endpoint,
            Participant::Taker,
            claimant,
            claimant_runtime.clone(),
        )
        .unwrap(),
    );
    let claimant_planner = NativeEscrowPlanner::new(
        Participant::Taker,
        claimant_key,
        escrow_program,
        claimant_runtime.clone(),
        Arc::clone(&claimant_rpc),
    )
    .unwrap();
    let request = ObserveNativeRefundRequest::new(
        context(Participant::Taker, "refund-discovery-observe-rpc-0001"),
        claimant_runtime,
        terms,
        NativeRefundObservationTarget::DiscoverByTerms {
            window: DiscoveryWindow::new(0, 2).unwrap(),
        },
    );

    let observed = claimant_rpc
        .observe_native_refund(&claimant_planner, &request)
        .await
        .unwrap();
    let NativeRefundObservation::Found(facts) = observed.refund else {
        panic!("claimant must discover the depositor refund");
    };
    assert_eq!(
        facts.transaction.transaction_id,
        prepared.refund.transaction_id
    );
    assert!(facts.transaction.signer_account_ids.as_slice().is_empty());
    assert_eq!(facts.instruction.swap_id, request.terms.swap_id());
    assert_refund_discovery_edges(RefundDiscoveryEdgeFixture {
        rpc: claimant_rpc.as_ref(),
        planner: &claimant_planner,
        mock: &mock,
        request: &request,
        escrow_program,
        genesis,
        refund_tx,
        canonical_blocks,
        canonical_accounts,
    })
    .await;
    handle.stop().unwrap();
}
