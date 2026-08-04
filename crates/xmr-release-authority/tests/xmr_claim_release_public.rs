//! Public-boundary happy path for the M4 XMR claim-release issuer.
use std::{ffi::CString, fs::File, io::Write as _, os::unix::fs::PermissionsExt as _};

use async_trait::async_trait;
use command_fds::{CommandFdExt as _, FdMapping};
use std::str::FromStr as _;
use std::sync::{
    Arc, OnceLock,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;

use digest_auth::{AuthContext, AuthorizationHeader};
use jsonrpsee::RpcModule;
use lez_adaptor_signature::{AdaptorSessionContext, AdaptorSigner, SigningRole};
use lez_bridge_adapter::{LezBridgeAdapter, XmrLezBridgeBindingV3};
use lez_bridge_client::{
    BridgeClient, BridgeClientConfig, METHOD_PREPARE_NATIVE_XMR_CLAIM_AUTHORIZATION_V3,
    METHOD_SUBMIT_NATIVE_XMR_CLAIM_AUTHORIZATION_V3, RUN_ID_HEADER, SIDECAR_ROLE_HEADER,
    SidecarCapability, XmrReleaseClient,
};
use lez_bridge_protocol::{
    AccountIds, ChainClock, ChainPosition, ClassifyFinalizedNativeXmrEffectV3Request,
    ClassifyFinalizedNativeXmrEffectV3Result, DiscoveryWindow, ExactTransactionBytes,
    FinalizedBlockIdentity, FinalizedNativeXmrEffectFactsV3, FinalizedNativeXmrScanOutcomeV3,
    FinalizedNativeXmrTransactionTargetV3, Hex32, METHOD_CLASSIFY_FINALIZED_NATIVE_XMR_EFFECT_V3,
    NativeCustodyFacts, ObservedTransactionFacts, Participant as BridgeParticipant,
    PrepareNativeXmrClaimAuthorizationV3Request, PrepareNativeXmrClaimAuthorizationV3Result,
    PreparedTransaction, RequestId, RunId, RuntimeCompatibility, RuntimeDescriptor,
    SubmissionOutcome, SubmitNativeXmrClaimAuthorizationV3Request,
    SubmitNativeXmrClaimAuthorizationV3Result, TransactionId, XmrNativeEffectV3,
    XmrNativeEscrowMetadataFactsV3, XmrNativeEscrowStateV3, XmrNativeInstructionFactsV3,
};
use lez_swap_core::Participant;
use lez_xmr_monero_adapter::{
    ExpectedMoneroOutput, LoopbackRpcEndpoint, MoneroAddress, MoneroChainIdentity, MoneroNetwork,
    MoneroOutputVerifier, MoneroTopologyVerifier, MoneroTransactionId,
};
use lez_xmr_release_authority::{
    FinalizedLezClockError, FinalizedLezClockSource, PublicationAdmissionStatus,
    PublicationProtectionKey, ReleaseError, ReleasePublicationError, ReleasePublicationOutcome,
    ReleaseState, ReleaseStore, XmrReleaseSubmissionBindingV3,
};
use lez_xmr_swap_sdk::{
    CrossCurveDleqProofV1, CrossCurveScalar, MoneroAddressNetworkV1, MoneroPrivateViewKey,
    MoneroSharedAddressV1, XMR_ACTIVATION_SCHEMA_V1, XMR_AGREEMENT_SCHEMA_V1,
    XmrActivatedAgreementV1, XmrActivationBodyV1, XmrActivationRecordV1, XmrAgreementBodyV1,
    XmrAgreementRecordV1, XmrAgreementV1, XmrLezTermsV1, XmrMessagesV1, XmrMoneroTermsV1,
    XmrNamedProfileV1, XmrParticipantIdentityV1, XmrParticipantsV1, XmrRoleV1,
    XmrSessionTranscriptV1, XmrSwapDirectionV1, XmrWindowsV1,
};
use monero_rpc::monero::{Block, Hash as MoneroHash, consensus::encode::serialize_hex};
use rand_chacha::{ChaCha20Rng, rand_core::SeedableRng as _};
use rustix::fs::{MemfdFlags, Mode, SealFlags, fchmod, fcntl_add_seals, memfd_create};
use secp256k1::{Keypair, Message as SecpMessage, PublicKey, Secp256k1, SecretKey};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;
use tower::ServiceBuilder;
use tower_http::validate_request::ValidateRequestHeaderLayer;

const CAPABILITY: &str = "m4-release-public-boundary-capability-0001";
const RUN: &str = "m4-release-public-boundary-run";
const MAKER_AGREEMENT_SECRET: [u8; 32] = [7; 32];
const TAKER_AGREEMENT_SECRET: [u8; 32] = [8; 32];
const MAKER_CLAIM_SECRET: [u8; 32] = [9; 32];
const TAKER_CLAIM_SECRET: [u8; 32] = [10; 32];
const MAKER_REFUND_SECRET: [u8; 32] = [11; 32];
const TAKER_REFUND_SECRET: [u8; 32] = [12; 32];
const VIEW_KEY_BYTES: [u8; 32] = {
    let mut bytes = [0; 32];
    bytes[0] = 17;
    bytes
};
const SESSION_DOMAIN: &[u8] = b"logos.gateway.lez-xmr.adaptor-session.v1\0";
const FINALIZED_FUND_TIMESTAMP_MS: u64 = 12_500;
const REFUND_AT_MS: u64 = 20_000;
const MONERO_TX: [u8; 32] = [2; 32];
const MONERO_GENESIS: [u8; 32] = [31; 32];
const MONERO_AMOUNT: u64 = 1_000_000_000_000;
const XMR_RELEASE_INVOCATION_FD: i32 = 220;
const XMR_RELEASE_CAPABILITY_FD: i32 = 221;
const XMR_RELEASE_PROTECTION_KEY_FD: i32 = 222;
const XMR_RELEASE_STATE_DIRECTORY_FD: i32 = 223;

struct ProofFixture {
    maker_wire: Vec<u8>,
    taker_wire: Vec<u8>,
    view_public: [u8; 32],
    spend_public: [u8; 32],
    address: String,
    maker_transcript_commitment: [u8; 32],
    taker_transcript_commitment: [u8; 32],
    maker_secp_public: [u8; 33],
    taker_secp_public: [u8; 33],
}

fn proofs() -> &'static ProofFixture {
    static FIXTURE: OnceLock<ProofFixture> = OnceLock::new();
    FIXTURE.get_or_init(|| {
        let maker = scalar(11);
        let taker = scalar(13);
        let maker_proof =
            CrossCurveDleqProofV1::prove(&maker, &mut ChaCha20Rng::from_seed([71; 32]))
                .expect("Maker proof");
        let taker_proof =
            CrossCurveDleqProofV1::prove(&taker, &mut ChaCha20Rng::from_seed([72; 32]))
                .expect("Taker proof");
        let view = view_key();
        let address = MoneroSharedAddressV1::derive(
            MoneroAddressNetworkV1::Regtest,
            &maker_proof,
            &taker_proof,
            &view,
        )
        .expect("shared address");
        ProofFixture {
            maker_wire: maker_proof.to_wire_bytes().expect("Maker wire"),
            taker_wire: taker_proof.to_wire_bytes().expect("Taker wire"),
            view_public: address.public_view_key(),
            spend_public: address.public_spend_key(),
            address: address.address_string(),
            maker_transcript_commitment: maker_proof.transcript_commitment(),
            taker_transcript_commitment: taker_proof.transcript_commitment(),
            maker_secp_public: maker_proof.secp256k1_public_key(),
            taker_secp_public: taker_proof.secp256k1_public_key(),
        }
    })
}

