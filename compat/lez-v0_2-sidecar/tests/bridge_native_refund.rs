#![cfg(any(target_os = "linux", target_os = "macos"))]

use std::{
    fs,
    net::{Ipv4Addr, SocketAddr},
    os::unix::fs::PermissionsExt as _,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};

use async_trait::async_trait;
use common::{HashType, transaction::LeeTransaction};
use indexer_service_protocol::Block;
use jsonrpsee::{RpcModule, server::ServerBuilder, types::ErrorObjectOwned};
use lez_bridge_client::{BridgeClient, BridgeClientConfig, BridgeClientError, SidecarCapability};
use lez_bridge_protocol::{
    ErrorCode, Hex32, MessageContext, Participant, PrepareNativeRefundRequest,
    PrepareWitnessedEscrowRequest, PreparedTransaction, RequestId, RunId, RuntimeCompatibility,
    RuntimeDescriptor, SubmissionOutcome, SubmitTransactionRequest, WitnessedNativeEscrowTerms,
    WitnessedNativeEscrowTermsInput,
};
use lez_v0_2_sidecar::{
    BridgeRuntime, BridgeRuntimeError, BridgeServerCapability, BridgeServerConfig,
    FinalizedIndexerApi, HistoricalAccount, NativeEscrowPlanner, NativePrepareError, NonceSource,
    OfficialNodeRpc, start_bridge_server,
};
use nssa::{AccountId, PrivateKey, PublicKey};

const BRIDGE_CAPABILITY: &str = "m3-native-refund-capability-000001";
const ESCROW_PROGRAM: [u32; 8] = [0x1020_3040; 8];
const TRANSFER_PROGRAM: [u32; 8] = [0x5060_7080; 8];
static TEST_DIRECTORY_SEQUENCE: AtomicUsize = AtomicUsize::new(0);

#[derive(Debug)]
struct TestDirectory(PathBuf);

