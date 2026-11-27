#![cfg(any(target_os = "linux", target_os = "macos"))]
//! #174: the BTC actor's completed witnessed claim (the Taker's revealing
//! claim when it sells BTC, the Maker's follow-up claim when the Taker sells
//! LEZ) is decided on the chain again whenever its actor asks for the same
//! bytes under the same request id: looked up, and sent once more when it is
//! not included. A journaled answer replayed instead would strand a claim
//! that never reached the sequencer.

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
use borsh::BorshDeserialize as _;
use common::{HashType, transaction::LeeTransaction};
use indexer_service_protocol::Block;
use jsonrpsee::{RpcModule, server::ServerBuilder, types::ErrorObjectOwned};
use lez_bridge_client::{BridgeClient, BridgeClientConfig, BridgeClientError, SidecarCapability};
use lez_bridge_protocol::{
    AggregateBip340Signature, CompleteWitnessedClaimRequest, ErrorCode, Hex32, MessageContext,
    Participant, PrepareWitnessedClaimRequest, RequestId, RunId, RuntimeCompatibility,
    RuntimeDescriptor, SubmissionOutcome, SubmitTransactionRequest, TransactionId,
    WitnessedNativeEscrowTerms, WitnessedNativeEscrowTermsInput,
};
use lez_v0_2_sidecar::{
    BridgeRuntime, BridgeRuntimeError, BridgeServerCapability, BridgeServerConfig,
    FinalizedIndexerApi, HistoricalAccount, NativeEscrowPlanner, NativePrepareError, NonceSource,
    OfficialNodeRpc, start_bridge_server,
};
use nssa::{AccountId, PrivateKey, PublicKey, Signature, public_transaction::Message};

const BRIDGE_CAPABILITY: &str = "m5-witnessed-claim-capability-000001";
const RUN_ID: &str = "v02-bridge-witnessed-claim-run-0001";
const ESCROW_PROGRAM: [u32; 8] = [0x1020_3040; 8];
const TRANSFER_PROGRAM: [u32; 8] = [0x5060_7080; 8];
static TEST_DIRECTORY_SEQUENCE: AtomicUsize = AtomicUsize::new(0);

#[derive(Debug)]
struct TestDirectory(PathBuf);