struct StageBFixture {
    agreement: XmrAgreementV1,
    activation: XmrActivatedAgreementV1,
    taker_claim_partial: [u8; 32],
    binding: XmrLezBridgeBindingV3,
    runtime: RuntimeDescriptor,
}

fn stage_b() -> &'static StageBFixture {
    static FIXTURE: OnceLock<StageBFixture> = OnceLock::new();
    FIXTURE.get_or_init(build_stage_b)
}

#[allow(clippy::too_many_lines)]
fn build_stage_b() -> StageBFixture {
    let proof = proofs();
    let participants = participants();
    let claim_key = participants
        .claim_aggregate_x_only_key()
        .expect("claim aggregate");
    let refund_key = participants
        .refund_aggregate_x_only_key()
        .expect("refund aggregate");
    let profile = XmrNamedProfileV1::AcceleratedRegtest;
    let body = XmrAgreementBodyV1::new(
        XmrSwapDirectionV1::TakerSellsLez,
        profile,
        [19; 32],
        participants,
        XmrMoneroTermsV1::new(
            MoneroAddressNetworkV1::Regtest,
            MONERO_GENESIS,
            MONERO_AMOUNT,
            profile.required_monero_confirmations(),
            proof.maker_wire.clone(),
            proof.taker_wire.clone(),
            proof.view_public,
            proof.spend_public,
            proof.address.clone(),
        ),
        XmrLezTermsV1::new(
            [40; 32],
            [41; 32],
            [42; 8],
            [43; 8],
            profile.required_lez_finality_units(),
            [45; 32],
            [47; 32],
            [22; 32],
            [21; 32],
            claim_key,
            XmrLezTermsV1::authority_account_for_key(claim_key),
            refund_key,
            XmrLezTermsV1::authority_account_for_key(refund_key),
            proof.maker_transcript_commitment,
            proof.taker_transcript_commitment,
            501,
        ),
        XmrMessagesV1::new([51; 32], [52; 32], [53; 32]),
        XmrWindowsV1::new(10_000, REFUND_AT_MS, 30_000),
    );
    let agreement_commitment = body.commitment();
    let record = XmrAgreementRecordV1::from_parts(
        XMR_AGREEMENT_SCHEMA_V1,
        body,
        agreement_commitment,
        sign(MAKER_AGREEMENT_SECRET, agreement_commitment),
        sign(TAKER_AGREEMENT_SECRET, agreement_commitment),
    );
    let agreement = XmrAgreementV1::from_wire(&record.encode_wire().expect("agreement wire"))
        .expect("validated agreement");

    let claim_context = session_context(
        &agreement,
        b"claim",
        agreement.body().messages().claim(),
        proof.maker_secp_public,
        true,
    );
    let refund_context = session_context(
        &agreement,
        b"refund",
        agreement.body().messages().refund(),
        proof.taker_secp_public,
        false,
    );
    let (claim_transcript, maker_claim_partial, taker_claim_partial, _) =
        signer_round(&claim_context, MAKER_CLAIM_SECRET, TAKER_CLAIM_SECRET);
    let (refund_transcript, maker_refund_partial, taker_refund_partial, refund_presignature) =
        signer_round(&refund_context, MAKER_REFUND_SECRET, TAKER_REFUND_SECRET);
    let partial_context = agreement
        .claim_partial_context_binding(&claim_transcript, maker_claim_partial)
        .expect("claim partial context");
    let partial_commitment = agreement
        .commit_taker_claim_partial(&claim_transcript, maker_claim_partial, taker_claim_partial)
        .expect("Taker partial commitment");
    let activation_body = XmrActivationBodyV1::new(
        agreement.agreement_commitment(),
        agreement.claim_context_binding(),
        claim_transcript,
        maker_claim_partial,
        partial_context,
        partial_commitment,
        agreement.refund_context_binding(),
        refund_transcript,
        maker_refund_partial,
        taker_refund_partial,
        refund_presignature,
    );
    let activation_commitment = activation_body.commitment();
    let activation_record = XmrActivationRecordV1::from_parts(
        XMR_ACTIVATION_SCHEMA_V1,
        activation_body,
        activation_commitment,
        sign(MAKER_AGREEMENT_SECRET, activation_commitment),
        sign(TAKER_AGREEMENT_SECRET, activation_commitment),
    );
    let activation = XmrActivatedAgreementV1::validate(&agreement, activation_record, &view_key())
        .expect("validated Stage B");
    let binding = XmrLezBridgeBindingV3::new(&agreement, &activation).expect("bridge binding");
    let plan = activation
        .lez_initialize_plan(&agreement)
        .expect("initialize plan");
    let runtime = RuntimeDescriptor::new(
        BridgeParticipant::Taker,
        RuntimeCompatibility::LeeV0_2_0,
        h(39),
        Hex32::from_bytes(plan.channel_id()),
        Hex32::from_bytes(plan.genesis_hash()),
        Hex32::from_bytes(program_bytes(plan.escrow_program_id())),
        Hex32::from_bytes(plan.depositor_account()),
    );
    StageBFixture {
        agreement,
        activation,
        taker_claim_partial,
        binding,
        runtime,
    }
}

#[derive(Clone)]
struct BridgeFixture {
    calls: Arc<AtomicUsize>,
    submission_calls: Arc<AtomicUsize>,
}

struct BridgeSidecar {
    endpoint: String,
    calls: Arc<AtomicUsize>,
    submission_calls: Arc<AtomicUsize>,
    _handle: jsonrpsee::server::ServerHandle,
}

