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
use borsh::BorshDeserialize as _;
use common::{HashType, transaction::LeeTransaction};
use indexer_service_protocol::Block;
use jsonrpsee::{RpcModule, server::ServerBuilder, types::ErrorObjectOwned};
use lez_bridge_client::{
    BridgeClient, BridgeClientConfig, BridgeClientError, SidecarCapability, XmrReleaseClient,
};
use lez_bridge_protocol::{
    AggregateBip340Signature, ClassifyFinalizedNativeXmrEffectV3Request,
    CompleteNativeXmrClaimV3Request, CompleteNativeXmrRefundV3Request, DiscoveryWindow, ErrorCode,
    ExactMessageBytes, FinalizedNativeXmrScanOutcomeV3, FinalizedNativeXmrTransactionTargetV3,
    FinalizedNativeXmrUnavailableReasonV3, Hex32, MessageContext, Participant,
    PrepareNativeXmrClaimAuthorizationV3Request, PrepareNativeXmrClaimV3Request,
    PrepareNativeXmrEscrowV3Request, PrepareNativeXmrPunishV3Request,
    PrepareNativeXmrRefundV3Request, PreparedTransaction, PreparedWitnessedClaim, RequestId, RunId,
    RuntimeCompatibility, RuntimeDescriptor, SubmissionOutcome,
    SubmitNativeXmrClaimAuthorizationV3Request, SubmitTransactionRequest, TransactionId,
    XmrClaimPartialV3, XmrNativeEffectV3, XmrNativeEscrowTermsV3, XmrNativeEscrowTermsV3Input,
};
use lez_v0_2_sidecar::{
    BridgeRuntime, BridgeRuntimeError, BridgeServerCapability, BridgeServerConfig,
    BridgeServerError, FinalizedIndexerApi, HistoricalAccount, NativeEscrowPlanner,
    NativePrepareError, NonceSource, OfficialNodeRpc, ZecEscrowInstruction,
    decode_official_public_transaction, program_id_to_hex, start_bridge_server,
};
use nssa::{AccountId, PrivateKey, PublicKey, Signature, public_transaction::Message};
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
struct CountingNonce {
    value: u128,
    calls: AtomicUsize,
}

