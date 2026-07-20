#![cfg(target_os = "linux")]

use std::{
    fs,
    net::{Ipv4Addr, SocketAddr},
    os::unix::fs::PermissionsExt as _,
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
use lez_bridge_client::{
    BridgeClient, BridgeClientConfig, BridgeClientError, SidecarCapability, XmrReleaseClient,
};
use lez_bridge_protocol::{
    AggregateBip340Signature, ClassifyFinalizedNativeXmrEffectV3Request,
    CompleteNativeXmrClaimV3Request, CompleteNativeXmrRefundV3Request, DiscoveryWindow, ErrorCode,
    ExactMessageBytes, ExactTransactionBytes, FinalizedNativeXmrScanOutcomeV3,
    FinalizedNativeXmrTransactionTargetV3, FinalizedNativeXmrUnavailableReasonV3, Hex32,
    MessageContext, Participant, PrepareNativeXmrClaimAuthorizationV3Request,
    PrepareNativeXmrClaimV3Request, PrepareNativeXmrEscrowV3Request,
    PrepareNativeXmrPunishV3Request, PrepareNativeXmrRefundV3Request, PreparedTransaction,
    PreparedWitnessedClaim, RequestId, RunId, RuntimeCompatibility, RuntimeDescriptor,
    SubmissionOutcome, SubmitNativeXmrClaimAuthorizationV3Request, SubmitTransactionRequest,
    TransactionId, XmrClaimPartialV3, XmrNativeEffectV3, XmrNativeEscrowTermsV3,
    XmrNativeEscrowTermsV3Input,
};
use lez_v0_2_sidecar::{
    BridgeRuntime, BridgeRuntimeError, BridgeServerCapability, BridgeServerConfig,
    FinalizedIndexerApi, HistoricalAccount, NativeEscrowPlanner, NativePrepareError, NonceSource,
    OfficialNodeRpc, decode_official_public_transaction, program_id_to_hex, start_bridge_server,
};
use nssa::{AccountId, PrivateKey, PublicKey};
use sha2::{Digest as _, Sha256};
use tempfile::TempDir;

const CAPABILITY: &str = "xmr-v3-sidecar-capability-00000001";
const RUN_ID: &str = "xmr-v3-sidecar-routes";
const ESCROW_PROGRAM: [u32; 8] = [0x1020_3040; 8];
const TRANSFER_PROGRAM: [u32; 8] = [0x5060_7080; 8];
const CLAIM_PARTIAL_COMMITMENT_DOMAIN: &[u8] =
    b"logos.gateway.lez-xmr.claim-partial-commitment.v1\0";

#[derive(Debug)]
struct FixedNonce;

#[async_trait]
impl NonceSource for FixedNonce {
    async fn account_nonce(&self, _account_id: AccountId) -> Result<u128, NativePrepareError> {
        Ok(41)
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

struct RunningSidecar {
    client: BridgeClient,
    server: lez_v0_2_sidecar::BridgeServerHandle,
    descriptor: RuntimeDescriptor,
    directory: TempDir,
}

impl RunningSidecar {
    fn fresh_client(&self) -> BridgeClient {
        BridgeClient::connect(BridgeClientConfig::new(
            self.server.endpoint(),
            SidecarCapability::new(CAPABILITY).expect("client capability"),
            RunId::new(RUN_ID).expect("run id"),
            self.descriptor.clone(),
            Duration::from_secs(2),
        ))
        .expect("bridge client")
    }
    fn fresh_release_client(&self) -> XmrReleaseClient {
        XmrReleaseClient::connect(BridgeClientConfig::new(
            self.server.endpoint(),
            SidecarCapability::new(CAPABILITY).expect("release capability"),
            RunId::new(RUN_ID).expect("run id"),
            self.descriptor.clone(),
            Duration::from_secs(2),
        ))
        .expect("XMR release client")
    }
}

const fn h(byte: u8) -> Hex32 {
    Hex32::from_bytes([byte; 32])
}

fn claim_partial_commitment(context_binding: Hex32, claim_partial: [u8; 32]) -> Hex32 {
    let mut hasher = Sha256::new();
    hasher.update(CLAIM_PARTIAL_COMMITMENT_DOMAIN);
    hasher.update(context_binding.as_bytes());
    hasher.update(claim_partial);
    Hex32::from_bytes(hasher.finalize().into())
}

fn account(byte: u8) -> (AccountId, PrivateKey) {
    let key = PrivateKey::try_new([byte; 32]).expect("valid private key");
    let public = PublicKey::new_from_private_key(&key);
    (AccountId::from(&public), key)
}

fn terms(depositor: AccountId, claimant: AccountId, amount: u128) -> XmrNativeEscrowTermsV3 {
    let (claim_authority, claim_key) = account(33);
    let claim_public = PublicKey::new_from_private_key(&claim_key);
    let (refund_authority, refund_key) = account(34);
    let refund_public = PublicKey::new_from_private_key(&refund_key);
    let swap_id = [1; 32];
    XmrNativeEscrowTermsV3::new(XmrNativeEscrowTermsV3Input {
        swap_id: Hex32::from_bytes(swap_id),
        activation_commitment: h(2),
        escrow_program_id: program_id_to_hex(ESCROW_PROGRAM),
        authenticated_transfer_program_id: program_id_to_hex(TRANSFER_PROGRAM),
        metadata_account_id: Hex32::from_bytes(
            lez_v0_2_sidecar::compute_metadata_pda(&ESCROW_PROGRAM, &swap_id).into_value(),
        ),
        custody_account_id: Hex32::from_bytes(
            lez_v0_2_sidecar::compute_custody_pda(&ESCROW_PROGRAM, &swap_id).into_value(),
        ),
        depositor: Participant::Taker,
        depositor_account_id: Hex32::from_bytes(depositor.into_value()),
        claimant: Participant::Maker,
        claimant_account_id: Hex32::from_bytes(claimant.into_value()),
        claim_aggregate_x_only_public_key: Hex32::from_bytes(*claim_public.value()),
        claim_authority_account_id: Hex32::from_bytes(claim_authority.into_value()),
        refund_aggregate_x_only_public_key: Hex32::from_bytes(*refund_public.value()),
        refund_authority_account_id: Hex32::from_bytes(refund_authority.into_value()),
        maker_dleq_transcript_commitment: h(13),
        taker_dleq_transcript_commitment: h(14),
        claim_partial_context_binding: h(15),
        claim_partial_commitment: claim_partial_commitment(h(15), [77; 32]),
        amount,
        refund_at_ms: 10_000,
        punish_at_ms: 20_000,
        claim_message_hash: official_message_hash(&[0xc1; 128]),
        refund_message_hash: official_message_hash(&[0xd1; 128]),
        punish_message_hash: h(19),
    })
    .expect("valid XMR v3 terms")
}

fn runtime(role: Participant, signer: AccountId) -> RuntimeDescriptor {
    RuntimeDescriptor::new(
        role,
        RuntimeCompatibility::LeeV0_2_0,
        h(40),
        h(41),
        h(42),
        program_id_to_hex(ESCROW_PROGRAM),
        Hex32::from_bytes(signer.into_value()),
    )
}

fn context(role: Participant, request_id: &str) -> MessageContext {
    MessageContext::new(
        RunId::new(RUN_ID).expect("run id"),
        RequestId::new(request_id).expect("request id"),
        role,
    )
}

fn submission_context(role: Participant, transaction_id: TransactionId) -> MessageContext {
    MessageContext::new(
        RunId::new(RUN_ID).expect("run id"),
        transaction_id.submission_request_id(),
        role,
    )
}

fn transcript(bytes: Vec<u8>, request_id: &str) -> PreparedWitnessedClaim {
    PreparedWitnessedClaim::new(
        RequestId::new(request_id).expect("reservation id"),
        official_message_hash(&bytes),
        ExactMessageBytes::new(bytes).expect("message bytes"),
    )
}

fn official_message_hash(bytes: &[u8]) -> Hex32 {
    let mut hasher = Sha256::new();
    hasher.update(b"/LEE/v0.3/Message/Public/\x00\x00\x00\x00\x00\x00\x00");
    hasher.update(bytes);
    Hex32::from_bytes(hasher.finalize().into())
}

fn transaction(byte: u8) -> PreparedTransaction {
    PreparedTransaction::new(
        TransactionId::from_bytes([byte; 32]),
        ExactTransactionBytes::new(vec![byte]).expect("transaction bytes"),
    )
}

#[derive(Clone, Copy)]
enum SequencerSubmissionReply {
    Canonical,
    WrongTransactionId,
}

#[derive(Clone)]
struct SequencerFixture {
    sends: Arc<AtomicUsize>,
    lookups: Arc<AtomicUsize>,
    included_transaction: Arc<Mutex<Option<LeeTransaction>>>,
    submission_reply: SequencerSubmissionReply,
}

impl SequencerFixture {
    fn include_exact(&self, prepared: &PreparedTransaction) {
        let transaction = decode_official_public_transaction(prepared.exact_bytes.as_slice())
            .expect("official prepared transaction");
        *self
            .included_transaction
            .lock()
            .expect("included transaction lock") = Some(LeeTransaction::Public(transaction));
    }

    fn send_count(&self) -> usize {
        self.sends.load(Ordering::SeqCst)
    }

    fn lookup_count(&self) -> usize {
        self.lookups.load(Ordering::SeqCst)
    }
}

async fn start_configurable_sequencer(
    submission_reply: SequencerSubmissionReply,
) -> (String, jsonrpsee::server::ServerHandle, SequencerFixture) {
    let fixture = SequencerFixture {
        sends: Arc::new(AtomicUsize::new(0)),
        included_transaction: Arc::new(Mutex::new(None)),
        lookups: Arc::new(AtomicUsize::new(0)),
        submission_reply,
    };
    let server = ServerBuilder::default()
        .build("127.0.0.1:0")
        .await
        .expect("sequencer binds");
    let address = server.local_addr().expect("sequencer address");
    let mut rpc = RpcModule::new(fixture.clone());
    rpc.register_method("checkHealth", |_, _, _| Ok::<_, ErrorObjectOwned>(()))
        .expect("health method");
    rpc.register_method("getChannelId", |_, _, _| {
        Ok::<_, ErrorObjectOwned>(hex::encode([41_u8; 32]))
    })
    .expect("channel method");
    rpc.register_method("getTransaction", |params, fixture, _| {
        let requested: HashType = params.one()?;
        fixture.lookups.fetch_add(1, Ordering::SeqCst);
        let observed = fixture
            .included_transaction
            .lock()
            .expect("included transaction lock")
            .as_ref()
            .filter(|transaction| transaction.hash() == requested)
            .cloned();
        Ok::<_, ErrorObjectOwned>(observed)
    })
    .expect("transaction lookup method");
    rpc.register_method("sendTransaction", |params, fixture, _| {
        let transaction: LeeTransaction = params.one()?;
        fixture.sends.fetch_add(1, Ordering::SeqCst);
        let returned_id = match fixture.submission_reply {
            SequencerSubmissionReply::Canonical => transaction.hash(),
            SequencerSubmissionReply::WrongTransactionId => HashType([99; 32]),
        };
        Ok::<_, ErrorObjectOwned>(returned_id)
    })
    .expect("send method");
    (format!("http://{address}"), server.start(rpc), fixture)
}

async fn start_sequencer() -> (String, jsonrpsee::server::ServerHandle, Arc<AtomicUsize>) {
    let (endpoint, server, fixture) =
        start_configurable_sequencer(SequencerSubmissionReply::Canonical).await;
    (endpoint, server, fixture.sends)
}

async fn start_sidecar(
    role: Participant,
    signer: AccountId,
    signer_key: PrivateKey,
    sequencer_endpoint: &str,
) -> RunningSidecar {
    let descriptor = runtime(role, signer);
    let directory = TempDir::new().expect("sidecar directory");
    fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700))
        .expect("private directory");
    let planner_directory = directory.path().join("planner");
    fs::create_dir(&planner_directory).expect("planner directory");
    fs::set_permissions(&planner_directory, fs::Permissions::from_mode(0o700))
        .expect("private planner directory");
    let planner = Arc::new(
        NativeEscrowPlanner::new_durable(
            role,
            signer_key,
            ESCROW_PROGRAM,
            TRANSFER_PROGRAM,
            descriptor.clone(),
            Arc::new(FixedNonce),
            planner_directory,
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
            directory.path().join("bridge-idempotency.json"),
            SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        ),
        runtime,
    )
    .await
    .expect("sidecar starts");
    let client = BridgeClient::connect(BridgeClientConfig::new(
        server.endpoint(),
        SidecarCapability::new(CAPABILITY).expect("client capability"),
        RunId::new(RUN_ID).expect("run id"),
        descriptor.clone(),
        Duration::from_secs(2),
    ))
    .expect("bridge client");
    RunningSidecar {
        client,
        server,
        descriptor,
        directory,
    }
}

async fn prepare_owned_claim_authorization(
    sidecar: &RunningSidecar,
    descriptor: RuntimeDescriptor,
    xmr_terms: &XmrNativeEscrowTermsV3,
    request_id: &str,
) -> PreparedTransaction {
    let escrow_request_id = format!("{request_id}-escrow");
    let _ = sidecar
        .fresh_client()
        .prepare_native_xmr_escrow_v3(PrepareNativeXmrEscrowV3Request::new(
            context(Participant::Taker, &escrow_request_id),
            descriptor.clone(),
            *xmr_terms,
        ))
        .await
        .expect("prepare durable XMR escrow prerequisite");

    sidecar
        .fresh_client()
        .prepare_native_xmr_claim_authorization_v3(
            PrepareNativeXmrClaimAuthorizationV3Request::new(
                context(Participant::Taker, request_id),
                descriptor,
                *xmr_terms,
                XmrClaimPartialV3::new([77; 32]).expect("claim partial"),
            ),
        )
        .await
        .expect("prepare exact durable claim authorization")
        .authorization
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
#[allow(
    clippy::too_many_lines,
    reason = "one route contract covers all nine additive methods and replay semantics"
)]
async fn xmr_v3_routes_are_authenticated_bound_with_two_official_builders_enabled() {
    let (depositor, depositor_key) = account(31);
    let (claimant, claimant_key) = account(32);
    let xmr_terms = terms(depositor, claimant, 42);
    let maker_runtime = runtime(Participant::Maker, claimant);
    let taker_runtime = runtime(Participant::Taker, depositor);
    let (sequencer_endpoint, sequencer, sequencer_sends) = start_sequencer().await;
    let maker = start_sidecar(
        Participant::Maker,
        claimant,
        claimant_key,
        &sequencer_endpoint,
    )
    .await;
    let taker = start_sidecar(
        Participant::Taker,
        depositor,
        depositor_key,
        &sequencer_endpoint,
    )
    .await;

    assert_remote_code(
        maker
            .client
            .prepare_native_xmr_claim_v3(PrepareNativeXmrClaimV3Request::new(
                context(Participant::Maker, "prepare-claim"),
                maker_runtime.clone(),
                xmr_terms,
            ))
            .await,
        ErrorCode::Unavailable,
    );
    assert_remote_code(
        maker
            .client
            .complete_native_xmr_claim_v3(
                CompleteNativeXmrClaimV3Request::new(
                    context(Participant::Maker, "complete-claim"),
                    maker_runtime.clone(),
                    xmr_terms,
                    transcript(vec![0xc1; 128], "claim-transcript"),
                    AggregateBip340Signature::from_bytes([41; 64]),
                )
                .expect("claim completion request"),
            )
            .await,
        ErrorCode::Unavailable,
    );
    assert_remote_code(
        taker
            .client
            .prepare_native_xmr_refund_v3(PrepareNativeXmrRefundV3Request::new(
                context(Participant::Taker, "prepare-refund"),
                taker_runtime.clone(),
                xmr_terms,
            ))
            .await,
        ErrorCode::Unavailable,
    );
    assert_remote_code(
        taker
            .client
            .complete_native_xmr_refund_v3(
                CompleteNativeXmrRefundV3Request::new(
                    context(Participant::Taker, "complete-refund"),
                    taker_runtime.clone(),
                    xmr_terms,
                    transcript(vec![0xd1; 128], "refund-transcript"),
                    AggregateBip340Signature::from_bytes([42; 64]),
                )
                .expect("refund completion request"),
            )
            .await,
        ErrorCode::Unavailable,
    );
    assert_remote_code(
        maker
            .client
            .prepare_native_xmr_punish_v3(PrepareNativeXmrPunishV3Request::new(
                context(Participant::Maker, "prepare-punish"),
                maker_runtime.clone(),
                xmr_terms,
            ))
            .await,
        ErrorCode::Unavailable,
    );
    let prepared_escrow = taker
        .client
        .prepare_native_xmr_escrow_v3(PrepareNativeXmrEscrowV3Request::new(
            context(Participant::Taker, "prepare-escrow"),
            taker_runtime.clone(),
            xmr_terms,
        ))
        .await
        .expect("XMR escrow route uses the exact durable planner");
    assert_eq!(
        prepared_escrow.context.request_id.as_str(),
        "prepare-escrow"
    );
    assert_eq!(prepared_escrow.terms, xmr_terms);
    assert_ne!(prepared_escrow.initialization, prepared_escrow.funding);
    let authorization_request = PrepareNativeXmrClaimAuthorizationV3Request::new(
        context(Participant::Taker, "prepare-authorization"),
        taker_runtime.clone(),
        xmr_terms,
        XmrClaimPartialV3::new([77; 32]).expect("claim partial"),
    );
    let prepared_authorization = taker
        .client
        .prepare_native_xmr_claim_authorization_v3(authorization_request.clone())
        .await
        .expect("XMR claim authorization route uses the exact durable planner");
    assert_eq!(
        prepared_authorization.context.request_id.as_str(),
        "prepare-authorization"
    );
    assert_eq!(prepared_authorization.terms, xmr_terms);
    assert_remote_code(
        taker
            .client
            .submit_transaction(SubmitTransactionRequest::new(
                context(Participant::Taker, "submit-authorization"),
                taker_runtime.clone(),
                prepared_authorization.authorization.clone(),
            ))
            .await,
        ErrorCode::InvalidTransaction,
    );
    assert_eq!(sequencer_sends.load(Ordering::SeqCst), 0);
    let submission_request = SubmitNativeXmrClaimAuthorizationV3Request::new(
        context(Participant::Taker, "release-authorization"),
        taker_runtime.clone(),
        xmr_terms,
        prepared_authorization.authorization.clone(),
    );
    let submitted_authorization = taker
        .fresh_release_client()
        .submit_native_xmr_claim_authorization_v3(submission_request.clone())
        .await
        .expect("dedicated release route accepts exact durable tag 14 once");
    assert_eq!(
        submitted_authorization.authorization_transaction_id,
        prepared_authorization.authorization.transaction_id
    );
    assert_eq!(submitted_authorization.outcome, SubmissionOutcome::Accepted);
    assert_eq!(sequencer_sends.load(Ordering::SeqCst), 1);
    let replayed_submission = taker
        .fresh_release_client()
        .submit_native_xmr_claim_authorization_v3(submission_request)
        .await
        .expect("successful dedicated submission replays without node I/O");
    assert_eq!(replayed_submission, submitted_authorization);
    assert_eq!(sequencer_sends.load(Ordering::SeqCst), 1);

    let replayed_authorization = taker
        .fresh_client()
        .prepare_native_xmr_claim_authorization_v3(authorization_request.clone())
        .await
        .expect("authorization replays byte-identically through a fresh client");
    assert_eq!(replayed_authorization, prepared_authorization);
    fs::remove_file(
        taker
            .directory
            .path()
            .join("planner/xmr-native-claim-authorization-reservation.v3.json"),
    )
    .expect("delete planner authorization reservation");
    assert_remote_code(
        taker
            .fresh_release_client()
            .submit_native_xmr_claim_authorization_v3(
                SubmitNativeXmrClaimAuthorizationV3Request::new(
                    context(Participant::Taker, "release-missing-durable"),
                    taker_runtime.clone(),
                    xmr_terms,
                    prepared_authorization.authorization.clone(),
                ),
            )
            .await,
        ErrorCode::InvalidTransaction,
    );
    assert_eq!(sequencer_sends.load(Ordering::SeqCst), 1);
    assert_remote_code(
        taker
            .fresh_client()
            .prepare_native_xmr_claim_authorization_v3(authorization_request)
            .await,
        ErrorCode::InvalidTransaction,
    );
    assert_eq!(sequencer_sends.load(Ordering::SeqCst), 1);

    let classification = maker
        .client
        .classify_finalized_native_xmr_effect_v3(ClassifyFinalizedNativeXmrEffectV3Request::new(
            context(Participant::Maker, "classify-claim"),
            maker_runtime.clone(),
            xmr_terms,
            XmrNativeEffectV3::Claim,
            FinalizedNativeXmrTransactionTargetV3::exact(transaction(80)),
            DiscoveryWindow::new(1, 1).expect("window"),
        ))
        .await
        .expect("classifier remains observation-only");
    assert_eq!(
        classification.outcome,
        FinalizedNativeXmrScanOutcomeV3::unavailable(
            FinalizedNativeXmrUnavailableReasonV3::HistoryUnavailable,
        )
    );

    let replay = PrepareNativeXmrClaimV3Request::new(
        context(Participant::Maker, "replay-claim"),
        maker_runtime.clone(),
        xmr_terms,
    );
    assert_remote_code(
        maker
            .fresh_client()
            .prepare_native_xmr_claim_v3(replay.clone())
            .await,
        ErrorCode::Unavailable,
    );
    assert_remote_code(
        maker
            .fresh_client()
            .prepare_native_xmr_claim_v3(replay)
            .await,
        ErrorCode::Unavailable,
    );
    let changed = PrepareNativeXmrClaimV3Request::new(
        context(Participant::Maker, "replay-claim"),
        maker_runtime,
        terms(depositor, claimant, 43),
    );
    assert_remote_code(
        maker
            .fresh_client()
            .prepare_native_xmr_claim_v3(changed)
            .await,
        ErrorCode::InvalidRequest,
    );

    maker.server.stop().await.expect("maker stop");
    taker.server.stop().await.expect("taker stop");
    sequencer.stop().expect("sequencer stop");
    sequencer.stopped().await;
}