async fn spawn_bridge_sidecar() -> BridgeSidecar {
    let fixture = BridgeFixture {
        calls: Arc::new(AtomicUsize::new(0)),
        submission_calls: Arc::new(AtomicUsize::new(0)),
    };
    let middleware = ServiceBuilder::new()
        .layer(
            ValidateRequestHeaderLayer::has_header_value(
                "authorization",
                &format!("Bearer {CAPABILITY}"),
            )
            .expect("authorization header"),
        )
        .layer(
            ValidateRequestHeaderLayer::has_header_value(RUN_ID_HEADER, RUN).expect("run header"),
        )
        .layer(
            ValidateRequestHeaderLayer::has_header_value(SIDECAR_ROLE_HEADER, "taker")
                .expect("role header"),
        );
    let server = jsonrpsee::server::ServerBuilder::default()
        .set_http_middleware(middleware)
        .build("127.0.0.1:0")
        .await
        .expect("mock sidecar binds");
    let address = server.local_addr().expect("mock address");
    let mut module = RpcModule::new(fixture.clone());
    module
        .register_async_method(
            METHOD_PREPARE_NATIVE_XMR_CLAIM_AUTHORIZATION_V3,
            |params, fixture, _| async move {
                let request: PrepareNativeXmrClaimAuthorizationV3Request = params.one()?;
                fixture.calls.fetch_add(1, Ordering::SeqCst);
                Ok::<_, jsonrpsee::types::ErrorObjectOwned>(
                    PrepareNativeXmrClaimAuthorizationV3Result::new(
                        request.context,
                        request.terms,
                        authorization_transaction(),
                    ),
                )
            },
        )
        .expect("register claim authorization");
    module
        .register_async_method(
            METHOD_SUBMIT_NATIVE_XMR_CLAIM_AUTHORIZATION_V3,
            |params, fixture, _| async move {
                let request: SubmitNativeXmrClaimAuthorizationV3Request = params.one()?;
                fixture.calls.fetch_add(1, Ordering::SeqCst);
                fixture.submission_calls.fetch_add(1, Ordering::SeqCst);
                assert_eq!(request.authorization, authorization_transaction());
                Ok::<_, jsonrpsee::types::ErrorObjectOwned>(
                    SubmitNativeXmrClaimAuthorizationV3Result::new(
                        request.context,
                        request.terms,
                        request.authorization.transaction_id,
                        SubmissionOutcome::Accepted,
                    ),
                )
            },
        )
        .expect("register claim authorization submission");
    module
        .register_async_method(
            METHOD_CLASSIFY_FINALIZED_NATIVE_XMR_EFFECT_V3,
            |params, fixture, _| async move {
                let request: ClassifyFinalizedNativeXmrEffectV3Request = params.one()?;
                fixture.calls.fetch_add(1, Ordering::SeqCst);
                let facts = finalized_fund_facts(&request);
                let outcome = FinalizedNativeXmrScanOutcomeV3::found(
                    ChainClock::new(h(72), 110, FINALIZED_FUND_TIMESTAMP_MS),
                    request.window,
                    facts,
                );
                let response = ClassifyFinalizedNativeXmrEffectV3Result::new(
                    request.context,
                    request.terms,
                    request.effect,
                    request.target,
                    outcome,
                )
                .expect("protocol-valid finalized Fund");
                Ok::<_, jsonrpsee::types::ErrorObjectOwned>(response)
            },
        )
        .expect("register finalized classifier");
    let handle = server.start(module);
    BridgeSidecar {
        endpoint: format!("http://{address}"),
        calls: fixture.calls,
        submission_calls: fixture.submission_calls,
        _handle: handle,
    }
}

fn finalized_fund_facts(
    request: &ClassifyFinalizedNativeXmrEffectV3Request,
) -> FinalizedNativeXmrEffectFactsV3 {
    let FinalizedNativeXmrTransactionTargetV3::Exact { transaction } = &request.target else {
        panic!("issuer test uses exact persisted funding");
    };
    let terms = request.terms.to_input();
    FinalizedNativeXmrEffectFactsV3::new(
        ObservedTransactionFacts::new(
            transaction.transaction_id,
            transaction.exact_bytes.clone(),
            ChainPosition::new(h(71), 100, 2),
            AccountIds::new(vec![terms.depositor_account_id]).expect("signers"),
            true,
        ),
        XmrNativeInstructionFactsV3::new(
            XmrNativeEffectV3::Fund,
            terms.escrow_program_id,
            AccountIds::new(vec![
                terms.metadata_account_id,
                terms.custody_account_id,
                terms.depositor_account_id,
            ])
            .expect("fund accounts"),
            terms.swap_id,
            h(61),
            None,
        )
        .expect("fund instruction"),
        None,
        FinalizedBlockIdentity::new(100, h(71), 12_000),
        XmrNativeEscrowMetadataFactsV3::from_terms(request.terms, XmrNativeEscrowStateV3::Funded),
        NativeCustodyFacts::new(
            terms.custody_account_id,
            terms.authenticated_transfer_program_id,
            terms.amount,
        ),
    )
}

fn bridge_client(endpoint: &str, runtime: RuntimeDescriptor) -> BridgeClient {
    BridgeClient::connect(BridgeClientConfig::new(
        endpoint,
        SidecarCapability::new(CAPABILITY).expect("capability"),
        run_id(),
        runtime,
        Duration::from_secs(2),
    ))
    .expect("bridge client")
}

fn release_client(endpoint: &str, runtime: RuntimeDescriptor) -> XmrReleaseClient {
    XmrReleaseClient::connect(BridgeClientConfig::new(
        endpoint,
        SidecarCapability::new(CAPABILITY).expect("capability"),
        run_id(),
        runtime,
        Duration::from_secs(2),
    ))
    .expect("release client")
}

struct StableFinalizedClock {
    expected_genesis: Hex32,
    timestamp_ms: u64,
    calls: usize,
}

#[async_trait]
impl FinalizedLezClockSource for StableFinalizedClock {
    async fn read_genesis_bound_finalized_clock(
        &mut self,
        expected_genesis_block_hash: Hex32,
    ) -> Result<ChainClock, FinalizedLezClockError> {
        assert_eq!(expected_genesis_block_hash, self.expected_genesis);
        self.calls += 1;
        Ok(ChainClock::new(h(73), 111, self.timestamp_ms))
    }
}

fn authorization_transaction() -> PreparedTransaction {
    PreparedTransaction::new(
        TransactionId::from_bytes([70; 32]),
        ExactTransactionBytes::new(vec![70; 128]).expect("authorization bytes"),
    )
}

fn exact_funding() -> PreparedTransaction {
    PreparedTransaction::new(
        TransactionId::from_bytes([61; 32]),
        ExactTransactionBytes::new(vec![61; 128]).expect("funding bytes"),
    )
}

const CHALLENGE: &str = "Digest realm=\"monero-rpc\", qop=\"auth\", algorithm=MD5, nonce=\"fixed-test-nonce\", opaque=\"fixed-test-opaque\"";

#[derive(Clone, Copy)]
enum RpcRole {
    Daemon,
    TargetWallet,
    ForeignWallet,
}

#[derive(Clone)]
struct RpcData {
    role: RpcRole,
    block_hash: [u8; 32],
    block_blob: String,
    address: String,
}

struct TestRpcServer {
    endpoint: LoopbackRpcEndpoint,
    task: JoinHandle<()>,
}

impl Drop for TestRpcServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn spawn_rpc_server(
    role: RpcRole,
    username: &'static str,
    password: &'static str,
    address: String,
) -> TestRpcServer {
    let block = Block {
        tx_hashes: vec![MoneroHash(MONERO_TX)],
        ..Block::default()
    };
    let data = Arc::new(RpcData {
        role,
        block_hash: block.id().0,
        block_blob: serialize_hex(&block),
        address,
    });
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind isolated Monero fixture");
    let socket = listener.local_addr().expect("fixture address");
    let task = tokio::spawn(async move {
        loop {
            let Ok((stream, _peer)) = listener.accept().await else {
                break;
            };
            let data = Arc::clone(&data);
            tokio::spawn(async move {
                let _result = handle_rpc(stream, username, password, data).await;
            });
        }
    });
    let endpoint = LoopbackRpcEndpoint::new(&format!("http://{socket}"), username, password)
        .expect("literal-loopback endpoint");
    TestRpcServer { endpoint, task }
}