#[async_trait]
impl NonceSource for CountingNonce {
    async fn account_nonce(&self, _account_id: AccountId) -> Result<u128, NativePrepareError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(self.value)
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
    let metadata = lez_v0_2_sidecar::compute_metadata_pda(&ESCROW_PROGRAM, &swap_id);
    let custody = lez_v0_2_sidecar::compute_custody_pda(&ESCROW_PROGRAM, &swap_id);
    let claim_message = Message::try_new(
        ESCROW_PROGRAM,
        vec![metadata, custody, claimant, claim_authority],
        vec![41_u128.into()],
        ZecEscrowInstruction::ClaimNativeXmr { swap_id },
    )
    .expect("canonical tag-15 claim message");
    XmrNativeEscrowTermsV3::new(XmrNativeEscrowTermsV3Input {
        swap_id: Hex32::from_bytes(swap_id),
        activation_commitment: h(2),
        escrow_program_id: program_id_to_hex(ESCROW_PROGRAM),
        authenticated_transfer_program_id: program_id_to_hex(TRANSFER_PROGRAM),
        metadata_account_id: Hex32::from_bytes(metadata.into_value()),
        custody_account_id: Hex32::from_bytes(custody.into_value()),
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
        claim_message_hash: Hex32::from_bytes(claim_message.hash()),
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

async fn try_start_sidecar_at<N>(
    role: Participant,
    signer: AccountId,
    signer_key: PrivateKey,
    sequencer_endpoint: &str,
    directory: &Path,
    nonce_source: Arc<N>,
) -> Result<(BridgeClient, lez_v0_2_sidecar::BridgeServerHandle), BridgeServerError>
where
    N: NonceSource + 'static,
{
    let descriptor = runtime(role, signer);
    let planner = Arc::new(
        NativeEscrowPlanner::new_durable(
            role,
            signer_key,
            ESCROW_PROGRAM,
            TRANSFER_PROGRAM,
            descriptor.clone(),
            nonce_source,
            directory.join("planner"),
        )
        .expect("planner opens existing private directory"),
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
    .await?;
    let client = BridgeClient::connect(BridgeClientConfig::new(
        server.endpoint(),
        SidecarCapability::new(CAPABILITY).expect("client capability"),
        RunId::new(RUN_ID).expect("run id"),
        descriptor,
        Duration::from_secs(2),
    ))
    .expect("bridge client");
    Ok((client, server))
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
    reason = "one actor-realistic route test keeps authorization, tag-15 ABI, aggregate completion, and fail-closed bindings joined"
)]
async fn maker_prepares_and_completes_exact_tag_15_after_taker_authorization() {
    let (depositor, depositor_key) = account(31);
    let (claimant, claimant_key) = account(32);
    let xmr_terms = terms(depositor, claimant, 42);
    let maker_runtime = runtime(Participant::Maker, claimant);
    let taker_runtime = runtime(Participant::Taker, depositor);
    let (sequencer_endpoint, sequencer, sequencer_sends) = start_sequencer().await;
    let maker = start_sidecar(
        Participant::Maker,
        claimant,
        claimant_key.clone(),
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

    let authorization = prepare_owned_claim_authorization(
        &taker,
        taker_runtime.clone(),
        &xmr_terms,
        "tag15-authorization",
    )
    .await;
    let authorization_result = taker
        .fresh_release_client()
        .submit_native_xmr_claim_authorization_v3(SubmitNativeXmrClaimAuthorizationV3Request::new(
            context(Participant::Taker, "tag15-submit-authorization"),
            taker_runtime.clone(),
            xmr_terms,
            authorization,
        ))
        .await
        .expect("Taker publishes the exact durable tag-14 authorization first");
    assert_eq!(authorization_result.outcome, SubmissionOutcome::Accepted);
    assert_eq!(sequencer_sends.load(Ordering::SeqCst), 1);

    let prepare_request = PrepareNativeXmrClaimV3Request::new(
        context(Participant::Maker, "tag15-prepare-claim"),
        maker_runtime.clone(),
        xmr_terms,
    );
    let prepared = maker
        .fresh_client()
        .prepare_native_xmr_claim_v3(prepare_request.clone())
        .await
        .expect("Maker reserves the exact unsigned tag-15 claim message");
    assert_eq!(prepared.context, prepare_request.context);
    assert_eq!(prepared.terms, xmr_terms);
    assert_eq!(
        prepared.claim.preparation_request_id,
        prepare_request.context.request_id
    );
    let message = Message::try_from_slice(prepared.claim.exact_message_bytes.as_slice())
        .expect("canonical unsigned claim message");
    let terms_input = xmr_terms.to_input();
    let (claim_authority, claim_authority_key) = account(33);
    assert_eq!(message.hash(), *terms_input.claim_message_hash.as_bytes());
    assert_eq!(message.program_id, ESCROW_PROGRAM);
    assert_eq!(
        message.account_ids,
        vec![
            AccountId::new(*terms_input.metadata_account_id.as_bytes()),
            AccountId::new(*terms_input.custody_account_id.as_bytes()),
            claimant,
            claim_authority,
        ]
    );
    assert_eq!(message.nonces, vec![41_u128.into()]);
    let instruction =
        risc0_zkvm::serde::from_slice::<ZecEscrowInstruction, u32>(&message.instruction_data)
            .expect("generated tag-15 instruction");
    assert!(matches!(
        instruction,
        ZecEscrowInstruction::ClaimNativeXmr { swap_id } if swap_id == [1; 32]
    ));

    let aggregate_signature = Signature::new(&claim_authority_key, &message.hash());
    let aggregate_signature = AggregateBip340Signature::from_bytes(aggregate_signature.value);

    let mut wrong_reservation = prepared.claim.clone();
    wrong_reservation.preparation_request_id =
        RequestId::new("tag15-wrong-reservation").expect("request id");
    assert_remote_code(
        maker
            .fresh_client()
            .complete_native_xmr_claim_v3(
                CompleteNativeXmrClaimV3Request::new(
                    context(Participant::Maker, "tag15-complete-wrong-reservation"),
                    maker_runtime.clone(),
                    xmr_terms,
                    wrong_reservation,
                    aggregate_signature,
                )
                .expect("well-formed wrong reservation"),
            )
            .await,
        ErrorCode::InvalidTransaction,
    );
    let wrong_terms = terms(depositor, claimant, 43);
    assert_remote_code(
        maker
            .fresh_client()
            .complete_native_xmr_claim_v3(
                CompleteNativeXmrClaimV3Request::new(
                    context(Participant::Maker, "tag15-complete-wrong-terms"),
                    maker_runtime.clone(),
                    wrong_terms,
                    prepared.claim.clone(),
                    aggregate_signature,
                )
                .expect("claim hash is unchanged by amount drift"),
            )
            .await,
        ErrorCode::InvalidTransaction,
    );
    assert_remote_code(
        maker
            .fresh_client()
            .complete_native_xmr_claim_v3(
                CompleteNativeXmrClaimV3Request::new(
                    context(Participant::Maker, "tag15-complete-bad-signature"),
                    maker_runtime.clone(),
                    xmr_terms,
                    prepared.claim.clone(),
                    AggregateBip340Signature::from_bytes([41; 64]),
                )
                .expect("well-formed invalid signature"),
            )
            .await,
        ErrorCode::InvalidTransaction,
    );

    let complete_request = CompleteNativeXmrClaimV3Request::new(
        context(Participant::Maker, "tag15-complete-claim"),
        maker_runtime.clone(),
        xmr_terms,
        prepared.claim.clone(),
        aggregate_signature,
    )
    .expect("exact claim completion request");
    let completed = maker
        .fresh_client()
        .complete_native_xmr_claim_v3(complete_request.clone())
        .await
        .expect("aggregate signature completes one exact tag-15 transaction");
    assert_eq!(completed.context, complete_request.context);
    assert_eq!(completed.terms, xmr_terms);
    let transaction = decode_official_public_transaction(completed.claim.exact_bytes.as_slice())
        .expect("canonical completed claim transaction");
    assert_eq!(transaction.message(), &message);
    let [(observed_signature, observed_key)] =
        transaction.witness_set().signatures_and_public_keys()
    else {
        panic!("one aggregate claim witness")
    };
    assert_eq!(
        observed_signature.value,
        *complete_request.aggregate_signature.as_bytes()
    );
    assert_eq!(
        observed_key.value(),
        terms_input.claim_aggregate_x_only_public_key.as_bytes()
    );
    assert_eq!(
        maker
            .fresh_client()
            .prepare_native_xmr_claim_v3(prepare_request.clone())
            .await
            .expect("exact preparation replay"),
        prepared
    );
    assert_eq!(
        maker
            .fresh_client()
            .complete_native_xmr_claim_v3(complete_request.clone())
            .await
            .expect("exact completion replay"),
        completed
    );
    assert_eq!(
        sequencer_sends.load(Ordering::SeqCst),
        1,
        "tag-15 prepare and complete never submit"
    );
    let mut wrong_runtime = maker_runtime.clone();
    wrong_runtime.signer_account_id = h(99);
    assert!(matches!(
        maker
            .fresh_client()
            .prepare_native_xmr_claim_v3(PrepareNativeXmrClaimV3Request::new(
                context(Participant::Maker, "tag15-prepare-wrong-runtime"),
                wrong_runtime,
                xmr_terms,
            ))
            .await,
        Err(BridgeClientError::RequestContextMismatch { .. })
    ));
    assert!(
        taker
            .fresh_client()
            .prepare_native_xmr_claim_v3(PrepareNativeXmrClaimV3Request::new(
                context(Participant::Taker, "tag15-prepare-wrong-role"),
                taker_runtime,
                xmr_terms,
            ))
            .await
            .is_err(),
        "the Taker sidecar must fail closed before preparing the Maker's claim"
    );

    let maker_directory = maker.directory.path().to_path_buf();
    maker.server.stop().await.expect("maker stop");
    let restart_nonces = Arc::new(CountingNonce {
        value: 999,
        calls: AtomicUsize::new(0),
    });
    let (restarted_client, restarted_server) = try_start_sidecar_at(
        Participant::Maker,
        claimant,
        claimant_key.clone(),
        &sequencer_endpoint,
        &maker_directory,
        Arc::clone(&restart_nonces),
    )
    .await
    .expect("Maker server and planner restore exact tag-15 durable state");
    assert_eq!(
        restarted_client
            .prepare_native_xmr_claim_v3(prepare_request.clone())
            .await
            .expect("restarted server replays exact preparation"),
        prepared
    );
    assert_eq!(
        restarted_client
            .complete_native_xmr_claim_v3(complete_request.clone())
            .await
            .expect("restarted server replays exact completion"),
        completed
    );
    assert_eq!(
        restart_nonces.calls.load(Ordering::SeqCst),
        0,
        "startup recovery must not regenerate the aggregate-authority nonce"
    );
    let submission_request = SubmitTransactionRequest::new(
        context(Participant::Maker, "tag15-submit-claim"),
        maker_runtime,
        completed.claim.clone(),
    );
    let submitted = restarted_client
        .submit_transaction(submission_request)
        .await
        .expect("restored Maker submits the exact completed durable tag-15 claim");
    assert_eq!(submitted.transaction_id, completed.claim.transaction_id);
    assert_eq!(submitted.outcome, SubmissionOutcome::Accepted);
    assert_eq!(sequencer_sends.load(Ordering::SeqCst), 2);
    restarted_server
        .stop()
        .await
        .expect("restarted Maker stops");

    let completion_path = maker_directory
        .join("planner")
        .join("xmr-native-claim-completion.v3.json");
    let completion_bytes = fs::read(&completion_path).expect("saved completion reservation");
    fs::write(&completion_path, b"{").expect("corrupt completion reservation");
    let corrupt_nonces = Arc::new(CountingNonce {
        value: 999,
        calls: AtomicUsize::new(0),
    });
    let corrupt_restart = try_start_sidecar_at(
        Participant::Maker,
        claimant,
        claimant_key.clone(),
        &sequencer_endpoint,
        &maker_directory,
        Arc::clone(&corrupt_nonces),
    )
    .await;
    assert!(matches!(
        corrupt_restart,
        Err(BridgeServerError::InvalidDurableState)
    ));
    assert_eq!(corrupt_nonces.calls.load(Ordering::SeqCst), 0);
    assert_eq!(sequencer_sends.load(Ordering::SeqCst), 2);
    fs::write(&completion_path, completion_bytes).expect("restore completion reservation");

    fs::remove_file(
        maker_directory
            .join("planner")
            .join("xmr-native-claim-reservation.v3.json"),
    )
    .expect("remove preparation reservation");
    let missing_nonces = Arc::new(CountingNonce {
        value: 41,
        calls: AtomicUsize::new(0),
    });
    let missing_restart = try_start_sidecar_at(
        Participant::Maker,
        claimant,
        claimant_key,
        &sequencer_endpoint,
        &maker_directory,
        Arc::clone(&missing_nonces),
    )
    .await;
    assert!(matches!(
        missing_restart,
        Err(BridgeServerError::InvalidDurableState)
    ));
    assert_eq!(missing_nonces.calls.load(Ordering::SeqCst), 0);
    assert_eq!(sequencer_sends.load(Ordering::SeqCst), 2);

    taker.server.stop().await.expect("taker stop");
    sequencer.stop().expect("sequencer stop");
    sequencer.stopped().await;
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

    let classification = taker
        .client
        .classify_finalized_native_xmr_effect_v3(ClassifyFinalizedNativeXmrEffectV3Request::new(
            context(Participant::Taker, "classify-claim"),
            taker_runtime.clone(),
            xmr_terms,
            XmrNativeEffectV3::Claim,
            FinalizedNativeXmrTransactionTargetV3::DiscoverByTerms {},
            DiscoveryWindow::new(1, 1).expect("window"),
        ))
        .await
        .expect("classifier remains observation-only");
    assert_eq!(
        classification.outcome,
        FinalizedNativeXmrScanOutcomeV3::unavailable(
            FinalizedNativeXmrUnavailableReasonV3::FinalityUnavailable,
        )
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