impl TestDirectory {
    fn secure() -> Self {
        let sequence = TEST_DIRECTORY_SEQUENCE.fetch_add(1, Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!(
            "lez-v02-bridge-witnessed-claim-{}-{sequence}",
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

/// The aggregate authority's account nonce, read once at preparation.
#[derive(Debug)]
struct AuthorityNonce;

#[async_trait]
impl NonceSource for AuthorityNonce {
    async fn account_nonce(&self, _account_id: AccountId) -> Result<u128, NativePrepareError> {
        Ok(0)
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

fn run_id() -> RunId {
    RunId::new(RUN_ID).unwrap()
}

fn context(request_id: &str) -> MessageContext {
    MessageContext::new(
        run_id(),
        RequestId::new(request_id).unwrap(),
        Participant::Maker,
    )
}

fn client(endpoint: &str, descriptor: RuntimeDescriptor) -> BridgeClient {
    BridgeClient::connect(BridgeClientConfig::new(
        endpoint,
        SidecarCapability::new(BRIDGE_CAPABILITY).unwrap(),
        run_id(),
        descriptor,
        Duration::from_secs(2),
    ))
    .unwrap()
}

/// The sequencer a claim submission meets: its exact-inclusion lookups can
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
    /// What the lookup finds: the transaction the node holds under the
    /// requested hash (included after a send) or another one (a definite
    /// refusal: the sidecar reads a different transaction there).
    included: std::sync::Mutex<Option<LeeTransaction>>,
    included_on_send: AtomicBool,
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
        let found = node
            .included
            .lock()
            .unwrap()
            .clone()
            .map(|transaction| (transaction, 1_u64));
        Ok::<Option<(LeeTransaction, u64)>, ErrorObjectOwned>(found)
    })
    .unwrap();
    rpc.register_method("sendTransaction", |params, node, _| {
        let transaction: LeeTransaction = params.one()?;
        node.sends.fetch_add(1, Ordering::SeqCst);
        if node.failing_sends.load(Ordering::SeqCst) {
            return Err(node_failure());
        }
        let hash = transaction.hash();
        if node.included_on_send.load(Ordering::SeqCst) {
            *node.included.lock().unwrap() = Some(transaction);
        }
        Ok::<HashType, ErrorObjectOwned>(hash)
    })
    .unwrap();
    let handle = server.start(rpc);
    (format!("http://{address}"), handle)
}

/// The Maker (claimant) sidecar on the state in `directory` (a fresh one,
/// or one a stopped sidecar left).
async fn start_claim_sidecar(
    directory: &Path,
    node_endpoint: &str,
) -> (lez_v0_2_sidecar::BridgeServerHandle, RuntimeDescriptor) {
    let (claimant, signer_key, _) = account(161);
    let descriptor = runtime(claimant);
    let planner = Arc::new(
        NativeEscrowPlanner::new_durable(
            Participant::Maker,
            signer_key,
            ESCROW_PROGRAM,
            TRANSFER_PROGRAM,
            descriptor.clone(),
            Arc::new(AuthorityNonce),
            directory,
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
            run_id(),
            BridgeServerCapability::new(BRIDGE_CAPABILITY).unwrap(),
            directory.join("bridge-idempotency.json"),
            SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        ),
        runtime,
    )
    .await
    .unwrap();
    (server, descriptor)
}

fn prepare_request(descriptor: &RuntimeDescriptor) -> PrepareWitnessedClaimRequest {
    let (claimant, _, _) = account(161);
    let (authority, _, authority_key) = account(163);
    let terms = WitnessedNativeEscrowTerms::new(WitnessedNativeEscrowTermsInput {
        swap_id: h(4),
        terms_hash: h(5),
        depositor: Participant::Taker,
        depositor_account_id: h(53),
        claimant: Participant::Maker,
        claimant_account_id: Hex32::from_bytes(claimant.into_value()),
        aggregate_authority_account_id: Hex32::from_bytes(authority.into_value()),
        aggregate_x_only_public_key: Hex32::from_bytes(*authority_key.value()),
        amount: 75,
        refund_at_ms: 1_850_000_000_123,
        authenticated_transfer_program_id: program_hex(TRANSFER_PROGRAM),
    })
    .unwrap();
    PrepareWitnessedClaimRequest::new(
        context("v02-bridge-witnessed-claim-prepare-0001"),
        descriptor.clone(),
        terms,
        TransactionId::from_bytes([54; 32]),
    )
}

/// Prepares the claim and completes it with the aggregate authority's
/// signature, as every drive of the actor does before it observes or sends;
/// returns the actor's exact submission, whose request id never changes.
async fn complete_claim(
    endpoint: &str,
    descriptor: &RuntimeDescriptor,
) -> SubmitTransactionRequest {
    complete_claim_with(endpoint, descriptor, None).await.0
}

/// As [`complete_claim`], with the signature of an earlier completion when
/// one is given (the actor derives the same signature on every drive), and
/// returning the signature used.
async fn complete_claim_with(
    endpoint: &str,
    descriptor: &RuntimeDescriptor,
    earlier_signature: Option<[u8; 64]>,
) -> (SubmitTransactionRequest, [u8; 64]) {
    let (_, authority_private_key, _) = account(163);
    let prepared = client(endpoint, descriptor.clone())
        .prepare_witnessed_claim(prepare_request(descriptor))
        .await
        .unwrap();
    let message = Message::try_from_slice(prepared.claim.exact_message_bytes.as_slice()).unwrap();
    let signature = earlier_signature
        .unwrap_or_else(|| Signature::new(&authority_private_key, &message.hash()).value);
    let completed = client(endpoint, descriptor.clone())
        .complete_witnessed_claim(CompleteWitnessedClaimRequest::new(
            context("v02-bridge-witnessed-claim-complete-0001"),
            descriptor.clone(),
            prepared.claim,
            AggregateBip340Signature::from_bytes(signature),
        ))
        .await
        .unwrap();
    (
        SubmitTransactionRequest::new(
            context("v02-bridge-witnessed-claim-submit-0001"),
            descriptor.clone(),
            completed.claim,
        ),
        signature,
    )
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

/// Failure before the send: the exact-inclusion lookup fails, nothing is
/// sent, and the same request id is looked up and sent on the next call.
#[tokio::test]
async fn a_witnessed_claim_refused_before_its_send_is_sent_under_the_same_request_id() {
    let node = Arc::new(Node::default());
    let (node_endpoint, node_handle) = start_node(Arc::clone(&node)).await;
    let root = TestDirectory::secure();
    let (server, descriptor) = start_claim_sidecar(root.path(), &node_endpoint).await;
    let submission = complete_claim(server.endpoint(), &descriptor).await;

    node.unavailable_lookups.store(1, Ordering::SeqCst);
    assert_remote_code(
        client(server.endpoint(), descriptor.clone())
            .submit_transaction(submission.clone())
            .await,
        ErrorCode::Unavailable,
    );
    assert_eq!(node.sends(), 0);

    let accepted = client(server.endpoint(), descriptor)
        .submit_transaction(submission)
        .await
        .unwrap();
    assert_eq!(accepted.outcome, SubmissionOutcome::Accepted);
    assert_eq!(node.lookups(), 2);
    assert_eq!(node.sends(), 1);

    server.stop().await.unwrap();
    node_handle.stop().unwrap();
    node_handle.stopped().await;
}

/// Ambiguous outcome after the send: the send reached the node and failed,
/// so the claim may or may not be at the sequencer. Asked for again, the same
/// bytes are looked up and sent again; an acceptance is decided again as
/// well, until the claim is included and answered as already known.
#[tokio::test]
async fn a_witnessed_claim_with_an_unknown_outcome_is_decided_on_the_chain_again() {
    let node = Arc::new(Node::default());
    let (node_endpoint, node_handle) = start_node(Arc::clone(&node)).await;
    let root = TestDirectory::secure();
    let (server, descriptor) = start_claim_sidecar(root.path(), &node_endpoint).await;
    let submission = complete_claim(server.endpoint(), &descriptor).await;

    node.failing_sends.store(true, Ordering::SeqCst);
    assert_remote_code(
        client(server.endpoint(), descriptor.clone())
            .submit_transaction(submission.clone())
            .await,
        ErrorCode::UnknownSubmissionOutcome,
    );
    assert_eq!(node.sends(), 1);

    node.failing_sends.store(false, Ordering::SeqCst);
    let accepted = client(server.endpoint(), descriptor.clone())
        .submit_transaction(submission.clone())
        .await
        .unwrap();
    assert_eq!(accepted.outcome, SubmissionOutcome::Accepted);
    assert_eq!(node.lookups(), 2);
    assert_eq!(node.sends(), 2, "the unknown outcome is not replayed");

    // The actor asks again only while its own journal holds no acceptance
    // (the reply was lost): the sequencer dropped the claim, so it goes out
    // once more; included, it is already known and never sent again.
    node.included_on_send.store(true, Ordering::SeqCst);
    let again = client(server.endpoint(), descriptor.clone())
        .submit_transaction(submission.clone())
        .await
        .unwrap();
    assert_eq!(again.outcome, SubmissionOutcome::Accepted);
    assert_eq!(node.sends(), 3);
    let known = client(server.endpoint(), descriptor)
        .submit_transaction(submission)
        .await
        .unwrap();
    assert_eq!(known.outcome, SubmissionOutcome::AlreadyKnown);
    assert_eq!(node.sends(), 3);

    server.stop().await.unwrap();
    node_handle.stop().unwrap();
    node_handle.stopped().await;
}

fn copy_state_directory(from: &Path, to: &Path) {
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            fs::create_dir(&target).unwrap();
            fs::set_permissions(&target, fs::Permissions::from_mode(0o700)).unwrap();
            copy_state_directory(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), &target).unwrap();
        }
    }
}

/// A sidecar killed while the claim's submission is in flight leaves its
/// in-flight marker; after the restart the actor completes the claim again
/// (an exact replay) and its same submission is looked up and sent.
#[tokio::test]
async fn a_witnessed_claim_in_flight_at_a_kill_is_decided_again_after_the_restart() {
    let node = Arc::new(Node::default());
    let (node_endpoint, node_handle) = start_node(Arc::clone(&node)).await;
    let root = TestDirectory::secure();
    let (server, descriptor) = start_claim_sidecar(root.path(), &node_endpoint).await;
    let (submission, signature) = complete_claim_with(server.endpoint(), &descriptor, None).await;

    node.hold_next_lookup.store(true, Ordering::SeqCst);
    let held = tokio::spawn({
        let client = client(server.endpoint(), descriptor.clone());
        let submission = submission.clone();
        async move { client.submit_transaction(submission).await }
    });
    node.lookup_held.notified().await;
    let killed = TestDirectory::secure();
    copy_state_directory(root.path(), killed.path());
    assert_eq!(node.sends(), 0);

    let (restarted, descriptor) = start_claim_sidecar(killed.path(), &node_endpoint).await;
    assert_eq!(
        complete_claim_with(restarted.endpoint(), &descriptor, Some(signature))
            .await
            .0,
        submission
    );
    let accepted = client(restarted.endpoint(), descriptor)
        .submit_transaction(submission)
        .await
        .unwrap();
    assert_eq!(accepted.outcome, SubmissionOutcome::Accepted);
    assert_eq!(node.lookups(), 2);
    assert_eq!(node.sends(), 1);

    restarted.stop().await.unwrap();
    held.abort();
    node.release_lookup.notify_one();
    drop(server);
    node_handle.stop().unwrap();
}