async fn handle_rpc(
    mut stream: TcpStream,
    username: &str,
    password: &str,
    data: Arc<RpcData>,
) -> Result<(), std::io::Error> {
    let request = read_http_request(&mut stream).await?;
    let authorized = request.authorization.as_deref().is_some_and(|header| {
        verify_digest(header, username, password, &request.path, &request.body)
    });
    if !authorized {
        let extra = if request.authorization.is_none() {
            format!("WWW-Authenticate: {CHALLENGE}\r\n")
        } else {
            String::new()
        };
        return write_http_response(&mut stream, 401, &extra, "").await;
    }

    let body = rpc_body(&request, &data);
    write_http_response(
        &mut stream,
        200,
        "Content-Type: application/json\r\n",
        &body,
    )
    .await
}

fn rpc_body(request: &HttpRequest, data: &RpcData) -> String {
    if request.path == "/get_transactions" {
        return json!({
            "credits": 0,
            "top_hash": hex32([4; 32]),
            "status": "OK",
            "missed_tx": [],
            "txs": [{
                "as_hex": "",
                "as_json": null,
                "block_height": 111,
                "block_timestamp": 1,
                "double_spend_seen": false,
                "in_pool": false,
                "output_indices": [],
                "tx_hash": hex32(MONERO_TX)
            }],
            "txs_as_hex": null,
            "txs_as_json": null,
            "untrusted": false
        })
        .to_string();
    }

    let request_json: Value = serde_json::from_str(&request.body).expect("typed JSON-RPC request");
    let id = request_json["id"].clone();
    let method = request_json["method"].as_str().expect("RPC method");
    let result = match (data.role, method) {
        (RpcRole::Daemon, "get_info") => json!({
            "status": "OK",
            "untrusted": false,
            "nettype": "fakechain",
            "mainnet": false,
            "testnet": false,
            "stagenet": false,
            "offline": true,
            "incoming_connections_count": 0,
            "outgoing_connections_count": 0,
            "version": "0.18.5.1-release"
        }),
        (RpcRole::Daemon, "get_connections") => json!({
            "connections": [],
            "status": "OK",
            "untrusted": false
        }),
        (RpcRole::Daemon, "on_get_block_hash") => json!(hex32(MONERO_GENESIS)),
        (RpcRole::Daemon, "get_last_block_header") => block_header([4; 32], 120, 0),
        (RpcRole::Daemon, "get_block_header_by_height") => block_header(data.block_hash, 111, 9),
        (RpcRole::Daemon, "get_block") => json!({ "blob": data.block_blob }),
        (RpcRole::TargetWallet | RpcRole::ForeignWallet, "get_version") => {
            json!({ "release": true, "version": 65_567 })
        }
        (RpcRole::TargetWallet, "get_transfer_by_txid") => json!({
            "transfer": {
                "address": data.address,
                "amount": MONERO_AMOUNT,
                "confirmations": 10,
                "double_spend_seen": false,
                "fee": 0,
                "height": 111,
                "note": "",
                "destinations": null,
                "payment_id": "00".repeat(8),
                "subaddr_index": { "major": 0, "minor": 0 },
                "suggested_confirmations_threshold": 10,
                "timestamp": 1,
                "txid": hex32(MONERO_TX),
                "type": "in",
                "unlock_time": 0
            }
        }),
        (RpcRole::TargetWallet, "incoming_transfers") => json!({
            "transfers": [{
                "amount": MONERO_AMOUNT,
                "global_index": 1,
                "key_image": null,
                "spent": false,
                "subaddr_index": { "major": 0, "minor": 0 },
                "tx_hash": hex32(MONERO_TX),
                "tx_size": 1,
                "block_height": 111
            }]
        }),
        _ => panic!("unexpected fixture RPC method {method}"),
    };
    json!({ "jsonrpc": "2.0", "id": id, "result": result }).to_string()
}

fn block_header(hash: [u8; 32], height: u64, depth: u64) -> Value {
    json!({
        "block_header": {
            "block_size": 1,
            "depth": depth,
            "difficulty": 1,
            "hash": hex32(hash),
            "height": height,
            "major_version": 1,
            "minor_version": 1,
            "nonce": 0,
            "num_txes": 1,
            "orphan_status": false,
            "prev_hash": hex32([9; 32]),
            "reward": 0,
            "timestamp": 1
        }
    })
}

struct HttpRequest {
    authorization: Option<String>,
    path: String,
    body: String,
}

async fn read_http_request(stream: &mut TcpStream) -> Result<HttpRequest, std::io::Error> {
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 2_048];
    let header_end = loop {
        let count = stream.read(&mut buffer).await?;
        if count == 0 {
            return Err(std::io::Error::from(std::io::ErrorKind::UnexpectedEof));
        }
        bytes.extend_from_slice(&buffer[..count]);
        if bytes.len() > 64 * 1024 {
            return Err(std::io::Error::from(std::io::ErrorKind::InvalidData));
        }
        if let Some(position) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            break position + 4;
        }
    };
    let headers = std::str::from_utf8(&bytes[..header_end])
        .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidData))?
        .to_owned();
    let content_length = headers
        .lines()
        .find_map(|line| {
            line.split_once(':').and_then(|(name, value)| {
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().ok())
                    .flatten()
            })
        })
        .unwrap_or(0);
    while bytes.len() < header_end + content_length {
        let count = stream.read(&mut buffer).await?;
        if count == 0 {
            return Err(std::io::Error::from(std::io::ErrorKind::UnexpectedEof));
        }
        bytes.extend_from_slice(&buffer[..count]);
    }
    let path = headers
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::InvalidData))?
        .to_owned();
    let authorization = headers.lines().find_map(|line| {
        line.split_once(':').and_then(|(name, value)| {
            name.eq_ignore_ascii_case("authorization")
                .then(|| value.trim().to_owned())
        })
    });
    let body = String::from_utf8(bytes[header_end..header_end + content_length].to_vec())
        .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidData))?;
    Ok(HttpRequest {
        authorization,
        path,
        body,
    })
}

fn verify_digest(header: &str, username: &str, password: &str, path: &str, body: &str) -> bool {
    let Ok(mut supplied) = AuthorizationHeader::parse(header) else {
        return false;
    };
    if supplied.username != username || supplied.uri != path {
        return false;
    }
    let claimed = supplied.response.clone();
    supplied.digest(&AuthContext::new_post(
        username,
        password,
        path,
        Some(body.as_bytes()),
    ));
    supplied.response == claimed
}

