use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use async_trait::async_trait;
use lez_bridge_client::{BridgeClient, BridgeClientConfig, BridgeClientError, SidecarCapability};
use lez_bridge_protocol::{
    DescribeRuntimeRequest, ErrorCode, EscrowObservationTarget, Hex32, MessageContext,
    NativeEscrowTerms, NativeEscrowTermsInput, ObserveEscrowRequest, ObserveRevealingClaimRequest,
    Participant, PrepareNativeEscrowRequest, PrepareRevealingClaimRequest, RequestId,
    RevealingClaimObservationTarget, RevealingPreimage, RunId, RuntimeCompatibility,
    RuntimeDescriptor, SubmitTransactionRequest, SubmitTransactionResult,
};
use lez_v0_1_2_sidecar::{
    BridgeServerCapability, BridgeServerConfig, ExactTransactionSubmitter, NativeEscrowPlanner,
    NonceSource, SidecarError, start_bridge_server,
};
use nssa::{AccountId, PrivateKey, PublicKey, program::Program};
use sha2::{Digest as _, Sha256};
use tempfile::TempDir;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

const CAPABILITY: &str = "server-test-capability-000000000001";
const RUN: &str = "bridge-server-run-0001";

#[derive(Debug)]
struct FixedNonce {
    nonce: u128,
    calls: Arc<AtomicUsize>,
}

#[async_trait]
impl NonceSource for FixedNonce {
    async fn account_nonce(&self, _account_id: AccountId) -> Result<u128, SidecarError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(self.nonce)
    }
}

#[derive(Debug)]
struct UnknownSubmitter {
    calls: Arc<AtomicUsize>,
}

#[derive(Debug)]
struct BlockingUnknownSubmitter {
    calls: Arc<AtomicUsize>,
    entered: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
}

#[async_trait]
impl ExactTransactionSubmitter for BlockingUnknownSubmitter {
    async fn submit_exact(
        &self,
        planner: &NativeEscrowPlanner,
        request: &SubmitTransactionRequest,
    ) -> Result<SubmitTransactionResult, SidecarError> {
        planner
            .decode_exact_for_submission(&request.transaction, request.context.sidecar_role)
            .await?;
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.entered.notify_one();
        self.release.notified().await;
        Err(SidecarError::UnknownSubmissionOutcome)
    }
}