#[tokio::test]
async fn durable_xmr_escrow_pair_uses_actor_ordered_one_attempt_route() {
    let (depositor, depositor_key) = account(31);
    let (claimant, _) = account(32);
    let xmr_terms = terms(depositor, claimant, 42);
    let taker_runtime = runtime(Participant::Taker, depositor);
    let (sequencer_endpoint, sequencer, fixture) =
        start_configurable_sequencer(SequencerSubmissionReply::Canonical).await;
    let taker = start_sidecar(
        Participant::Taker,
        depositor,
        depositor_key,
        &sequencer_endpoint,
    )
    .await;
    let prepared = taker
        .fresh_client()
        .prepare_native_xmr_escrow_v3(PrepareNativeXmrEscrowV3Request::new(
            context(Participant::Taker, "prepare-actor-ordered-escrow"),
            taker_runtime.clone(),
            xmr_terms,
        ))
        .await
        .expect("prepare exact durable XMR escrow pair");

    let initialization_request = SubmitTransactionRequest::new(
        submission_context(Participant::Taker, prepared.initialization.transaction_id),
        taker_runtime.clone(),
        prepared.initialization.clone(),
    );
    let initialization = taker
        .fresh_client()
        .submit_transaction(initialization_request.clone())
        .await
        .expect("submit durable XMR initialization");
    assert_eq!(
        initialization.transaction_id,
        prepared.initialization.transaction_id
    );
    assert_eq!(initialization.outcome, SubmissionOutcome::Accepted);
    assert_eq!(fixture.lookup_count(), 1);
    assert_eq!(fixture.send_count(), 1);
    assert_eq!(
        taker
            .fresh_client()
            .submit_transaction(initialization_request)
            .await
            .expect("replay XMR initialization"),
        initialization
    );
    assert_eq!(fixture.lookup_count(), 1);
    assert_eq!(fixture.send_count(), 1);

    assert_remote_code(
        taker
            .fresh_client()
            .submit_transaction(SubmitTransactionRequest::new(
                context(Participant::Taker, "fresh-id-cannot-rearm-xmr-init"),
                taker_runtime.clone(),
                prepared.initialization.clone(),
            ))
            .await,
        ErrorCode::InvalidTransaction,
    );
    assert_eq!(fixture.lookup_count(), 1);
    assert_eq!(fixture.send_count(), 1);

    fixture.include_exact(&prepared.initialization);

    let funding_request = SubmitTransactionRequest::new(
        submission_context(Participant::Taker, prepared.funding.transaction_id),
        taker_runtime.clone(),
        prepared.funding.clone(),
    );
    let funding = taker
        .fresh_client()
        .submit_transaction(funding_request.clone())
        .await
        .expect("submit durable XMR funding");
    assert_eq!(funding.transaction_id, prepared.funding.transaction_id);
    assert_eq!(funding.outcome, SubmissionOutcome::Accepted);
    assert_eq!(fixture.lookup_count(), 3);
    assert_eq!(fixture.send_count(), 2);
    assert_eq!(
        taker
            .fresh_client()
            .submit_transaction(funding_request)
            .await
            .expect("replay XMR funding"),
        funding
    );
    assert_eq!(fixture.lookup_count(), 3);
    assert_eq!(fixture.send_count(), 2);

    taker.server.stop().await.expect("taker stop");
    sequencer.stop().expect("sequencer stop");
    sequencer.stopped().await;
}