async fn write_http_response(
    stream: &mut TcpStream,
    status: u16,
    extra_headers: &str,
    body: &str,
) -> Result<(), std::io::Error> {
    let reason = if status == 200 { "OK" } else { "Unauthorized" };
    let response = format!(
        "HTTP/1.1 {status} {reason}\r\n{extra_headers}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(response.as_bytes()).await?;
    stream.shutdown().await
}

fn hex32(bytes: [u8; 32]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(64);
    for byte in bytes {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    encoded
}

fn session_context(
    agreement: &XmrAgreementV1,
    purpose: &[u8],
    message: [u8; 32],
    adaptor_point: [u8; 33],
    claim: bool,
) -> AdaptorSessionContext {
    let participants = agreement.body().participants();
    let maker = participants.for_role(XmrRoleV1::Maker);
    let taker = participants.for_role(XmrRoleV1::Taker);
    let keys = if claim {
        [
            maker.claim_session_public_key(),
            taker.claim_session_public_key(),
        ]
    } else {
        [
            maker.refund_session_public_key(),
            taker.refund_session_public_key(),
        ]
    };
    AdaptorSessionContext::untweaked(
        keys,
        message,
        adaptor_point,
        session_id(agreement.agreement_commitment(), purpose),
    )
    .expect("adaptor session")
}

fn signer_round(
    context: &AdaptorSessionContext,
    maker_secret: [u8; 32],
    taker_secret: [u8; 32],
) -> (XmrSessionTranscriptV1, [u8; 32], [u8; 32], [u8; 65]) {
    let mut maker = AdaptorSigner::new(context.clone(), SigningRole::Maker, maker_secret)
        .expect("Maker signer");
    let mut taker = AdaptorSigner::new(context.clone(), SigningRole::Taker, taker_secret)
        .expect("Taker signer");
    let maker_commitment = maker.nonce_commitment();
    let taker_commitment = taker.nonce_commitment();
    maker
        .accept_peer_commitment(taker_commitment)
        .expect("Maker commitment");
    taker
        .accept_peer_commitment(maker_commitment)
        .expect("Taker commitment");
    let maker_nonce = maker.public_nonce().expect("Maker nonce");
    let taker_nonce = taker.public_nonce().expect("Taker nonce");
    maker.accept_peer_nonce(taker_nonce).expect("Maker opening");
    taker.accept_peer_nonce(maker_nonce).expect("Taker opening");
    let maker_partial = maker.create_partial_signature().expect("Maker partial");
    let taker_partial = taker.create_partial_signature().expect("Taker partial");
    maker
        .accept_peer_partial_signature(taker_partial)
        .expect("Maker verifies partial");
    taker
        .accept_peer_partial_signature(maker_partial)
        .expect("Taker verifies partial");
    (
        XmrSessionTranscriptV1::new(maker_commitment, taker_commitment, maker_nonce, taker_nonce),
        maker_partial,
        taker_partial,
        maker.presignature().expect("presignature"),
    )
}

fn participants() -> XmrParticipantsV1 {
    XmrParticipantsV1::new(
        XmrParticipantIdentityV1::new(
            [21; 32],
            public_key(MAKER_AGREEMENT_SECRET),
            public_key(MAKER_CLAIM_SECRET),
            public_key(MAKER_REFUND_SECRET),
        ),
        XmrParticipantIdentityV1::new(
            [22; 32],
            public_key(TAKER_AGREEMENT_SECRET),
            public_key(TAKER_CLAIM_SECRET),
            public_key(TAKER_REFUND_SECRET),
        ),
    )
}

fn scalar(value: u8) -> CrossCurveScalar {
    let mut bytes = [0; 32];
    bytes[0] = value;
    CrossCurveScalar::from_monero_little_endian(bytes).expect("fixture scalar")
}

fn view_key() -> MoneroPrivateViewKey {
    MoneroPrivateViewKey::from_monero_little_endian(VIEW_KEY_BYTES).expect("private view key")
}

fn public_key(secret: [u8; 32]) -> [u8; 33] {
    let secret = SecretKey::from_slice(&secret).expect("fixture secret");
    PublicKey::from_secret_key(&Secp256k1::new(), &secret).serialize()
}

fn sign(secret: [u8; 32], commitment: [u8; 32]) -> [u8; 64] {
    let secret = SecretKey::from_slice(&secret).expect("fixture secret");
    let secp = Secp256k1::new();
    secp.sign_schnorr_no_aux_rand(
        &SecpMessage::from_digest(commitment),
        &Keypair::from_secret_key(&secp, &secret),
    )
    .serialize()
}

fn session_id(commitment: [u8; 32], purpose: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(SESSION_DOMAIN);
    hasher.update(commitment);
    hasher.update(purpose);
    hasher.finalize().into()
}

fn program_bytes(words: [u32; 8]) -> [u8; 32] {
    let mut bytes = [0; 32];
    for (index, word) in words.into_iter().enumerate() {
        bytes[index * 4..index * 4 + 4].copy_from_slice(&word.to_le_bytes());
    }
    bytes
}

fn run_id() -> RunId {
    RunId::new(RUN).expect("run ID")
}

const fn h(byte: u8) -> Hex32 {
    Hex32::from_bytes([byte; 32])
}

#[tokio::test]
#[allow(
    clippy::too_many_lines,
    reason = "one test keeps all four linear capabilities visibly joined through restart"
)]
async fn authenticated_capabilities_prepare_and_restart_exact_release() {
    let stage = stage_b();
    let bridge = spawn_bridge_sidecar().await;
    let adapter = LezBridgeAdapter::new(
        bridge_client(&bridge.endpoint, stage.runtime.clone()),
        run_id(),
        stage.runtime.clone(),
        Participant::Taker,
    )
    .expect("Taker adapter");

    let authorization = adapter
        .prepare_xmr_claim_authorization_v3(
            &stage.agreement,
            &stage.activation,
            &stage.binding,
            RequestId::new("m4-release-authorization").expect("request ID"),
            stage.taker_claim_partial,
        )
        .await
        .expect("authenticated authorization capability");
    let first_lock = adapter
        .prove_finalized_xmr_first_lock_v3(
            &stage.binding,
            RequestId::new("m4-release-finalized-fund").expect("request ID"),
            exact_funding(),
            DiscoveryWindow::new(90, 21).expect("scan window"),
        )
        .await
        .expect("authenticated finalized-Fund capability");
    assert_eq!(bridge.calls.load(Ordering::SeqCst), 2);

    let shared_address =
        MoneroAddress::from_str(stage.agreement.body().monero().address()).expect("shared address");
    let daemon = spawn_rpc_server(
        RpcRole::Daemon,
        "daemon-user",
        "daemon-secret",
        shared_address.to_string(),
    )
    .await;
    let target_wallet = spawn_rpc_server(
        RpcRole::TargetWallet,
        "wallet-user",
        "wallet-secret",
        shared_address.to_string(),
    )
    .await;
    let foreign_wallet = spawn_rpc_server(
        RpcRole::ForeignWallet,
        "foreign-user",
        "foreign-secret",
        shared_address.to_string(),
    )
    .await;
    let identity =
        MoneroChainIdentity::new(MoneroNetwork::Regtest, MONERO_GENESIS).expect("chain identity");
    let expected = ExpectedMoneroOutput::new(
        MoneroTransactionId(MONERO_TX),
        shared_address,
        MONERO_AMOUNT,
    )
    .expect("expected output");
    let observation =
        MoneroOutputVerifier::new(identity, &daemon.endpoint, &target_wallet.endpoint)
            .expect("output verifier")
            .verify(&expected)
            .await
            .expect("typed exact output observation");
    let topology = MoneroTopologyVerifier::new(
        run_id(),
        identity,
        &daemon.endpoint,
        &target_wallet.endpoint,
        &foreign_wallet.endpoint,
    )
    .expect("topology verifier")
    .verify()
    .await
    .expect("run-bound authenticated topology");

    let directory = tempfile::tempdir().expect("journal directory");
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))
        .expect("owner-private journal directory");
    let path = directory.path().join("release.sqlite3");
    let key =
        PublicationProtectionKey::new("m4-public-boundary-key", [88; 32]).expect("journal key");
    let terms = stage.binding.terms().to_input();
    let store = ReleaseStore::open(&path).expect("release journal");
    let prepared = store
        .prepare_xmr_claim_release(
            &stage.agreement,
            &stage.activation,
            first_lock,
            authorization,
            observation,
            topology,
            &key,
        )
        .expect("exact public release preparation");

    assert_eq!(prepared.state(), ReleaseState::Prepared);
    assert_eq!(prepared.publication_id(), [70; 32]);
    assert_eq!(prepared.window().start(), FINALIZED_FUND_TIMESTAMP_MS);
    assert_eq!(prepared.window().end(), REFUND_AT_MS);
    drop(store);

    let restarted = ReleaseStore::open(&path).expect("reopened release journal");
    let loaded = restarted
        .load_xmr_claim_release(*terms.swap_id.as_bytes(), &run_id(), &key)
        .expect("authenticated restart load");
    assert_eq!(loaded, prepared);

    let binding =
        XmrReleaseSubmissionBindingV3::new(run_id(), stage.runtime.clone(), stage.binding.terms())
            .expect("typed release binding");
    let mut wrong_runtime = stage.runtime.clone();
    wrong_runtime.chain_id = h(99);
    let wrong_client = release_client(&bridge.endpoint, wrong_runtime);
    let mut clock = StableFinalizedClock {
        expected_genesis: stage.runtime.genesis_block_hash,
        timestamp_ms: 13_000,
        calls: 0,
    };
    assert_eq!(
        restarted
            .publish_xmr_claim_release(loaded, &key, &binding, &wrong_client, &mut clock)
            .await
            .expect_err("misbound client fails before the send CAS"),
        ReleasePublicationError::Journal(ReleaseError::BindingMismatch)
    );
    assert_eq!(clock.calls, 0);
    assert_eq!(bridge.submission_calls.load(Ordering::SeqCst), 0);

    let loaded = restarted
        .load_xmr_claim_release(*terms.swap_id.as_bytes(), &run_id(), &key)
        .expect("prepared state survives pre-CAS mismatch");
    assert_eq!(loaded.state(), ReleaseState::Prepared);
    let client = release_client(&bridge.endpoint, stage.runtime.clone());
    assert_eq!(
        restarted
            .publish_xmr_claim_release(loaded, &key, &binding, &client, &mut clock)
            .await
            .expect("one admitted exact authorization"),
        ReleasePublicationOutcome::Admitted(PublicationAdmissionStatus::Accepted)
    );
    assert_eq!(clock.calls, 2);
    assert_eq!(bridge.submission_calls.load(Ordering::SeqCst), 1);
    assert_eq!(bridge.calls.load(Ordering::SeqCst), 3);
    drop(restarted);

    let restarted = ReleaseStore::open(&path).expect("post-admission restart");
    let admitted = restarted
        .load_xmr_claim_release(*terms.swap_id.as_bytes(), &run_id(), &key)
        .expect("admitted restart load");
    assert_eq!(admitted.state(), ReleaseState::Admitted);
    let restarted_client = release_client(&bridge.endpoint, stage.runtime.clone());
    let mut restart_clock = StableFinalizedClock {
        expected_genesis: stage.runtime.genesis_block_hash,
        timestamp_ms: 13_000,
        calls: 0,
    };
    assert_eq!(
        restarted
            .publish_xmr_claim_release(
                admitted,
                &key,
                &binding,
                &restarted_client,
                &mut restart_clock,
            )
            .await
            .expect("terminal restart observes only"),
        ReleasePublicationOutcome::ObserveOnly
    );
    assert_eq!(restart_clock.calls, 0);
    assert_eq!(bridge.submission_calls.load(Ordering::SeqCst), 1);
}
const M4_RELEASE_WORKER_BIN_ENV: &str = "M4_XMR_RELEASE_WORKER_BIN";
const M4_RELEASE_KEY_HEX: &str = "5858585858585858585858585858585858585858585858585858585858585858";

