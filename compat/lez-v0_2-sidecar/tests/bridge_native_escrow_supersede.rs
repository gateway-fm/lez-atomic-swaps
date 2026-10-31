//! D1: a native escrow transaction whose signer nonce the account has passed
//! is named `Superseded`, never broadcast; the pair is re-planned through
//! `supersede_native_escrow_prepare` into a durable lineage that survives a
//! restart and answers a replay with the same bytes.
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
    ErrorCode, Hex32, MAX_NATIVE_ESCROW_REPLANS, MessageContext, NativeEscrowStep,
    NativeEscrowTerms, NativeEscrowTermsInput, Participant, PrepareNativeEscrowRequest,
    PrepareNativeEscrowResult, PreparedTransaction, RequestId, RunId, RuntimeCompatibility,
    RuntimeDescriptor, SubmissionOutcome, SubmitTransactionRequest,
    SupersedeNativeEscrowPrepareRequest, SupersedeNativeEscrowPrepareResult, TransactionId,
};
use lez_v0_2_sidecar::{
    BridgeRuntime, BridgeRuntimeError, BridgeServerCapability, BridgeServerConfig,
    FinalizedIndexerApi, HistoricalAccount, NativeEscrowPlanner, NativePrepareError, NonceSource,
    OfficialNodeRpc, decode_official_public_transaction, decode_prepared_for_signer,
    program_id_to_hex, start_bridge_server,
};
use nssa::{Account, AccountId, PrivateKey, PublicKey};
use tempfile::TempDir;