impl TestDirectory {
    fn secure() -> Self {
        let sequence = TEST_DIRECTORY_SEQUENCE.fetch_add(1, Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!(
            "lez-v02-bridge-refund-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[derive(Debug)]
struct CountingNonce(AtomicUsize);

#[async_trait]
impl NonceSource for CountingNonce {
    async fn account_nonce(&self, _account_id: AccountId) -> Result<u128, NativePrepareError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(151)
    }
}

#[derive(Debug)]
struct UnusedIndexer;

#[async_trait]
impl FinalizedIndexerApi for UnusedIndexer {
    async fn last_finalized_block_id(&self) -> Result<Option<u64>, BridgeRuntimeError> {
        Err(BridgeRuntimeError::Unavailable)
    }

    async fn block_by_id(&self, _block_id: u64) -> Result<Option<Block>, BridgeRuntimeError> {
        Err(BridgeRuntimeError::Unavailable)
    }

    async fn block_by_hash(
        &self,
        _block_hash: [u8; 32],
    ) -> Result<Option<Block>, BridgeRuntimeError> {
        Err(BridgeRuntimeError::Unavailable)
    }

    async fn account_at_block(
        &self,
        _account_id: [u8; 32],
        _block_id: u64,
    ) -> Result<HistoricalAccount, BridgeRuntimeError> {
        Err(BridgeRuntimeError::Unavailable)
    }
}

fn account(byte: u8) -> (AccountId, PrivateKey, PublicKey) {
    let key = PrivateKey::try_new([byte; 32]).unwrap();
    let public = PublicKey::new_from_private_key(&key);
    (AccountId::from(&public), key, public)
}

const fn h(byte: u8) -> Hex32 {
    Hex32::from_bytes([byte; 32])
}

fn program_hex(program_id: [u32; 8]) -> Hex32 {
    let mut bytes = [0_u8; 32];
    for (chunk, word) in bytes.chunks_exact_mut(4).zip(program_id) {
        chunk.copy_from_slice(&word.to_le_bytes());
    }
    Hex32::from_bytes(bytes)
}

fn runtime(signer: AccountId) -> RuntimeDescriptor {
    RuntimeDescriptor::new(
        Participant::Maker,
        RuntimeCompatibility::LeeV0_2_0,
        h(1),
        h(2),
        h(3),
        program_hex(ESCROW_PROGRAM),
        Hex32::from_bytes(signer.into_value()),
    )
}

fn terms(
    depositor: AccountId,
    claimant: AccountId,
    authority: AccountId,
    authority_key: &PublicKey,
) -> WitnessedNativeEscrowTerms {
    WitnessedNativeEscrowTerms::new(WitnessedNativeEscrowTermsInput {
        swap_id: h(4),
        terms_hash: h(5),
        depositor: Participant::Maker,
        depositor_account_id: Hex32::from_bytes(depositor.into_value()),
        claimant: Participant::Taker,
        claimant_account_id: Hex32::from_bytes(claimant.into_value()),
        aggregate_authority_account_id: Hex32::from_bytes(authority.into_value()),
        aggregate_x_only_public_key: Hex32::from_bytes(*authority_key.value()),
        amount: 991,
        refund_at_ms: 1_850_000_000_123,
        authenticated_transfer_program_id: program_hex(TRANSFER_PROGRAM),
    })
    .unwrap()
}

fn refund_request(
    depositor: AccountId,
    terms: WitnessedNativeEscrowTerms,
) -> PrepareNativeRefundRequest {
    PrepareNativeRefundRequest::new_witnessed(
        MessageContext::new(
            RunId::new("v02-bridge-native-refund-run-0001").unwrap(),
            RequestId::new("v02-bridge-native-refund-prepare-0001").unwrap(),
            Participant::Maker,
        ),
        runtime(depositor),
        terms,
    )
}

fn witnessed_escrow_request(
    depositor: AccountId,
    terms: WitnessedNativeEscrowTerms,
) -> PrepareWitnessedEscrowRequest {
    PrepareWitnessedEscrowRequest::new(
        MessageContext::new(
            RunId::new("v02-bridge-native-refund-run-0001").unwrap(),
            RequestId::new("v02-bridge-witnessed-escrow-prepare-0001").unwrap(),
            Participant::Maker,
        ),
        runtime(depositor),
        terms,
    )
}

async fn start_sequencer() -> (String, jsonrpsee::server::ServerHandle) {
    let server = ServerBuilder::default().build("127.0.0.1:0").await.unwrap();
    let address = server.local_addr().unwrap();
    let mut rpc = RpcModule::new(());
    rpc.register_method("checkHealth", |_, (), _| Ok::<_, ErrorObjectOwned>(()))
        .unwrap();
    rpc.register_method("getChannelId", |_, (), _| {
        Ok::<_, ErrorObjectOwned>(hex::encode([2_u8; 32]))
    })
    .unwrap();
    let handle = server.start(rpc);
    (format!("http://{address}"), handle)
}

fn client(endpoint: &str, descriptor: RuntimeDescriptor) -> BridgeClient {
    BridgeClient::connect(BridgeClientConfig::new(
        endpoint,
        SidecarCapability::new(BRIDGE_CAPABILITY).unwrap(),
        RunId::new("v02-bridge-native-refund-run-0001").unwrap(),
        descriptor,
        Duration::from_secs(2),
    ))
    .unwrap()
}

#[tokio::test]
async fn authenticated_prepare_is_exact_and_restored_after_server_and_planner_restart() {
    let (depositor, signer_key, _) = account(131);
    let (claimant, _, _) = account(132);
    let (authority, _, authority_key) = account(133);
    let descriptor = runtime(depositor);
    let terms = terms(depositor, claimant, authority, &authority_key);
    let witnessed_request = witnessed_escrow_request(depositor, terms.clone());
    let request = refund_request(depositor, terms);
    let root = TestDirectory::secure();
    let planner_directory = root.path();
    let idempotency_path = root.path().join("bridge-idempotency.json");
    let (sequencer_endpoint, sequencer_handle) = start_sequencer().await;

    let first_nonces = Arc::new(CountingNonce(AtomicUsize::new(0)));
    let first_planner = Arc::new(
        NativeEscrowPlanner::new_durable(
            Participant::Maker,
            signer_key.clone(),
            ESCROW_PROGRAM,
            TRANSFER_PROGRAM,
            descriptor.clone(),
            Arc::clone(&first_nonces),
            planner_directory,
        )
        .unwrap(),
    );
    let first_runtime = Arc::new(BridgeRuntime::new(
        descriptor.clone(),
        first_planner,
        Arc::new(OfficialNodeRpc::connect(&sequencer_endpoint).unwrap()),
        Arc::new(UnusedIndexer),
    ));
    let first_server = start_bridge_server(
        BridgeServerConfig::new(
            request.context.run_id.clone(),
            BridgeServerCapability::new(BRIDGE_CAPABILITY).unwrap(),
            idempotency_path.clone(),
            SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        ),
        first_runtime,
    )
    .await
    .unwrap();

    let prepared_witnessed = client(first_server.endpoint(), descriptor.clone())
        .prepare_witnessed_escrow(witnessed_request.clone())
        .await
        .unwrap();
    assert_eq!(first_nonces.0.load(Ordering::SeqCst), 1);
    let prepared = client(first_server.endpoint(), descriptor.clone())
        .prepare_native_refund(request.clone())
        .await
        .unwrap();
    assert_eq!(first_nonces.0.load(Ordering::SeqCst), 1);
    first_server.stop().await.unwrap();

    let restarted_nonces = Arc::new(CountingNonce(AtomicUsize::new(0)));
    let restarted_planner = Arc::new(
        NativeEscrowPlanner::new_durable(
            Participant::Maker,
            signer_key,
            ESCROW_PROGRAM,
            TRANSFER_PROGRAM,
            descriptor.clone(),
            Arc::clone(&restarted_nonces),
            planner_directory,
        )
        .unwrap(),
    );
    let restarted_runtime = Arc::new(BridgeRuntime::new(
        descriptor.clone(),
        restarted_planner,
        Arc::new(OfficialNodeRpc::connect(&sequencer_endpoint).unwrap()),
        Arc::new(UnusedIndexer),
    ));
    let restarted_server = start_bridge_server(
        BridgeServerConfig::new(
            request.context.run_id.clone(),
            BridgeServerCapability::new(BRIDGE_CAPABILITY).unwrap(),
            idempotency_path,
            SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        ),
        restarted_runtime,
    )
    .await
    .unwrap();

    let replayed_witnessed = client(restarted_server.endpoint(), descriptor.clone())
        .prepare_witnessed_escrow(witnessed_request)
        .await
        .unwrap();
    assert_eq!(replayed_witnessed, prepared_witnessed);
    let replayed = client(restarted_server.endpoint(), descriptor)
        .prepare_native_refund(request)
        .await
        .unwrap();
    assert_eq!(replayed, prepared);
    assert_eq!(restarted_nonces.0.load(Ordering::SeqCst), 0);

    restarted_server.stop().await.unwrap();
    sequencer_handle.stop().unwrap();
    sequencer_handle.stopped().await;
}

/// The sequencer a refund submission meets: its exact-inclusion lookups can
/// fail (`unavailable_lookups` more of them) or be held until released, and
/// its sends are counted and can fail after they were received.
#[derive(Debug, Default)]
struct Node {
    unavailable_lookups: AtomicUsize,
    lookups: AtomicUsize,
    hold_next_lookup: AtomicBool,
    lookup_held: tokio::sync::Notify,
    release_lookup: tokio::sync::Notify,
    failing_sends: AtomicBool,
    sends: AtomicUsize,
}

impl Node {
    fn lookups(&self) -> usize {
        self.lookups.load(Ordering::SeqCst)
    }

    fn sends(&self) -> usize {
        self.sends.load(Ordering::SeqCst)
    }
}

fn node_failure() -> ErrorObjectOwned {
    ErrorObjectOwned::owned(-32_000, "node failure", None::<()>)
}

async fn start_node(node: Arc<Node>) -> (String, jsonrpsee::server::ServerHandle) {
    let server = ServerBuilder::default().build("127.0.0.1:0").await.unwrap();
    let address = server.local_addr().unwrap();
    let mut rpc = RpcModule::from_arc(node);
    rpc.register_method("checkHealth", |_, _, _| Ok::<_, ErrorObjectOwned>(()))
        .unwrap();
    rpc.register_method("getChannelId", |_, _, _| {
        Ok::<_, ErrorObjectOwned>(hex::encode([2_u8; 32]))
    })
    .unwrap();
    rpc.register_async_method("getTransaction", |_, node, _| async move {
        node.lookups.fetch_add(1, Ordering::SeqCst);
        if node.hold_next_lookup.swap(false, Ordering::SeqCst) {
            node.lookup_held.notify_one();
            node.release_lookup.notified().await;
        }
        if node
            .unavailable_lookups
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |left| {
                left.checked_sub(1)
            })
            .is_ok()
        {
            return Err(node_failure());
        }
        Ok::<Option<(LeeTransaction, u64)>, ErrorObjectOwned>(None)
    })
    .unwrap();
    rpc.register_method("sendTransaction", |params, node, _| {
        let transaction: LeeTransaction = params.one()?;
        node.sends.fetch_add(1, Ordering::SeqCst);
        if node.failing_sends.load(Ordering::SeqCst) {
            return Err(node_failure());
        }
        Ok::<HashType, ErrorObjectOwned>(transaction.hash())
    })
    .unwrap();
    let handle = server.start(rpc);
    (format!("http://{address}"), handle)
}

/// A Maker sidecar on `node` holding a prepared witnessed escrow and its
/// refund; returns the server and the refund's exact submission.
async fn sidecar_with_prepared_refund(
    root: &TestDirectory,
    node_endpoint: &str,
) -> (
    lez_v0_2_sidecar::BridgeServerHandle,
    RuntimeDescriptor,
    SubmitTransactionRequest,
) {
    let (depositor, signer_key, _) = account(141);
    let (claimant, _, _) = account(142);
    let (authority, _, authority_key) = account(143);
    let descriptor = runtime(depositor);
    let terms = terms(depositor, claimant, authority, &authority_key);
    let planner = Arc::new(
        NativeEscrowPlanner::new_durable(
            Participant::Maker,
            signer_key,
            ESCROW_PROGRAM,
            TRANSFER_PROGRAM,
            descriptor.clone(),
            Arc::new(CountingNonce(AtomicUsize::new(0))),
            root.path(),
        )
        .unwrap(),
    );
    let runtime = Arc::new(BridgeRuntime::new(
        descriptor.clone(),
        planner,
        Arc::new(OfficialNodeRpc::connect(node_endpoint).unwrap()),
        Arc::new(UnusedIndexer),
    ));
    let server = start_bridge_server(
        BridgeServerConfig::new(
            RunId::new("v02-bridge-native-refund-run-0001").unwrap(),
            BridgeServerCapability::new(BRIDGE_CAPABILITY).unwrap(),
            root.path().join("bridge-idempotency.json"),
            SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        ),
        runtime,
    )
    .await
    .unwrap();
    let _ = client(server.endpoint(), descriptor.clone())
        .prepare_witnessed_escrow(witnessed_escrow_request(depositor, terms.clone()))
        .await
        .unwrap();
    let refund: PreparedTransaction = client(server.endpoint(), descriptor.clone())
        .prepare_native_refund(refund_request(depositor, terms))
        .await
        .unwrap()
        .refund;
    let submission = SubmitTransactionRequest::new(
        MessageContext::new(
            RunId::new("v02-bridge-native-refund-run-0001").unwrap(),
            refund.transaction_id.submission_request_id(),
            Participant::Maker,
        ),
        descriptor.clone(),
        refund,
    );
    (server, descriptor, submission)
}

fn assert_remote_code<T: std::fmt::Debug>(
    result: Result<T, BridgeClientError>,
    expected: ErrorCode,
) {
    let Err(BridgeClientError::Remote(error)) = result else {
        panic!("expected remote {expected:?}, got {result:?}");
    };
    assert_eq!(error.code(), expected);
}

#[tokio::test]
async fn a_refund_refused_before_its_send_is_sent_once_under_the_same_request_id() {
    let node = Arc::new(Node::default());
    let (node_endpoint, node_handle) = start_node(Arc::clone(&node)).await;
    let root = TestDirectory::secure();
    let (server, descriptor, submission) =
        sidecar_with_prepared_refund(&root, &node_endpoint).await;

    // The exact-inclusion lookup before the send fails: nothing is sent.
    node.unavailable_lookups.store(1, Ordering::SeqCst);
    assert_remote_code(
        client(server.endpoint(), descriptor.clone())
            .submit_transaction(submission.clone())
            .await,
        ErrorCode::Unavailable,
    );
    assert_eq!(node.sends(), 0);

    // The same request id is tried again, not answered from the journal.
    let accepted = client(server.endpoint(), descriptor.clone())
        .submit_transaction(submission.clone())
        .await
        .unwrap();
    assert_eq!(accepted.outcome, SubmissionOutcome::Accepted);
    assert_eq!(node.lookups(), 2);
    assert_eq!(node.sends(), 1);

    // The acceptance is replayed as before: no lookup, no second send.
    let replayed = client(server.endpoint(), descriptor)
        .submit_transaction(submission)
        .await
        .unwrap();
    assert_eq!(replayed, accepted);
    assert_eq!(node.lookups(), 2);
    assert_eq!(node.sends(), 1);

    server.stop().await.unwrap();
    node_handle.stop().unwrap();
    node_handle.stopped().await;
}

#[tokio::test]
async fn a_refund_whose_send_was_reached_is_never_sent_again() {
    let node = Arc::new(Node::default());
    let (node_endpoint, node_handle) = start_node(Arc::clone(&node)).await;
    let root = TestDirectory::secure();
    let (server, descriptor, submission) =
        sidecar_with_prepared_refund(&root, &node_endpoint).await;

    // Refused before the send first, then the send is reached and fails:
    // the transaction may be at the sequencer.
    node.unavailable_lookups.store(1, Ordering::SeqCst);
    node.failing_sends.store(true, Ordering::SeqCst);
    assert_remote_code(
        client(server.endpoint(), descriptor.clone())
            .submit_transaction(submission.clone())
            .await,
        ErrorCode::Unavailable,
    );
    assert_remote_code(
        client(server.endpoint(), descriptor.clone())
            .submit_transaction(submission.clone())
            .await,
        ErrorCode::UnknownSubmissionOutcome,
    );
    assert_eq!(node.sends(), 1);

    // From then on the unknown outcome is replayed, also once the node
    // would take it: one send in all.
    node.failing_sends.store(false, Ordering::SeqCst);
    for _ in 0..2 {
        assert_remote_code(
            client(server.endpoint(), descriptor.clone())
                .submit_transaction(submission.clone())
                .await,
            ErrorCode::UnknownSubmissionOutcome,
        );
    }
    assert_eq!(node.lookups(), 2);
    assert_eq!(node.sends(), 1);

    server.stop().await.unwrap();
    node_handle.stop().unwrap();
    node_handle.stopped().await;
}

#[tokio::test]
async fn a_second_call_while_the_first_is_refused_before_its_send_sends_nothing() {
    let node = Arc::new(Node::default());
    let (node_endpoint, node_handle) = start_node(Arc::clone(&node)).await;
    let root = TestDirectory::secure();
    let (server, descriptor, submission) =
        sidecar_with_prepared_refund(&root, &node_endpoint).await;

    // The first call's lookup is held, then fails.
    node.hold_next_lookup.store(true, Ordering::SeqCst);
    node.unavailable_lookups.store(1, Ordering::SeqCst);
    let first = tokio::spawn({
        let client = client(server.endpoint(), descriptor.clone());
        let submission = submission.clone();
        async move { client.submit_transaction(submission).await }
    });
    node.lookup_held.notified().await;

    // The same request id meanwhile reaches neither the node nor a send (the
    // sidecar takes one connection at a time; a request that got in would
    // meet the first one's in-flight marker).
    let second = client(server.endpoint(), descriptor.clone())
        .submit_transaction(submission.clone())
        .await;
    assert!(second.is_err(), "a concurrent call is refused: {second:?}");
    assert_eq!(node.lookups(), 1);

    node.release_lookup.notify_one();
    assert_remote_code(first.await.unwrap(), ErrorCode::Unavailable);
    assert_eq!(node.sends(), 0);

    // Tried again afterwards: sent exactly once.
    let accepted = client(server.endpoint(), descriptor)
        .submit_transaction(submission)
        .await
        .unwrap();
    assert_eq!(accepted.outcome, SubmissionOutcome::Accepted);
    assert_eq!(node.sends(), 1);

    server.stop().await.unwrap();
    node_handle.stop().unwrap();
    node_handle.stopped().await;
}