#[tokio::test]
async fn xmr_escrow_submission_requires_the_owner_only_durable_pair() {
    let (depositor, depositor_key) = account(31);
    let (claimant, _) = account(32);
    let taker_runtime = runtime(Participant::Taker, depositor);
    let (sequencer_endpoint, sequencer, fixture) =
        start_configurable_sequencer(SequencerSubmissionReply::Canonical).await;
    let taker = start_sidecar(
        Participant::Taker,
        depositor,
        depositor_key,
        &sequencer_endpoint,
    )
    .await;
    let prepared = taker
        .fresh_client()
        .prepare_native_xmr_escrow_v3(PrepareNativeXmrEscrowV3Request::new(
            context(Participant::Taker, "prepare-missing-durable-pair"),
            taker_runtime.clone(),
            terms(depositor, claimant, 42),
        ))
        .await
        .expect("prepare exact durable XMR escrow pair");
    fs::remove_file(
        taker
            .directory
            .path()
            .join("planner/xmr-native-escrow-reservation.v3.json"),
    )
    .expect("delete durable XMR escrow reservation");

    assert_remote_code(
        taker
            .fresh_client()
            .submit_transaction(SubmitTransactionRequest::new(
                submission_context(Participant::Taker, prepared.initialization.transaction_id),
                taker_runtime,
                prepared.initialization,
            ))
            .await,
        ErrorCode::InvalidTransaction,
    );
    assert_eq!(fixture.lookup_count(), 0);
    assert_eq!(fixture.send_count(), 0);

    taker.server.stop().await.expect("taker stop");
    sequencer.stop().expect("sequencer stop");
    sequencer.stopped().await;
}