const CAPABILITY: &str = "d1-supersede-sidecar-capability-0001";
const RUN_ID: &str = "d1-supersede-sidecar-run";
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

    fn nonce_calls(&self) -> usize {
        self.nonce_calls.load(Ordering::SeqCst)
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

fn prepare_request(signer: AccountId, claimant: AccountId) -> PrepareNativeEscrowRequest {
    let terms = NativeEscrowTerms::new(NativeEscrowTermsInput {
        swap_id: h(4),
        terms_hash: h(5),
        secret_digest: h(6),
        depositor: Participant::Taker,
        depositor_account_id: Hex32::from_bytes(signer.into_value()),
        claimant: Participant::Maker,
        claimant_account_id: Hex32::from_bytes(claimant.into_value()),
        amount: 75_000,
        refund_at_ms: 1_850_000_000_123,
        authenticated_transfer_program_id: program_id_to_hex(TRANSFER_PROGRAM),
    })
    .expect("terms");
    PrepareNativeEscrowRequest::new(context("prepare-native-escrow"), runtime(signer), terms)
}

fn supersede_request(
    signer: AccountId,
    request_id: &str,
    superseded: (TransactionId, TransactionId),
    step: NativeEscrowStep,
) -> SupersedeNativeEscrowPrepareRequest {
    SupersedeNativeEscrowPrepareRequest::new(
        context(request_id),
        runtime(signer),
        superseded.0,
        superseded.1,
        step,
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

const fn ids(
    initialization: &PreparedTransaction,
    funding: &PreparedTransaction,
) -> (TransactionId, TransactionId) {
    (initialization.transaction_id, funding.transaction_id)
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
        .filter(|name| name.starts_with("native-escrow-supersession."))
        .collect::<Vec<_>>();
    names.sort();
    names
}

async fn prepare(
    sidecar: &Sidecar,
    signer: AccountId,
    claimant: AccountId,
) -> PrepareNativeEscrowResult {
    sidecar
        .client()
        .prepare_native_escrow(prepare_request(signer, claimant))
        .await
        .expect("prepare native escrow pair")
}

#[tokio::test]
async fn a_passed_nonce_is_named_superseded_and_never_sent() {
    let (signer, key) = account(31);
    let (claimant, _) = account(32);
    let chain = Chain::new();
    let (endpoint, sequencer) = start_sequencer(chain.clone()).await;
    let directory = secure_directory();
    let sidecar = start_sidecar(signer, key, &endpoint, directory.path(), &chain).await;

    let pair = prepare(&sidecar, signer, claimant).await;
    assert_eq!(nonce_of(&pair.initialization, signer), FIRST_NONCE);
    assert_eq!(nonce_of(&pair.funding, signer), FIRST_NONCE + 1);

    // Another transaction of this signer took nonces 41 and 42 first.
    chain.set_live_nonce(FIRST_NONCE + 2);
    let submitted = sidecar
        .client()
        .submit_transaction(submit_request(signer, &pair.initialization))
        .await
        .expect("submission answered");
    assert_eq!(submitted.transaction_id, pair.initialization.transaction_id);
    assert_eq!(
        submitted.outcome,
        SubmissionOutcome::Superseded {
            planned_nonce: FIRST_NONCE,
            live_nonce: FIRST_NONCE + 2,
        }
    );
    assert_eq!(
        chain.send_count(),
        0,
        "superseded bytes are never broadcast"
    );

    // The journal answers the replay of the same submission the same way.
    let replayed = sidecar
        .client()
        .submit_transaction(submit_request(signer, &pair.initialization))
        .await
        .expect("replayed submission");
    assert_eq!(replayed, submitted);
    assert_eq!(chain.send_count(), 0);

    // A live nonce that merely equals the plan is not a supersession.
    let (other_signer, other_key) = account(33);
    let other_chain = Chain::new();
    let (other_endpoint, other_sequencer) = start_sequencer(other_chain.clone()).await;
    let other_directory = secure_directory();
    let other = start_sidecar(
        other_signer,
        other_key,
        &other_endpoint,
        other_directory.path(),
        &other_chain,
    )
    .await;
    let other_pair = prepare(&other, other_signer, claimant).await;
    let accepted = other
        .client()
        .submit_transaction(submit_request(other_signer, &other_pair.initialization))
        .await
        .expect("accepted submission");
    assert_eq!(accepted.outcome, SubmissionOutcome::Accepted);
    assert_eq!(other_chain.send_count(), 1);

    sidecar.server.stop().await.expect("stop");
    other.server.stop().await.expect("stop");
    sequencer.stop().expect("sequencer stop");
    other_sequencer.stop().expect("sequencer stop");
    sequencer.stopped().await;
    other_sequencer.stopped().await;
}

#[tokio::test]
#[allow(clippy::too_many_lines, reason = "one lineage is followed end to end")]
async fn fund_supersession_keeps_the_initialization_and_replays_identically() {
    let (signer, key) = account(31);
    let (claimant, _) = account(32);
    let chain = Chain::new();
    let (endpoint, sequencer) = start_sequencer(chain.clone()).await;
    let directory = secure_directory();
    let sidecar = start_sidecar(signer, key, &endpoint, directory.path(), &chain).await;

    let pair = prepare(&sidecar, signer, claimant).await;
    let initialization = sidecar
        .client()
        .submit_transaction(submit_request(signer, &pair.initialization))
        .await
        .expect("initialization submitted");
    assert_eq!(initialization.outcome, SubmissionOutcome::Accepted);
    chain.include_exact(&pair.initialization);
    // Nonce 42 (the planned funding) and 43 went to other swaps of this signer.
    chain.set_live_nonce(FIRST_NONCE + 3);

    let funding = sidecar
        .client()
        .submit_transaction(submit_request(signer, &pair.funding))
        .await
        .expect("funding answered");
    assert_eq!(
        funding.outcome,
        SubmissionOutcome::Superseded {
            planned_nonce: FIRST_NONCE + 1,
            live_nonce: FIRST_NONCE + 3,
        }
    );
    assert_eq!(chain.send_count(), 1);

    // Naming the wrong step is refused before any nonce is read.
    let nonce_calls = chain.nonce_calls();
    assert_remote_code(
        sidecar
            .client()
            .supersede_native_escrow_prepare(supersede_request(
                signer,
                "supersede-wrong-step",
                ids(&pair.initialization, &pair.funding),
                NativeEscrowStep::Initialize,
            ))
            .await,
        ErrorCode::InvalidRequest,
    );
    assert_eq!(chain.nonce_calls(), nonce_calls);
    // Naming a pair that is not the lineage's current one is refused too.
    assert_remote_code(
        sidecar
            .client()
            .supersede_native_escrow_prepare(supersede_request(
                signer,
                "supersede-unknown-pair",
                (
                    pair.funding.transaction_id,
                    pair.initialization.transaction_id,
                ),
                NativeEscrowStep::Fund,
            ))
            .await,
        ErrorCode::InvalidTransaction,
    );

    let request = supersede_request(
        signer,
        "supersede-fund-1",
        ids(&pair.initialization, &pair.funding),
        NativeEscrowStep::Fund,
    );
    let replanned: SupersedeNativeEscrowPrepareResult = sidecar
        .client()
        .supersede_native_escrow_prepare(request.clone())
        .await
        .expect("fund re-planned");
    assert_eq!(replanned.context, request.context);
    assert_eq!(
        replanned.superseded_initialization_transaction_id,
        pair.initialization.transaction_id
    );
    assert_eq!(
        replanned.superseded_funding_transaction_id,
        pair.funding.transaction_id
    );
    assert_eq!(
        replanned.initialization, pair.initialization,
        "kept byte-identical"
    );
    assert_ne!(replanned.funding, pair.funding);
    assert_eq!(replanned.superseded_nonce, FIRST_NONCE + 1);
    assert_eq!(replanned.replanned_nonce, FIRST_NONCE + 3);
    assert_eq!(nonce_of(&replanned.funding, signer), FIRST_NONCE + 3);
    assert_eq!(replanned.replan_index, 1);
    assert_eq!(
        supersession_files(directory.path()),
        vec!["native-escrow-supersession.1.v1.json".to_owned()]
    );
    let nonce_calls = chain.nonce_calls();

    // The same request replays from the journal; a fresh request naming the
    // same superseded pair gets the same pair with its own context echoed.
    // Neither reads the nonce again or writes a second slot.
    let journal_replay = sidecar
        .client()
        .supersede_native_escrow_prepare(request.clone())
        .await
        .expect("journal replay");
    assert_eq!(journal_replay, replanned);
    let fresh_request = supersede_request(
        signer,
        "supersede-fund-1-again",
        ids(&pair.initialization, &pair.funding),
        NativeEscrowStep::Fund,
    );
    let lineage_replay = sidecar
        .client()
        .supersede_native_escrow_prepare(fresh_request.clone())
        .await
        .expect("lineage replay");
    assert_eq!(lineage_replay.context, fresh_request.context);
    assert_eq!(lineage_replay.initialization, replanned.initialization);
    assert_eq!(lineage_replay.funding, replanned.funding);
    assert_eq!(lineage_replay.replan_index, 1);
    assert_eq!(chain.nonce_calls(), nonce_calls);
    assert_eq!(
        supersession_files(directory.path()),
        vec!["native-escrow-supersession.1.v1.json".to_owned()]
    );

    // The original pair still answers nothing new: its funding stays superseded,
    // the re-planned funding is the lineage's current one and is sent once.
    let stale = sidecar
        .client()
        .submit_transaction(submit_request(signer, &pair.funding))
        .await
        .expect("stale funding answered");
    assert!(matches!(
        stale.outcome,
        SubmissionOutcome::Superseded { .. }
    ));
    let sent = sidecar
        .client()
        .submit_transaction(submit_request(signer, &replanned.funding))
        .await
        .expect("re-planned funding submitted");
    assert_eq!(sent.outcome, SubmissionOutcome::Accepted);
    assert_eq!(chain.send_count(), 2);

    // The included initialization cannot be re-planned "from the initialize step".
    chain.set_live_nonce(FIRST_NONCE + 5);
    assert_remote_code(
        sidecar
            .client()
            .supersede_native_escrow_prepare(supersede_request(
                signer,
                "supersede-included-initialization",
                ids(&replanned.initialization, &replanned.funding),
                NativeEscrowStep::Initialize,
            ))
            .await,
        ErrorCode::InvalidRequest,
    );

    // Restart: the server replays the journal through a fresh planner. The
    // prepare answers the original pair, the supersede the lineage's current
    // pair, and no nonce is spent for either.
    sidecar.server.stop().await.expect("stop");
    let restarted_chain = Chain {
        live_nonce: Arc::clone(&chain.live_nonce),
        nonce_calls: Arc::new(AtomicUsize::new(0)),
        sends: Arc::clone(&chain.sends),
        included: Arc::clone(&chain.included),
    };
    let (_, key) = account(31);
    let restarted = start_sidecar(signer, key, &endpoint, directory.path(), &restarted_chain).await;
    assert_eq!(restarted_chain.nonce_calls(), 0);
    let original_again = prepare(&restarted, signer, claimant).await;
    assert_eq!(original_again, pair);
    let current_again = restarted
        .client()
        .supersede_native_escrow_prepare(request.clone())
        .await
        .expect("supersede replay after restart");
    assert_eq!(current_again, replanned);
    let another_context = restarted
        .client()
        .supersede_native_escrow_prepare(supersede_request(
            signer,
            "supersede-fund-1-after-restart",
            ids(&pair.initialization, &pair.funding),
            NativeEscrowStep::Fund,
        ))
        .await
        .expect("lineage replay after restart");
    assert_eq!(another_context.funding, replanned.funding);
    assert_eq!(restarted_chain.nonce_calls(), 0);
    // The recovered lineage still owns the re-planned funding. Asked again
    // for the same bytes the sidecar decides on the chain again instead of
    // replaying the journaled acceptance (chain 35: a replayed "accepted"
    // never reached the node after the first copy was dropped at execution);
    // the signer's nonce has moved past this plan above, so the answer is
    // Superseded and nothing is resent.
    let decided = restarted
        .client()
        .submit_transaction(submit_request(signer, &replanned.funding))
        .await
        .expect("re-planned funding decided again");
    assert_eq!(
        decided.outcome,
        SubmissionOutcome::Superseded {
            planned_nonce: nonce_of(&replanned.funding, signer),
            live_nonce: FIRST_NONCE + 5,
        }
    );
    assert_eq!(
        chain.send_count(),
        2,
        "decided on the chain again: nothing resent"
    );

    restarted.server.stop().await.expect("stop");
    sequencer.stop().expect("sequencer stop");
    sequencer.stopped().await;
}

#[tokio::test]
async fn initialize_supersessions_chain_and_the_sixth_is_refused_by_name() {
    let (signer, key) = account(31);
    let (claimant, _) = account(32);
    let chain = Chain::new();
    let (endpoint, sequencer) = start_sequencer(chain.clone()).await;
    let directory = secure_directory();
    let sidecar = start_sidecar(signer, key, &endpoint, directory.path(), &chain).await;

    let pair = prepare(&sidecar, signer, claimant).await;
    let mut current = (pair.initialization.clone(), pair.funding.clone());
    let mut live = FIRST_NONCE;
    for index in 1..=MAX_NATIVE_ESCROW_REPLANS {
        // The kept-initialization step is refused while nothing is included.
        assert_remote_code(
            sidecar
                .client()
                .supersede_native_escrow_prepare(supersede_request(
                    signer,
                    &format!("supersede-{index}-not-included"),
                    ids(&current.0, &current.1),
                    NativeEscrowStep::Fund,
                ))
                .await,
            ErrorCode::InvalidRequest,
        );
        live += 2;
        chain.set_live_nonce(live);
        let replanned = sidecar
            .client()
            .supersede_native_escrow_prepare(supersede_request(
                signer,
                &format!("supersede-{index}"),
                ids(&current.0, &current.1),
                NativeEscrowStep::Initialize,
            ))
            .await
            .expect("initialize re-planned");
        assert_eq!(replanned.replan_index, index);
        assert_eq!(replanned.superseded_nonce, live - 2);
        assert_eq!(replanned.replanned_nonce, live);
        assert_eq!(nonce_of(&replanned.initialization, signer), live);
        assert_eq!(nonce_of(&replanned.funding, signer), live + 1);
        assert_eq!(
            replanned.superseded_initialization_transaction_id,
            current.0.transaction_id
        );
        current = (replanned.initialization, replanned.funding);
    }
    assert_eq!(
        supersession_files(directory.path()).len(),
        MAX_NATIVE_ESCROW_REPLANS as usize
    );

    chain.set_live_nonce(live + 2);
    assert_remote_code(
        sidecar
            .client()
            .supersede_native_escrow_prepare(supersede_request(
                signer,
                "supersede-sixth",
                ids(&current.0, &current.1),
                NativeEscrowStep::Initialize,
            ))
            .await,
        ErrorCode::ReplanExhausted,
    );
    assert_eq!(
        supersession_files(directory.path()).len(),
        MAX_NATIVE_ESCROW_REPLANS as usize
    );
    // A nonce that has not moved past the plan is not a supersession either.
    let (fresh_signer, fresh_key) = account(34);
    let fresh_chain = Chain::new();
    let (fresh_endpoint, fresh_sequencer) = start_sequencer(fresh_chain.clone()).await;
    let fresh_directory = secure_directory();
    let fresh = start_sidecar(
        fresh_signer,
        fresh_key,
        &fresh_endpoint,
        fresh_directory.path(),
        &fresh_chain,
    )
    .await;
    let fresh_pair = prepare(&fresh, fresh_signer, claimant).await;
    assert_remote_code(
        fresh
            .client()
            .supersede_native_escrow_prepare(supersede_request(
                fresh_signer,
                "supersede-still-live",
                ids(&fresh_pair.initialization, &fresh_pair.funding),
                NativeEscrowStep::Initialize,
            ))
            .await,
        ErrorCode::InvalidTransaction,
    );
    assert!(supersession_files(fresh_directory.path()).is_empty());

    sidecar.server.stop().await.expect("stop");
    fresh.server.stop().await.expect("stop");
    sequencer.stop().expect("sequencer stop");
    fresh_sequencer.stop().expect("sequencer stop");
    sequencer.stopped().await;
    fresh_sequencer.stopped().await;
}

/// The live failure of chain 35: the actor resubmits the same exact bytes
/// under the same durable request id whenever it observes them absent. A
/// journaled acceptance must not be replayed for that: the decision is made
/// again on the chain — the bytes go out again while nothing changed, and
/// once another transaction of the signer took the nonce the answer is
/// Superseded, so the actor can re-plan.
#[tokio::test]
async fn a_resubmission_after_a_journaled_acceptance_is_decided_on_the_chain_again() {
    let (signer, key) = account(31);
    let (claimant, _) = account(32);
    let chain = Chain::new();
    let (endpoint, sequencer) = start_sequencer(chain.clone()).await;
    let directory = secure_directory();
    let sidecar = start_sidecar(signer, key, &endpoint, directory.path(), &chain).await;

    let pair = prepare(&sidecar, signer, claimant).await;
    let request = submit_request(signer, &pair.initialization);
    let accepted = sidecar
        .client()
        .submit_transaction(request.clone())
        .await
        .expect("first submission");
    assert_eq!(accepted.outcome, SubmissionOutcome::Accepted);
    assert_eq!(chain.send_count(), 1);

    // Nothing changed on the chain: the resubmission reaches the node again
    // instead of being answered from the journal.
    let again = sidecar
        .client()
        .submit_transaction(request.clone())
        .await
        .expect("resubmission");
    assert_eq!(again.outcome, SubmissionOutcome::Accepted);
    assert_eq!(chain.send_count(), 2, "the exact bytes went out again");

    // The signer's other transactions took the nonce (the first submission
    // was dropped at execution): the same request is now Superseded.
    chain.set_live_nonce(FIRST_NONCE + 2);
    let superseded = sidecar
        .client()
        .submit_transaction(request.clone())
        .await
        .expect("resubmission after the nonce passed");
    assert_eq!(
        superseded.outcome,
        SubmissionOutcome::Superseded {
            planned_nonce: FIRST_NONCE,
            live_nonce: FIRST_NONCE + 2,
        }
    );
    assert_eq!(
        chain.send_count(),
        2,
        "superseded bytes are never broadcast"
    );

    // The journal is history, not an overwrite: the request id's entry holds
    // the first acceptance and the resend behind the Superseded decision.
    let journal: serde_json::Value = serde_json::from_slice(
        &std::fs::read(directory.path().join("bridge-idempotency.json")).expect("journal file"),
    )
    .expect("journal json");
    let entry = journal["entries"][request.context.request_id.as_str()].clone();
    let earlier = entry["earlier_outcomes"]
        .as_array()
        .expect("earlier decisions kept");
    assert_eq!(
        earlier.len(),
        2,
        "both earlier acceptances are kept: {entry}"
    );
    assert!(
        earlier.iter().all(
            |outcome| outcome["kind"] == "success" && outcome["value"]["outcome"] == "accepted"
        ),
        "{entry}"
    );
    assert_eq!(entry["outcome"]["kind"], "success", "{entry}");
    assert!(
        entry["outcome"]["value"]["outcome"]["superseded"].is_object(),
        "{entry}"
    );

    // Landed bytes are reported known, not resent.
    chain.include_exact(&pair.initialization);
    let known = sidecar
        .client()
        .submit_transaction(request)
        .await
        .expect("resubmission of included bytes");
    assert_eq!(known.outcome, SubmissionOutcome::AlreadyKnown);
    assert_eq!(chain.send_count(), 2);

    sidecar.server.stop().await.expect("stop");
    sequencer.stop().expect("sequencer stop");
    sequencer.stopped().await;
}