#[derive(Clone)]
struct ProcessIndexerData {
    genesis: Value,
    tip: Value,
    finalized_calls: Arc<AtomicUsize>,
    by_id_calls: Arc<AtomicUsize>,
    by_hash_calls: Arc<AtomicUsize>,
}

struct ProcessIndexer {
    endpoint: String,
    finalized_calls: Arc<AtomicUsize>,
    by_id_calls: Arc<AtomicUsize>,
    by_hash_calls: Arc<AtomicUsize>,
    _handle: jsonrpsee::server::ServerHandle,
}

async fn spawn_process_indexer(genesis_hash: [u8; 32]) -> ProcessIndexer {
    let finalized_calls = Arc::new(AtomicUsize::new(0));
    let by_id_calls = Arc::new(AtomicUsize::new(0));
    let by_hash_calls = Arc::new(AtomicUsize::new(0));
    let tip_hash = [73; 32];
    let data = ProcessIndexerData {
        genesis: process_indexer_block(1, [0; 32], genesis_hash, 1),
        tip: process_indexer_block(111, genesis_hash, tip_hash, 13_000),
        finalized_calls: Arc::clone(&finalized_calls),
        by_id_calls: Arc::clone(&by_id_calls),
        by_hash_calls: Arc::clone(&by_hash_calls),
    };
    let server = jsonrpsee::server::ServerBuilder::default()
        .build("127.0.0.1:0")
        .await
        .expect("process indexer binds");
    let address = server.local_addr().expect("process indexer address");
    let mut module = RpcModule::new(data);
    module
        .register_async_method("getLastFinalizedBlockId", |_params, data, _| async move {
            data.finalized_calls.fetch_add(1, Ordering::SeqCst);
            Ok::<_, jsonrpsee::types::ErrorObjectOwned>(Some(111_u64))
        })
        .expect("register finalized ID");
    module
        .register_async_method("getBlockById", |params, data, _| async move {
            let block_id: u64 = params.one()?;
            data.by_id_calls.fetch_add(1, Ordering::SeqCst);
            let block = match block_id {
                1 => Some(data.genesis.clone()),
                111 => Some(data.tip.clone()),
                _ => None,
            };
            Ok::<_, jsonrpsee::types::ErrorObjectOwned>(block)
        })
        .expect("register block by ID");
    module
        .register_async_method("getBlockByHash", |params, data, _| async move {
            let block_hash: String = params.one()?;
            data.by_hash_calls.fetch_add(1, Ordering::SeqCst);
            let block = if block_hash == data.genesis["header"]["hash"] {
                Some(data.genesis.clone())
            } else if block_hash == data.tip["header"]["hash"] {
                Some(data.tip.clone())
            } else {
                None
            };
            Ok::<_, jsonrpsee::types::ErrorObjectOwned>(block)
        })
        .expect("register block by hash");
    let handle = server.start(module);
    ProcessIndexer {
        endpoint: format!("http://{address}"),
        finalized_calls,
        by_id_calls,
        by_hash_calls,
        _handle: handle,
    }
}