#[tokio::test]
async fn xmr_funding_before_initialization_is_terminal_and_zero_send() {
    let (depositor, depositor_key) = account(31);
    let (claimant, _) = account(32);
    let taker_runtime = runtime(Participant::Taker, depositor);
    let (sequencer_endpoint, sequencer, fixture) =
        start_configurable_sequencer(SequencerSubmissionReply::Canonical).await;
    let taker = start_sidecar(
        Participant::Taker,
        depositor,
        depositor_key,
        &sequencer_endpoint,
    )
    .await;
    let prepared = taker
        .fresh_client()
        .prepare_native_xmr_escrow_v3(PrepareNativeXmrEscrowV3Request::new(
            context(Participant::Taker, "prepare-fund-before-init"),
            taker_runtime.clone(),
            terms(depositor, claimant, 42),
        ))
        .await
        .expect("prepare exact durable XMR escrow pair");
    let funding_request = SubmitTransactionRequest::new(
        submission_context(Participant::Taker, prepared.funding.transaction_id),
        taker_runtime,
        prepared.funding,
    );

    assert_remote_code(
        taker
            .fresh_client()
            .submit_transaction(funding_request.clone())
            .await,
        ErrorCode::InvalidTransaction,
    );
    assert_eq!(fixture.lookup_count(), 1);
    assert_eq!(fixture.send_count(), 0);

    assert_remote_code(
        taker
            .fresh_client()
            .submit_transaction(funding_request)
            .await,
        ErrorCode::InvalidTransaction,
    );
    assert_eq!(fixture.lookup_count(), 1);
    assert_eq!(fixture.send_count(), 0);

    taker.server.stop().await.expect("taker stop");
    sequencer.stop().expect("sequencer stop");
    sequencer.stopped().await;
}

