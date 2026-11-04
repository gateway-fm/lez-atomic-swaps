//! FINDINGS 24: an owned revealing claim whose signer nonce another claim of
//! the same signer took is named `Superseded` (on its first submission or on a
//! resubmission after a journaled acceptance), never broadcast, and re-planned
//! through `supersede_revealing_claim` into a durable lineage — the same claim
//! at the live nonce — that survives a restart, answers a replay with the same
//! bytes, refuses an included claim and stops by name after five re-plans.
#![cfg(target_os = "linux")]

use std::{
    fs,
    net::{Ipv4Addr, SocketAddr},
    os::unix::fs::PermissionsExt as _,
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use async_trait::async_trait;
use common::{HashType, transaction::LeeTransaction};
use indexer_service_protocol::Block;
use jsonrpsee::{RpcModule, server::ServerBuilder, types::ErrorObjectOwned};
use lez_bridge_client::{BridgeClient, BridgeClientConfig, BridgeClientError, SidecarCapability};
use lez_bridge_protocol::{
    ErrorCode, Hex32, MAX_REVEALING_CLAIM_REPLANS, MessageContext, NativeEscrowTerms,
    NativeEscrowTermsInput, Participant, PrepareRevealingClaimRequest, PreparedTransaction,
    RequestId, RevealingPreimage, RunId, RuntimeCompatibility, RuntimeDescriptor,
    SubmissionOutcome, SubmitTransactionRequest, SupersedeRevealingClaimRequest, TransactionId,
};
use lez_v0_2_sidecar::{
    BridgeRuntime, BridgeRuntimeError, BridgeServerCapability, BridgeServerConfig,
    FinalizedIndexerApi, HistoricalAccount, NativeEscrowPlanner, NativePrepareError, NonceSource,
    OfficialNodeRpc, decode_official_public_transaction, decode_prepared_for_signer,
    program_id_to_hex, start_bridge_server,
};
use nssa::{Account, AccountId, PrivateKey, PublicKey};
use sha2::{Digest as _, Sha256};
use tempfile::TempDir;

const CAPABILITY: &str = "f24-claim-supersede-capability-0001";
const RUN_ID: &str = "f24-claim-supersede-run";
const ESCROW_PROGRAM: [u32; 8] = [0x1020_3040; 8];
const TRANSFER_PROGRAM: [u32; 8] = [0x5060_7080; 8];
const FIRST_NONCE: u128 = 41;

/// The chain as the sidecar sees it: the signer's live nonce (served to the
/// planner's nonce source and `getAccount`), the included transactions, and
/// counters for sends and lookups.
#[derive(Clone)]
struct Chain {
    live_nonce: Arc<Mutex<u128>>,
    nonce_calls: Arc<AtomicUsize>,
    sends: Arc<AtomicUsize>,
    included: Arc<Mutex<Vec<LeeTransaction>>>,
}

impl Chain {
    fn new() -> Self {
        Self {
            live_nonce: Arc::new(Mutex::new(FIRST_NONCE)),
            nonce_calls: Arc::new(AtomicUsize::new(0)),
            sends: Arc::new(AtomicUsize::new(0)),
            included: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn set_live_nonce(&self, nonce: u128) {
        *self.live_nonce.lock().expect("nonce lock") = nonce;
    }

    fn include_exact(&self, prepared: &PreparedTransaction) {
        let transaction = decode_official_public_transaction(prepared.exact_bytes.as_slice())
            .expect("official prepared transaction");
        self.included
            .lock()
            .expect("included lock")
            .push(LeeTransaction::Public(transaction));
    }

    fn send_count(&self) -> usize {
        self.sends.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl NonceSource for Chain {
    async fn account_nonce(&self, _account_id: AccountId) -> Result<u128, NativePrepareError> {
        self.nonce_calls.fetch_add(1, Ordering::SeqCst);
        Ok(*self.live_nonce.lock().expect("nonce lock"))
    }
}

#[derive(Debug)]
struct UnavailableIndexer;

#[async_trait]
impl FinalizedIndexerApi for UnavailableIndexer {
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

async fn start_sequencer(chain: Chain) -> (String, jsonrpsee::server::ServerHandle) {
    let server = ServerBuilder::default()
        .build("127.0.0.1:0")
        .await
        .expect("sequencer binds");
    let address = server.local_addr().expect("sequencer address");
    let mut rpc = RpcModule::new(chain);
    rpc.register_method("checkHealth", |_, _, _| Ok::<_, ErrorObjectOwned>(()))
        .expect("health method");
    rpc.register_method("getChannelId", |_, _, _| {
        Ok::<_, ErrorObjectOwned>(hex::encode([41_u8; 32]))
    })
    .expect("channel method");
    rpc.register_method("getAccount", |_params, chain, _| {
        let account = Account {
            balance: u128::MAX / 2,
            nonce: (*chain.live_nonce.lock().expect("nonce lock")).into(),
            ..Default::default()
        };
        Ok::<_, ErrorObjectOwned>(account)
    })
    .expect("account method");
    rpc.register_method("getTransaction", |params, chain, _| {
        let requested: HashType = params.one()?;
        let observed = chain
            .included
            .lock()
            .expect("included lock")
            .iter()
            .find(|transaction| transaction.hash() == requested)
            .cloned()
            .map(|transaction| (transaction, 1_u64));
        Ok::<_, ErrorObjectOwned>(observed)
    })
    .expect("transaction lookup method");
    rpc.register_method("sendTransaction", |params, chain, _| {
        let transaction: LeeTransaction = params.one()?;
        chain.sends.fetch_add(1, Ordering::SeqCst);
        Ok::<_, ErrorObjectOwned>(transaction.hash())
    })
    .expect("send method");
    (format!("http://{address}"), server.start(rpc))
}

fn account(byte: u8) -> (AccountId, PrivateKey) {
    let key = PrivateKey::try_new([byte; 32]).expect("private key");
    let account = AccountId::from(&PublicKey::new_from_private_key(&key));
    (account, key)
}

const fn h(byte: u8) -> Hex32 {
    Hex32::from_bytes([byte; 32])
}

fn runtime(signer: AccountId) -> RuntimeDescriptor {
    RuntimeDescriptor::new(
        Participant::Taker,
        RuntimeCompatibility::LeeV0_2_0,
        h(40),
        h(41),
        h(42),
        program_id_to_hex(ESCROW_PROGRAM),
        Hex32::from_bytes(signer.into_value()),
    )
}

fn context(request_id: &str) -> MessageContext {
    MessageContext::new(
        RunId::new(RUN_ID).expect("run id"),
        RequestId::new(request_id).expect("request id"),
        Participant::Taker,
    )
}

fn submission_context(transaction_id: TransactionId) -> MessageContext {
    MessageContext::new(
        RunId::new(RUN_ID).expect("run id"),
        transaction_id.submission_request_id(),
        Participant::Taker,
    )
}

fn submit_request(signer: AccountId, prepared: &PreparedTransaction) -> SubmitTransactionRequest {
    SubmitTransactionRequest::new(
        submission_context(prepared.transaction_id),
        runtime(signer),
        prepared.clone(),
    )
}

fn nonce_of(prepared: &PreparedTransaction, signer: AccountId) -> u128 {
    let transaction = decode_prepared_for_signer(prepared, signer).expect("signer transaction");
    let [nonce] = transaction.message().nonces.as_slice() else {
        panic!("one signer nonce");
    };
    u128::from(*nonce)
}

struct Sidecar {
    server: lez_v0_2_sidecar::BridgeServerHandle,
    descriptor: RuntimeDescriptor,
}

impl Sidecar {
    fn client(&self) -> BridgeClient {
        BridgeClient::connect(BridgeClientConfig::new(
            self.server.endpoint(),
            SidecarCapability::new(CAPABILITY).expect("client capability"),
            RunId::new(RUN_ID).expect("run id"),
            self.descriptor.clone(),
            Duration::from_secs(2),
        ))
        .expect("bridge client")
    }
}

fn secure_directory() -> TempDir {
    let directory = TempDir::new().expect("sidecar directory");
    fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700))
        .expect("private directory");
    let planner_directory = directory.path().join("planner");
    fs::create_dir(&planner_directory).expect("planner directory");
    fs::set_permissions(&planner_directory, fs::Permissions::from_mode(0o700))
        .expect("private planner directory");
    directory
}

async fn start_sidecar(
    signer: AccountId,
    signer_key: PrivateKey,
    sequencer_endpoint: &str,
    directory: &Path,
    chain: &Chain,
) -> Sidecar {
    let descriptor = runtime(signer);
    let planner = Arc::new(
        NativeEscrowPlanner::new_durable(
            Participant::Taker,
            signer_key,
            ESCROW_PROGRAM,
            TRANSFER_PROGRAM,
            descriptor.clone(),
            Arc::new(chain.clone()),
            directory.join("planner"),
        )
        .expect("planner"),
    );
    let runtime = Arc::new(BridgeRuntime::new(
        descriptor.clone(),
        planner,
        Arc::new(OfficialNodeRpc::connect(sequencer_endpoint).expect("official node")),
        Arc::new(UnavailableIndexer),
    ));
    let server = start_bridge_server(
        BridgeServerConfig::new(
            RunId::new(RUN_ID).expect("run id"),
            BridgeServerCapability::new(CAPABILITY).expect("server capability"),
            directory.join("bridge-idempotency.json"),
            SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        ),
        runtime,
    )
    .await
    .expect("sidecar starts");
    Sidecar { server, descriptor }
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

fn supersession_files(directory: &Path) -> Vec<String> {
    let mut names = fs::read_dir(directory.join("planner"))
        .expect("planner directory listing")
        .map(|entry| {
            entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .filter(|name| name.starts_with("native-claim-supersession."))
        .collect::<Vec<_>>();
    names.sort();
    names
}

const PREIMAGE: [u8; 32] = [0x5a; 32];

fn claim_request(signer: AccountId, depositor: AccountId) -> PrepareRevealingClaimRequest {
    let digest: [u8; 32] = Sha256::digest(PREIMAGE).into();
    let terms = NativeEscrowTerms::new(NativeEscrowTermsInput {
        swap_id: h(4),
        terms_hash: h(5),
        secret_digest: Hex32::from_bytes(digest),
        depositor: Participant::Maker,
        depositor_account_id: Hex32::from_bytes(depositor.into_value()),
        claimant: Participant::Taker,
        claimant_account_id: Hex32::from_bytes(signer.into_value()),
        amount: 75_000,
        refund_at_ms: 1_850_000_000_123,
        authenticated_transfer_program_id: program_id_to_hex(TRANSFER_PROGRAM),
    })
    .expect("terms");
    PrepareRevealingClaimRequest::new(
        context("prepare-revealing-claim"),
        runtime(signer),
        terms,
        TransactionId::from_bytes([9; 32]),
        RevealingPreimage::new(PREIMAGE),
    )
}

fn supersede(
    signer: AccountId,
    request_id: &str,
    superseded: TransactionId,
) -> SupersedeRevealingClaimRequest {
    SupersedeRevealingClaimRequest::new(context(request_id), runtime(signer), superseded)
}

async fn prepare_claim(
    sidecar: &Sidecar,
    signer: AccountId,
    depositor: AccountId,
) -> PreparedTransaction {
    sidecar
        .client()
        .prepare_revealing_claim(claim_request(signer, depositor))
        .await
        .expect("prepare revealing claim")
        .claim
}

#[tokio::test]
#[allow(
    clippy::too_many_lines,
    reason = "one claim lineage is followed end to end"
)]
async fn a_taken_claim_nonce_is_superseded_replanned_and_survives_a_restart() {
    let (signer, key) = account(51);
    let (depositor, _) = account(52);
    let chain = Chain::new();
    let (endpoint, sequencer) = start_sequencer(chain.clone()).await;
    let directory = secure_directory();
    let sidecar = start_sidecar(signer, key.clone(), &endpoint, directory.path(), &chain).await;

    let original = prepare_claim(&sidecar, signer, depositor).await;
    assert_eq!(nonce_of(&original, signer), FIRST_NONCE);

    // Another claim of this signer took nonce 41 first.
    chain.set_live_nonce(FIRST_NONCE + 1);
    let submitted = sidecar
        .client()
        .submit_transaction(submit_request(signer, &original))
        .await
        .expect("submission answered");
    assert_eq!(
        submitted.outcome,
        SubmissionOutcome::Superseded {
            planned_nonce: FIRST_NONCE,
            live_nonce: FIRST_NONCE + 1,
        }
    );
    assert_eq!(
        chain.send_count(),
        0,
        "superseded claim bytes are never broadcast"
    );

    // The same claim (terms, escrow, preimage) at the live nonce.
    let replanned = sidecar
        .client()
        .supersede_revealing_claim(supersede(
            signer,
            "claim-supersede-1",
            original.transaction_id,
        ))
        .await
        .expect("claim re-planned");
    assert_eq!(replanned.superseded_nonce, FIRST_NONCE);
    assert_eq!(replanned.replanned_nonce, FIRST_NONCE + 1);
    assert_eq!(replanned.replan_index, 1);
    assert_eq!(nonce_of(&replanned.claim, signer), FIRST_NONCE + 1);
    assert_ne!(replanned.claim.transaction_id, original.transaction_id);
    let original_message = decode_prepared_for_signer(&original, signer).expect("original");
    let replanned_message =
        decode_prepared_for_signer(&replanned.claim, signer).expect("re-planned");
    assert_eq!(
        original_message.message().instruction_data,
        replanned_message.message().instruction_data,
        "the re-planned claim carries the same instruction (same preimage, same escrow)"
    );
    assert_eq!(
        supersession_files(directory.path()),
        ["native-claim-supersession.1.v1.json"]
    );

    // A replay naming the same superseded claim answers the same bytes.
    let replay = sidecar
        .client()
        .supersede_revealing_claim(supersede(
            signer,
            "claim-supersede-1-again",
            original.transaction_id,
        ))
        .await
        .expect("replayed supersession");
    assert_eq!(replay.claim, replanned.claim);

    let accepted = sidecar
        .client()
        .submit_transaction(submit_request(signer, &replanned.claim))
        .await
        .expect("re-planned claim submitted");
    assert_eq!(accepted.outcome, SubmissionOutcome::Accepted);
    assert_eq!(chain.send_count(), 1);

    // After a restart the lineage is recovered with the claim preparation.
    sidecar.server.stop().await.expect("stop");
    let restarted = start_sidecar(signer, key, &endpoint, directory.path(), &chain).await;
    assert_eq!(prepare_claim(&restarted, signer, depositor).await, original);
    let recovered = restarted
        .client()
        .supersede_revealing_claim(supersede(
            signer,
            "claim-supersede-after-restart",
            original.transaction_id,
        ))
        .await
        .expect("recovered supersession");
    assert_eq!(recovered.claim, replanned.claim);

    restarted.server.stop().await.expect("stop");
    sequencer.stop().expect("sequencer stop");
    sequencer.stopped().await;
}

#[tokio::test]
async fn a_claim_accepted_once_is_decided_again_and_an_included_claim_is_never_replanned() {
    let (signer, key) = account(53);
    let (depositor, _) = account(54);
    let chain = Chain::new();
    let (endpoint, sequencer) = start_sequencer(chain.clone()).await;
    let directory = secure_directory();
    let sidecar = start_sidecar(signer, key, &endpoint, directory.path(), &chain).await;
    let original = prepare_claim(&sidecar, signer, depositor).await;

    // Accepted at nonce 41 (another claim was accepted at 41 too).
    let accepted = sidecar
        .client()
        .submit_transaction(submit_request(signer, &original))
        .await
        .expect("accepted");
    assert_eq!(accepted.outcome, SubmissionOutcome::Accepted);
    // The other claim landed; this one cannot. The resubmission is decided
    // on the chain again, not replayed from the journal.
    chain.set_live_nonce(FIRST_NONCE + 1);
    let decided = sidecar
        .client()
        .submit_transaction(submit_request(signer, &original))
        .await
        .expect("decided again");
    assert_eq!(
        decided.outcome,
        SubmissionOutcome::Superseded {
            planned_nonce: FIRST_NONCE,
            live_nonce: FIRST_NONCE + 1,
        }
    );
    assert_eq!(chain.send_count(), 1);

    // Had it landed, it is already known and never re-planned.
    chain.include_exact(&original);
    let known = sidecar
        .client()
        .submit_transaction(submit_request(signer, &original))
        .await
        .expect("already known");
    assert_eq!(known.outcome, SubmissionOutcome::AlreadyKnown);
    assert_remote_code(
        sidecar
            .client()
            .supersede_revealing_claim(supersede(
                signer,
                "claim-supersede-included",
                original.transaction_id,
            ))
            .await,
        ErrorCode::InvalidRequest,
    );
    assert!(supersession_files(directory.path()).is_empty());

    sidecar.server.stop().await.expect("stop");
    sequencer.stop().expect("sequencer stop");
    sequencer.stopped().await;
}

#[tokio::test]
async fn the_sixth_claim_replan_is_refused_by_name() {
    let (signer, key) = account(55);
    let (depositor, _) = account(56);
    let chain = Chain::new();
    let (endpoint, sequencer) = start_sequencer(chain.clone()).await;
    let directory = secure_directory();
    let sidecar = start_sidecar(signer, key, &endpoint, directory.path(), &chain).await;
    let mut current = prepare_claim(&sidecar, signer, depositor).await;
    for index in 1..=MAX_REVEALING_CLAIM_REPLANS {
        chain.set_live_nonce(FIRST_NONCE + u128::from(index));
        let replanned = sidecar
            .client()
            .supersede_revealing_claim(supersede(
                signer,
                &format!("claim-supersede-{index}"),
                current.transaction_id,
            ))
            .await
            .expect("bounded re-plan");
        assert_eq!(replanned.replan_index, index);
        current = replanned.claim;
    }
    chain.set_live_nonce(FIRST_NONCE + 10);
    assert_remote_code(
        sidecar
            .client()
            .supersede_revealing_claim(supersede(
                signer,
                "claim-supersede-6",
                current.transaction_id,
            ))
            .await,
        ErrorCode::ReplanExhausted,
    );
    assert_eq!(supersession_files(directory.path()).len(), 5);

    sidecar.server.stop().await.expect("stop");
    sequencer.stop().expect("sequencer stop");
    sequencer.stopped().await;
}