#[async_trait]
impl ExactTransactionSubmitter for UnknownSubmitter {
    async fn submit_exact(
        &self,
        _planner: &NativeEscrowPlanner,
        _request: &SubmitTransactionRequest,
    ) -> Result<SubmitTransactionResult, SidecarError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Err(SidecarError::UnknownSubmissionOutcome)
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

fn context(request_id: &str) -> MessageContext {
    MessageContext::new(
        RunId::new(RUN).unwrap(),
        RequestId::new(request_id).unwrap(),
        Participant::Maker,
    )
}

fn prepare_request(
    signer: AccountId,
    claimant: AccountId,
    escrow_program: [u32; 8],
) -> PrepareNativeEscrowRequest {
    let terms = NativeEscrowTerms::new(NativeEscrowTermsInput {
        swap_id: h(4),
        terms_hash: h(5),
        secret_digest: Hex32::from_bytes(Sha256::digest([42_u8; 32]).into()),
        depositor: Participant::Maker,
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
        context("prepare-server-0001"),
        runtime(Participant::Maker, signer, escrow_program),
        terms,
    )
}

fn planner(
    key_byte: u8,
    escrow_program: [u32; 8],
    descriptor: &RuntimeDescriptor,
    nonce: u128,
    calls: Arc<AtomicUsize>,
) -> Arc<NativeEscrowPlanner> {
    let (_, key) = keyed_account(key_byte);
    Arc::new(
        NativeEscrowPlanner::new(
            Participant::Maker,
            key,
            escrow_program,
            descriptor.clone(),
            Arc::new(FixedNonce { nonce, calls }),
        )
        .unwrap(),
    )
}

async fn test_server<S: ExactTransactionSubmitter + 'static>(
    descriptor: &RuntimeDescriptor,
    store: PathBuf,
    planner: Arc<NativeEscrowPlanner>,
    submitter: Arc<S>,
) -> lez_v0_1_2_sidecar::BridgeServerHandle {
    start_bridge_server(
        BridgeServerConfig::new(
            RunId::new(RUN).unwrap(),
            descriptor.clone(),
            BridgeServerCapability::new(CAPABILITY).unwrap(),
            store,
        ),
        planner,
        submitter,
    )
    .await
    .unwrap()
}

fn client(endpoint: &str, descriptor: &RuntimeDescriptor) -> BridgeClient {
    BridgeClient::connect(BridgeClientConfig::new(
        endpoint,
        SidecarCapability::new(CAPABILITY).unwrap(),
        RunId::new(RUN).unwrap(),
        descriptor.clone(),
        Duration::from_secs(2),
    ))
    .unwrap()
}

async fn raw_request(endpoint: &str, body: &[u8], declared_length: usize) -> Vec<u8> {
    let address = endpoint.strip_prefix("http://").unwrap();
    let mut stream = tokio::net::TcpStream::connect(address).await.unwrap();
    let headers = format!(
        "POST / HTTP/1.1\r\nHost: {address}\r\ncontent-type: application/json\r\nauthorization: Bearer {CAPABILITY}\r\nx-lez-bridge-run-id: {RUN}\r\nx-lez-bridge-sidecar-role: maker\r\ncontent-length: {declared_length}\r\nconnection: close\r\n\r\n"
    );
    stream.write_all(headers.as_bytes()).await.unwrap();
    stream.write_all(body).await.unwrap();
    stream.shutdown().await.unwrap();
    let mut response = Vec::new();
    tokio::time::timeout(Duration::from_secs(2), stream.read_to_end(&mut response))
        .await
        .unwrap()
        .unwrap();
    response
}

#[tokio::test]
async fn durable_cache_replays_randomized_prepare_and_unknown_submit_across_restart() {
    let temp = TempDir::new().unwrap();
    let store = temp.path().join("bridge-idempotency.json");
    let (signer, _) = keyed_account(81);
    let (claimant, _) = keyed_account(82);
    let escrow_program = [0x1234_5678; 8];
    let descriptor = runtime(Participant::Maker, signer, escrow_program);
    let nonce_calls = Arc::new(AtomicUsize::new(0));
    let submit_calls = Arc::new(AtomicUsize::new(0));
    let request = prepare_request(signer, claimant, escrow_program);
    let server = test_server(
        &descriptor,
        store.clone(),
        planner(
            81,
            escrow_program,
            &descriptor,
            77,
            Arc::clone(&nonce_calls),
        ),
        Arc::new(UnknownSubmitter {
            calls: Arc::clone(&submit_calls),
        }),
    )
    .await;
    let first_client = client(server.endpoint(), &descriptor);
    let prepared = first_client
        .prepare_native_escrow(request.clone())
        .await
        .unwrap();
    let submit = SubmitTransactionRequest::new(
        context("submit-server-0001"),
        descriptor.clone(),
        prepared.initialization.clone(),
    );
    assert!(matches!(
        first_client.submit_transaction(submit.clone()).await,
        Err(BridgeClientError::Remote(error))
            if error.code() == lez_bridge_protocol::ErrorCode::UnknownSubmissionOutcome
    ));
    let collision_client = client(server.endpoint(), &descriptor);
    let collision = SubmitTransactionRequest::new(
        context("submit-server-0001"),
        descriptor.clone(),
        prepared.funding.clone(),
    );
    assert!(matches!(
        collision_client.submit_transaction(collision).await,
        Err(BridgeClientError::Remote(error)) if error.code() == ErrorCode::InvalidRequest
    ));
    server.stop().await.unwrap();
    assert_eq!(nonce_calls.load(Ordering::SeqCst), 1);
    assert_eq!(submit_calls.load(Ordering::SeqCst), 1);

    let restarted = test_server(
        &descriptor,
        store,
        planner(
            81,
            escrow_program,
            &descriptor,
            999,
            Arc::clone(&nonce_calls),
        ),
        Arc::new(UnknownSubmitter {
            calls: Arc::clone(&submit_calls),
        }),
    )
    .await;
    let replay_client = client(restarted.endpoint(), &descriptor);
    assert_eq!(
        replay_client.prepare_native_escrow(request).await.unwrap(),
        prepared
    );
    assert!(matches!(
        replay_client.submit_transaction(submit).await,
        Err(BridgeClientError::Remote(error))
            if error.code() == lez_bridge_protocol::ErrorCode::UnknownSubmissionOutcome
    ));
    assert_eq!(nonce_calls.load(Ordering::SeqCst), 1);
    assert_eq!(submit_calls.load(Ordering::SeqCst), 1);
    restarted.stop().await.unwrap();
}

#[tokio::test]
async fn registers_all_six_methods_and_does_not_persist_claim_preimage() {
    let temp = TempDir::new().unwrap();
    let store = temp.path().join("all-methods.json");
    let (signer, _) = keyed_account(83);
    let (claimant, _) = keyed_account(84);
    let escrow_program = [0x1357_2468; 8];
    let descriptor = runtime(Participant::Maker, signer, escrow_program);
    let server = test_server(
        &descriptor,
        store.clone(),
        planner(
            83,
            escrow_program,
            &descriptor,
            10,
            Arc::new(AtomicUsize::new(0)),
        ),
        Arc::new(UnknownSubmitter {
            calls: Arc::new(AtomicUsize::new(0)),
        }),
    )
    .await;
    let bridge = client(server.endpoint(), &descriptor);
    assert_eq!(
        bridge
            .describe_runtime(DescribeRuntimeRequest::new(context("describe-server-0001")))
            .await
            .unwrap()
            .runtime,
        descriptor
    );
    let request = prepare_request(signer, claimant, escrow_program);
    let prepared = bridge.prepare_native_escrow(request.clone()).await.unwrap();
    let observe = ObserveEscrowRequest::new(
        context("observe-escrow-server-0001"),
        descriptor.clone(),
        request.terms.clone(),
        EscrowObservationTarget::Exact {
            initialization_transaction_id: prepared.initialization.transaction_id,
            funding_transaction_id: prepared.funding.transaction_id,
        },
    );
    assert!(
        matches!(bridge.observe_escrow(observe).await, Err(BridgeClientError::Remote(error)) if error.code() == ErrorCode::Unavailable)
    );
    let claim = PrepareRevealingClaimRequest::new(
        context("prepare-claim-server-0001"),
        descriptor.clone(),
        request.terms.clone(),
        prepared.funding.transaction_id,
        RevealingPreimage::new([0xab; 32]),
    );
    assert!(
        matches!(bridge.prepare_revealing_claim(claim).await, Err(BridgeClientError::Remote(error)) if error.code() == ErrorCode::Unavailable)
    );
    let observe_claim = ObserveRevealingClaimRequest::new(
        context("observe-claim-server-0001"),
        descriptor.clone(),
        request.terms,
        RevealingClaimObservationTarget::Exact {
            claim_transaction_id: prepared.funding.transaction_id,
        },
    );
    assert!(
        matches!(bridge.observe_revealing_claim(observe_claim).await, Err(BridgeClientError::Remote(error)) if error.code() == ErrorCode::Unavailable)
    );
    let submit = SubmitTransactionRequest::new(
        context("all-methods-submit-0001"),
        descriptor,
        prepared.initialization,
    );
    assert!(
        matches!(bridge.submit_transaction(submit).await, Err(BridgeClientError::Remote(error)) if error.code() == ErrorCode::UnknownSubmissionOutcome)
    );
    assert!(
        !std::fs::read_to_string(store)
            .unwrap()
            .contains(&"ab".repeat(32))
    );
    server.stop().await.unwrap();
}

#[tokio::test]
async fn rejects_wrong_capability_before_planning() {
    let temp = TempDir::new().unwrap();
    let (signer, _) = keyed_account(91);
    let (claimant, _) = keyed_account(92);
    let escrow_program = [0x8765_4321; 8];
    let descriptor = runtime(Participant::Maker, signer, escrow_program);
    let nonce_calls = Arc::new(AtomicUsize::new(0));
    let server = test_server(
        &descriptor,
        temp.path().join("bridge-idempotency.json"),
        planner(91, escrow_program, &descriptor, 1, Arc::clone(&nonce_calls)),
        Arc::new(UnknownSubmitter {
            calls: Arc::new(AtomicUsize::new(0)),
        }),
    )
    .await;
    let wrong = BridgeClient::connect(BridgeClientConfig::new(
        server.endpoint(),
        SidecarCapability::new("wrong-capability-00000000000000000001").unwrap(),
        RunId::new(RUN).unwrap(),
        descriptor.clone(),
        Duration::from_secs(2),
    ))
    .unwrap();
    assert!(
        wrong
            .prepare_native_escrow(prepare_request(signer, claimant, escrow_program))
            .await
            .is_err()
    );
    assert_eq!(nonce_calls.load(Ordering::SeqCst), 0);

    let wrong_run = RunId::new("wrong-bridge-run-0001").unwrap();
    let wrong_run_client = BridgeClient::connect(BridgeClientConfig::new(
        server.endpoint(),
        SidecarCapability::new(CAPABILITY).unwrap(),
        wrong_run.clone(),
        descriptor.clone(),
        Duration::from_secs(2),
    ))
    .unwrap();
    let mut wrong_run_request = prepare_request(signer, claimant, escrow_program);
    wrong_run_request.context = MessageContext::new(
        wrong_run,
        RequestId::new("wrong-run-request-0001").unwrap(),
        Participant::Maker,
    );
    assert!(
        wrong_run_client
            .prepare_native_escrow(wrong_run_request)
            .await
            .is_err()
    );
    assert_eq!(nonce_calls.load(Ordering::SeqCst), 0);

    let mut taker_runtime = descriptor.clone();
    taker_runtime.sidecar_role = Participant::Taker;
    let wrong_role_client = BridgeClient::connect(BridgeClientConfig::new(
        server.endpoint(),
        SidecarCapability::new(CAPABILITY).unwrap(),
        RunId::new(RUN).unwrap(),
        taker_runtime.clone(),
        Duration::from_secs(2),
    ))
    .unwrap();
    let mut wrong_role_request = prepare_request(signer, claimant, escrow_program);
    wrong_role_request.context = MessageContext::new(
        RunId::new(RUN).unwrap(),
        RequestId::new("wrong-role-request-0001").unwrap(),
        Participant::Taker,
    );
    wrong_role_request.runtime = taker_runtime;
    assert!(
        wrong_role_client
            .prepare_native_escrow(wrong_role_request)
            .await
            .is_err()
    );
    assert_eq!(nonce_calls.load(Ordering::SeqCst), 0);
    drop(wrong);
    drop(wrong_run_client);
    drop(wrong_role_client);
    server.stop().await.unwrap();
}

#[tokio::test]
async fn rejects_malformed_and_oversized_bodies_before_planning() {
    let temp = TempDir::new().unwrap();
    let (signer, _) = keyed_account(93);
    let escrow_program = [0x1020_3040; 8];
    let descriptor = runtime(Participant::Maker, signer, escrow_program);
    let nonce_calls = Arc::new(AtomicUsize::new(0));
    let server = test_server(
        &descriptor,
        temp.path().join("raw-input.json"),
        planner(93, escrow_program, &descriptor, 1, Arc::clone(&nonce_calls)),
        Arc::new(UnknownSubmitter {
            calls: Arc::new(AtomicUsize::new(0)),
        }),
    )
    .await;
    let malformed = raw_request(server.endpoint(), b"{not-json", 9).await;
    assert!(
        !malformed.starts_with(b"HTTP/1.1 200"),
        "unexpected malformed response: {}",
        String::from_utf8_lossy(&malformed)
    );
    assert_eq!(nonce_calls.load(Ordering::SeqCst), 0);

    let oversized = raw_request(server.endpoint(), b"x", 5_500_001).await;
    assert!(
        !oversized.starts_with(b"HTTP/1.1 200"),
        "unexpected oversized response: {}",
        String::from_utf8_lossy(&oversized)
    );
    assert_eq!(nonce_calls.load(Ordering::SeqCst), 0);
    server.stop().await.unwrap();
}

#[tokio::test]
async fn persists_unknown_marker_before_submit_and_restart_never_resubmits() {
    let temp = TempDir::new().unwrap();
    let live_store = temp.path().join("live-idempotency.json");
    let crash_store = temp.path().join("crash-idempotency.json");
    let (signer, _) = keyed_account(101);
    let (claimant, _) = keyed_account(102);
    let escrow_program = [0x2468_1357; 8];
    let descriptor = runtime(Participant::Maker, signer, escrow_program);
    let server = test_server(
        &descriptor,
        live_store.clone(),
        planner(
            101,
            escrow_program,
            &descriptor,
            44,
            Arc::new(AtomicUsize::new(0)),
        ),
        Arc::new(UnknownSubmitter {
            calls: Arc::new(AtomicUsize::new(0)),
        }),
    )
    .await;
    let request = prepare_request(signer, claimant, escrow_program);
    let live_client = client(server.endpoint(), &descriptor);
    let prepared = live_client.prepare_native_escrow(request).await.unwrap();
    let submitted_transaction = prepared.initialization.clone();
    drop(live_client);
    server.stop().await.unwrap();

    let submit_calls = Arc::new(AtomicUsize::new(0));
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let resumed = test_server(
        &descriptor,
        live_store.clone(),
        planner(
            101,
            escrow_program,
            &descriptor,
            999,
            Arc::new(AtomicUsize::new(0)),
        ),
        Arc::new(BlockingUnknownSubmitter {
            calls: Arc::clone(&submit_calls),
            entered: Arc::clone(&entered),
            release: Arc::clone(&release),
        }),
    )
    .await;
    let submit = SubmitTransactionRequest::new(
        context("crash-submit-server-0001"),
        descriptor.clone(),
        submitted_transaction.clone(),
    );
    let resumed_client = client(resumed.endpoint(), &descriptor);
    let submit_task = tokio::spawn(async move { resumed_client.submit_transaction(submit).await });
    tokio::time::timeout(Duration::from_secs(2), entered.notified())
        .await
        .unwrap();
    assert_eq!(submit_calls.load(Ordering::SeqCst), 1);
    assert!(
        std::fs::read_to_string(&live_store)
            .unwrap()
            .contains("submission_in_flight")
    );
    std::fs::copy(&live_store, &crash_store).unwrap();

    let replay_calls = Arc::new(AtomicUsize::new(0));
    let restarted = test_server(
        &descriptor,
        crash_store,
        planner(
            101,
            escrow_program,
            &descriptor,
            999,
            Arc::new(AtomicUsize::new(0)),
        ),
        Arc::new(UnknownSubmitter {
            calls: Arc::clone(&replay_calls),
        }),
    )
    .await;
    let replay_client = client(restarted.endpoint(), &descriptor);
    let replay = SubmitTransactionRequest::new(
        context("crash-submit-server-0001"),
        runtime(Participant::Maker, signer, escrow_program),
        submitted_transaction,
    );
    assert!(matches!(
        replay_client.submit_transaction(replay).await,
        Err(BridgeClientError::Remote(error))
            if error.code() == ErrorCode::UnknownSubmissionOutcome
    ));
    assert_eq!(replay_calls.load(Ordering::SeqCst), 0);

    release.notify_one();
    assert!(submit_task.await.unwrap().is_err());
    restarted.stop().await.unwrap();
    resumed.stop().await.unwrap();
}