#[tokio::test]
async fn dedicated_xmr_authorization_already_known_uses_exact_lookup_without_send() {
    let (depositor, depositor_key) = account(31);
    let (claimant, _) = account(32);
    let xmr_terms = terms(depositor, claimant, 42);
    let taker_runtime = runtime(Participant::Taker, depositor);
    let (sequencer_endpoint, sequencer, fixture) =
        start_configurable_sequencer(SequencerSubmissionReply::Canonical).await;
    let taker = start_sidecar(
        Participant::Taker,
        depositor,
        depositor_key,
        &sequencer_endpoint,
    )
    .await;
    let authorization = prepare_owned_claim_authorization(
        &taker,
        taker_runtime.clone(),
        &xmr_terms,
        "prepare-already-known",
    )
    .await;
    fixture.include_exact(&authorization);
    let request = SubmitNativeXmrClaimAuthorizationV3Request::new(
        context(Participant::Taker, "submit-already-known"),
        taker_runtime,
        xmr_terms,
        authorization.clone(),
    );

    let result = taker
        .fresh_release_client()
        .submit_native_xmr_claim_authorization_v3(request.clone())
        .await
        .expect("exact canonical authorization is already known");
    assert_eq!(result.context, request.context);
    assert_eq!(result.terms, xmr_terms);
    assert_eq!(
        result.authorization_transaction_id,
        authorization.transaction_id
    );
    assert_eq!(result.outcome, SubmissionOutcome::AlreadyKnown);
    assert_eq!(fixture.lookup_count(), 1);
    assert_eq!(fixture.send_count(), 0);

    taker.server.stop().await.expect("taker stop");
    sequencer.stop().expect("sequencer stop");
    sequencer.stopped().await;
}