fn process_indexer_block(
    block_id: u64,
    previous_hash: [u8; 32],
    hash: [u8; 32],
    timestamp: u64,
) -> Value {
    json!({
        "header": {
            "block_id": block_id,
            "prev_block_hash": hex::encode(previous_hash),
            "hash": hex::encode(hash),
            "timestamp": timestamp,
            "signature": "00".repeat(64)
        },
        "body": {
            "transactions": []
        },
        "bedrock_status": "Finalized"
    })
}

fn write_process_private_file(path: &std::path::Path, contents: &[u8]) {
    use std::os::unix::fs::OpenOptionsExt as _;

    let mut file = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(path)
        .expect("create owner-private process input");
    std::io::Write::write_all(&mut file, contents).expect("write owner-private process input");
}

async fn run_release_worker(
    worker: std::ffi::OsString,
    public_config: std::path::PathBuf,
    state_directory: std::path::PathBuf,
    capability_file: std::path::PathBuf,
    protection_key_file: std::path::PathBuf,
) -> std::process::Output {
    let mut command = tokio::process::Command::new(worker);
    command
        .arg("--public-config-file")
        .arg(public_config)
        .arg("--state-directory")
        .arg(state_directory)
        .arg("--sidecar-capability-file")
        .arg(capability_file)
        .arg("--protection-key-file")
        .arg(protection_key_file)
        .kill_on_drop(true);
    tokio::time::timeout(Duration::from_secs(15), command.output())
        .await
        .expect("release worker exceeded 15-second bound")
        .expect("spawn isolated release worker")
}

fn sealed_process_input(label: &str, bytes: &[u8]) -> File {
    let name = CString::new(label).expect("sealed release label");
    let descriptor = memfd_create(
        name.as_c_str(),
        MemfdFlags::CLOEXEC | MemfdFlags::ALLOW_SEALING,
    )
    .expect("create sealed release descriptor");
    let mut file = File::from(descriptor);
    fchmod(&file, Mode::RUSR | Mode::WUSR).expect("make release descriptor writable");
    file.write_all(bytes)
        .expect("write sealed release descriptor");
    fchmod(&file, Mode::RUSR).expect("make release descriptor read-only");
    fcntl_add_seals(
        &file,
        SealFlags::SEAL | SealFlags::SHRINK | SealFlags::GROW | SealFlags::WRITE,
    )
    .expect("seal release descriptor");
    file
}

fn unsealed_process_input(label: &str, bytes: &[u8]) -> File {
    let name = CString::new(label).expect("unsealed release label");
    let descriptor = memfd_create(
        name.as_c_str(),
        MemfdFlags::CLOEXEC | MemfdFlags::ALLOW_SEALING,
    )
    .expect("create unsealed release descriptor");
    let mut file = File::from(descriptor);
    fchmod(&file, Mode::RUSR | Mode::WUSR).expect("make release descriptor writable");
    file.write_all(bytes)
        .expect("write unsealed release descriptor");
    fchmod(&file, Mode::RUSR).expect("make release descriptor read-only");
    file
}

async fn run_sealed_release_worker(
    worker: std::ffi::OsString,
    public_config: Value,
    state_directory: &std::path::Path,
) -> std::process::Output {
    run_release_worker_with_sealed_descriptors(worker, public_config, state_directory, true).await
}

async fn run_release_worker_with_sealed_descriptors(
    worker: std::ffi::OsString,
    public_config: Value,
    state_directory: &std::path::Path,
    seal_protection_key: bool,
) -> std::process::Output {
    let mut invocation = serde_json::to_vec(&json!({
        "schema_version": 1,
        "public_config": public_config,
    }))
    .expect("encode sealed release invocation");
    invocation.push(b'\n');
    let protection_key = if seal_protection_key {
        sealed_process_input("xmr-release-protection-key", M4_RELEASE_KEY_HEX.as_bytes())
    } else {
        unsealed_process_input("xmr-release-protection-key", M4_RELEASE_KEY_HEX.as_bytes())
    };
    let descriptors = [
        (
            sealed_process_input("xmr-release-invocation", &invocation),
            XMR_RELEASE_INVOCATION_FD,
        ),
        (
            sealed_process_input("xmr-release-capability", CAPABILITY.as_bytes()),
            XMR_RELEASE_CAPABILITY_FD,
        ),
        (protection_key, XMR_RELEASE_PROTECTION_KEY_FD),
        (
            File::open(state_directory).expect("open release state directory"),
            XMR_RELEASE_STATE_DIRECTORY_FD,
        ),
    ];
    let mut command = std::process::Command::new(worker);
    command
        .fd_mappings(
            descriptors
                .into_iter()
                .map(|(file, child_fd)| FdMapping {
                    parent_fd: file.into(),
                    child_fd,
                })
                .collect(),
        )
        .expect("map sealed release descriptors");
    let mut command = tokio::process::Command::from(command);
    command.kill_on_drop(true);
    tokio::time::timeout(Duration::from_secs(15), command.output())
        .await
        .expect("sealed release worker exceeded 15-second bound")
        .expect("spawn sealed release worker")
}

fn assert_process_output_redacted(output: &std::process::Output, private_root: &std::path::Path) {
    let rendered = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    for forbidden in [
        CAPABILITY,
        M4_RELEASE_KEY_HEX,
        &hex::encode([70; 32]),
        &private_root.display().to_string(),
    ] {
        assert!(
            !rendered.contains(forbidden),
            "process output leaked private material"
        );
    }
}