#[tokio::test]
async fn dedicated_xmr_authorization_wrong_returned_id_is_unknown_and_replay_does_not_resend() {
    let (depositor, depositor_key) = account(31);
    let (claimant, _) = account(32);
    let xmr_terms = terms(depositor, claimant, 42);
    let taker_runtime = runtime(Participant::Taker, depositor);
    let (sequencer_endpoint, sequencer, fixture) =
        start_configurable_sequencer(SequencerSubmissionReply::WrongTransactionId).await;
    let taker = start_sidecar(
        Participant::Taker,
        depositor,
        depositor_key,
        &sequencer_endpoint,
    )
    .await;
    let authorization = prepare_owned_claim_authorization(
        &taker,
        taker_runtime.clone(),
        &xmr_terms,
        "prepare-wrong-returned-id",
    )
    .await;
    let request = SubmitNativeXmrClaimAuthorizationV3Request::new(
        context(Participant::Taker, "submit-wrong-returned-id"),
        taker_runtime,
        xmr_terms,
        authorization,
    );

    assert_remote_code(
        taker
            .fresh_release_client()
            .submit_native_xmr_claim_authorization_v3(request.clone())
            .await,
        ErrorCode::UnknownSubmissionOutcome,
    );
    assert_eq!(fixture.lookup_count(), 1);
    assert_eq!(fixture.send_count(), 1);

    assert_remote_code(
        taker
            .fresh_release_client()
            .submit_native_xmr_claim_authorization_v3(request)
            .await,
        ErrorCode::UnknownSubmissionOutcome,
    );
    assert_eq!(fixture.lookup_count(), 1);
    assert_eq!(fixture.send_count(), 1);

    taker.server.stop().await.expect("taker stop");
    sequencer.stop().expect("sequencer stop");
    sequencer.stopped().await;
}