#[tokio::test]
#[ignore = "run through scripts/test-m4-xmr-release-worker-process.sh"]
#[allow(
    clippy::too_many_lines,
    reason = "one ignored process test keeps typed issuance and two fresh processes visibly joined"
)]
async fn subprocess_worker_admits_once_and_restart_observes_only() {
    let worker = std::env::var_os(M4_RELEASE_WORKER_BIN_ENV)
        .expect("process runner supplies the exact built worker");
    let stage = stage_b();
    let bridge = spawn_bridge_sidecar().await;
    let adapter = LezBridgeAdapter::new(
        bridge_client(&bridge.endpoint, stage.runtime.clone()),
        run_id(),
        stage.runtime.clone(),
        Participant::Taker,
    )
    .expect("Taker adapter");

    let authorization = adapter
        .prepare_xmr_claim_authorization_v3(
            &stage.agreement,
            &stage.activation,
            &stage.binding,
            RequestId::new("m4-process-release-authorization").expect("request ID"),
            stage.taker_claim_partial,
        )
        .await
        .expect("authenticated authorization capability");
    let first_lock = adapter
        .prove_finalized_xmr_first_lock_v3(
            &stage.binding,
            RequestId::new("m4-process-finalized-fund").expect("request ID"),
            exact_funding(),
            DiscoveryWindow::new(90, 21).expect("scan window"),
        )
        .await
        .expect("authenticated finalized-Fund capability");
    assert_eq!(bridge.calls.load(Ordering::SeqCst), 2);

    let shared_address =
        MoneroAddress::from_str(stage.agreement.body().monero().address()).expect("shared address");
    let daemon = spawn_rpc_server(
        RpcRole::Daemon,
        "process-daemon-user",
        "process-daemon-secret",
        shared_address.to_string(),
    )
    .await;
    let target_wallet = spawn_rpc_server(
        RpcRole::TargetWallet,
        "process-wallet-user",
        "process-wallet-secret",
        shared_address.to_string(),
    )
    .await;
    let foreign_wallet = spawn_rpc_server(
        RpcRole::ForeignWallet,
        "process-foreign-user",
        "process-foreign-secret",
        shared_address.to_string(),
    )
    .await;
    let identity =
        MoneroChainIdentity::new(MoneroNetwork::Regtest, MONERO_GENESIS).expect("chain identity");
    let expected = ExpectedMoneroOutput::new(
        MoneroTransactionId(MONERO_TX),
        shared_address,
        MONERO_AMOUNT,
    )
    .expect("expected output");
    let observation =
        MoneroOutputVerifier::new(identity, &daemon.endpoint, &target_wallet.endpoint)
            .expect("output verifier")
            .verify(&expected)
            .await
            .expect("typed exact output observation");
    let topology = MoneroTopologyVerifier::new(
        run_id(),
        identity,
        &daemon.endpoint,
        &target_wallet.endpoint,
        &foreign_wallet.endpoint,
    )
    .expect("topology verifier")
    .verify()
    .await
    .expect("run-bound authenticated topology");

    let directory = tempfile::tempdir().expect("process-private directory");
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))
        .expect("process-private directory mode");
    let journal_path = directory.path().join("xmr-release.sqlite3");
    let capability_path = directory.path().join("sidecar-capability");
    let protection_key_path = directory.path().join("journal-key");
    let public_config_path = directory.path().join("release.json");
    write_process_private_file(&capability_path, CAPABILITY.as_bytes());
    write_process_private_file(&protection_key_path, M4_RELEASE_KEY_HEX.as_bytes());

    let key =
        PublicationProtectionKey::new("m4-process-release-key", [88; 32]).expect("journal key");
    let terms = stage.binding.terms();
    let store = ReleaseStore::open(&journal_path).expect("release journal");
    let prepared = store
        .prepare_xmr_claim_release(
            &stage.agreement,
            &stage.activation,
            first_lock,
            authorization,
            observation,
            topology,
            &key,
        )
        .expect("typed process release preparation");
    assert_eq!(prepared.state(), ReleaseState::Prepared);
    drop(store);

    let indexer = spawn_process_indexer(*stage.runtime.genesis_block_hash.as_bytes()).await;
    let config = json!({
        "schema_version": 1,
        "sidecar_endpoint": bridge.endpoint,
        "indexer_endpoint": indexer.endpoint,
        "node_profile": "local",
        "run_id": run_id(),
        "runtime": stage.runtime,
        "terms": terms,
        "protection_key_id": "m4-process-release-key"
    });
    std::fs::write(
        &public_config_path,
        serde_json::to_vec_pretty(&config).expect("encode process config"),
    )
    .expect("write process config");
    std::fs::set_permissions(&public_config_path, std::fs::Permissions::from_mode(0o664))
        .expect("deliberately mutable public config");

    let rejected = run_release_worker(
        worker.clone(),
        public_config_path.clone(),
        directory.path().to_path_buf(),
        capability_path.clone(),
        protection_key_path.clone(),
    )
    .await;
    assert_process_output_redacted(&rejected, directory.path());
    assert!(
        !rejected.status.success(),
        "group-writable config unexpectedly reached release authority"
    );
    assert_eq!(bridge.calls.load(Ordering::SeqCst), 2);
    assert_eq!(bridge.submission_calls.load(Ordering::SeqCst), 0);
    assert_eq!(indexer.finalized_calls.load(Ordering::SeqCst), 0);
    assert_eq!(indexer.by_id_calls.load(Ordering::SeqCst), 0);
    assert_eq!(indexer.by_hash_calls.load(Ordering::SeqCst), 0);

    std::fs::set_permissions(&public_config_path, std::fs::Permissions::from_mode(0o644))
        .expect("integrity-controlled public config");
    let unsealed = run_release_worker_with_sealed_descriptors(
        worker.clone(),
        config.clone(),
        directory.path(),
        false,
    )
    .await;
    assert_process_output_redacted(&unsealed, directory.path());
    assert!(
        !unsealed.status.success(),
        "unsealed protection key unexpectedly reached release authority"
    );
    assert_eq!(bridge.calls.load(Ordering::SeqCst), 2);
    assert_eq!(bridge.submission_calls.load(Ordering::SeqCst), 0);
    assert_eq!(indexer.finalized_calls.load(Ordering::SeqCst), 0);
    assert_eq!(indexer.by_id_calls.load(Ordering::SeqCst), 0);
    assert_eq!(indexer.by_hash_calls.load(Ordering::SeqCst), 0);

    let first = run_sealed_release_worker(worker.clone(), config.clone(), directory.path()).await;
    assert_process_output_redacted(&first, directory.path());
    assert!(
        first.status.success(),
        "first release worker failed with status {}: {}",
        first.status,
        String::from_utf8_lossy(&first.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&first.stdout).expect("first report"),
        json!({
            "schema_version": 1,
            "event": "xmr_claim_authorization_publication",
            "outcome": "admitted_accepted",
            "durable_state": "admitted",
            "node_profile": "local"
        })
    );
    assert_eq!(bridge.submission_calls.load(Ordering::SeqCst), 1);
    assert_eq!(bridge.calls.load(Ordering::SeqCst), 3);
    assert_eq!(indexer.finalized_calls.load(Ordering::SeqCst), 4);
    assert_eq!(indexer.by_id_calls.load(Ordering::SeqCst), 8);
    assert_eq!(indexer.by_hash_calls.load(Ordering::SeqCst), 8);

    let second = run_sealed_release_worker(worker, config, directory.path()).await;
    assert_process_output_redacted(&second, directory.path());
    assert!(
        second.status.success(),
        "restart release worker failed with status {}: {}",
        second.status,
        String::from_utf8_lossy(&second.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&second.stdout).expect("restart report"),
        json!({
            "schema_version": 1,
            "event": "xmr_claim_authorization_publication",
            "outcome": "observe_only",
            "durable_state": "admitted",
            "node_profile": "local"
        })
    );
    assert_eq!(bridge.submission_calls.load(Ordering::SeqCst), 1);
    assert_eq!(bridge.calls.load(Ordering::SeqCst), 3);
    assert_eq!(indexer.finalized_calls.load(Ordering::SeqCst), 4);
    assert_eq!(indexer.by_id_calls.load(Ordering::SeqCst), 8);
    assert_eq!(indexer.by_hash_calls.load(Ordering::SeqCst), 8);

    let restarted = ReleaseStore::open(&journal_path).expect("post-process restart");
    let admitted = restarted
        .load_xmr_claim_release(*terms.to_input().swap_id.as_bytes(), &run_id(), &key)
        .expect("authenticated post-process load");
    assert_eq!(admitted.state(), ReleaseState::Admitted);
}
