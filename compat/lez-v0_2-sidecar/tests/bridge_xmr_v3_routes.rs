#![cfg(target_os = "linux")]

use std::{
    fs,
    net::{Ipv4Addr, SocketAddr},
    os::unix::fs::PermissionsExt as _,
    sync::Arc,
    time::Duration,
};

use async_trait::async_trait;
use indexer_service_protocol::Block;
use jsonrpsee::{RpcModule, server::ServerBuilder, types::ErrorObjectOwned};
use lez_bridge_client::{BridgeClient, BridgeClientConfig, BridgeClientError, SidecarCapability};
use lez_bridge_protocol::{
    AggregateBip340Signature, ClassifyFinalizedNativeXmrEffectV3Request,
    CompleteNativeXmrClaimV3Request, CompleteNativeXmrRefundV3Request, DiscoveryWindow, ErrorCode,
    ExactMessageBytes, ExactTransactionBytes, FinalizedNativeXmrScanOutcomeV3,
    FinalizedNativeXmrTransactionTargetV3, FinalizedNativeXmrUnavailableReasonV3, Hex32,
    MessageContext, Participant, PrepareNativeXmrClaimAuthorizationV3Request,
    PrepareNativeXmrClaimV3Request, PrepareNativeXmrEscrowV3Request,
    PrepareNativeXmrPunishV3Request, PrepareNativeXmrRefundV3Request, PreparedTransaction,
    PreparedWitnessedClaim, RequestId, RunId, RuntimeCompatibility, RuntimeDescriptor,
    TransactionId, XmrClaimPartialV3, XmrNativeEffectV3, XmrNativeEscrowTermsV3,
    XmrNativeEscrowTermsV3Input,
};
use lez_v0_2_sidecar::{
    BridgeRuntime, BridgeRuntimeError, BridgeServerCapability, BridgeServerConfig,
    FinalizedIndexerApi, HistoricalAccount, NativeEscrowPlanner, NativePrepareError, NonceSource,
    OfficialNodeRpc, program_id_to_hex, start_bridge_server,
};
use nssa::{AccountId, PrivateKey, PublicKey};
use sha2::{Digest as _, Sha256};
use tempfile::TempDir;

const CAPABILITY: &str = "xmr-v3-sidecar-capability-00000001";
const RUN_ID: &str = "xmr-v3-sidecar-routes";
const ESCROW_PROGRAM: [u32; 8] = [0x1020_3040; 8];
const TRANSFER_PROGRAM: [u32; 8] = [0x5060_7080; 8];

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
    _directory: TempDir,
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
}

const fn h(byte: u8) -> Hex32 {
    Hex32::from_bytes([byte; 32])
}

fn account(byte: u8) -> (AccountId, PrivateKey) {
    let key = PrivateKey::try_new([byte; 32]).expect("valid private key");
    let public = PublicKey::new_from_private_key(&key);
    (AccountId::from(&public), key)
}

fn terms(depositor: AccountId, claimant: AccountId, amount: u128) -> XmrNativeEscrowTermsV3 {
    XmrNativeEscrowTermsV3::new(XmrNativeEscrowTermsV3Input {
        swap_id: h(1),
        activation_commitment: h(2),
        escrow_program_id: program_id_to_hex(ESCROW_PROGRAM),
        authenticated_transfer_program_id: program_id_to_hex(TRANSFER_PROGRAM),
        metadata_account_id: h(5),
        custody_account_id: h(6),
        depositor: Participant::Taker,
        depositor_account_id: Hex32::from_bytes(depositor.into_value()),
        claimant: Participant::Maker,
        claimant_account_id: Hex32::from_bytes(claimant.into_value()),
        claim_aggregate_x_only_public_key: h(9),
        claim_authority_account_id: h(10),
        refund_aggregate_x_only_public_key: h(11),
        refund_authority_account_id: h(12),
        maker_dleq_transcript_commitment: h(13),
        taker_dleq_transcript_commitment: h(14),
        claim_partial_context_binding: h(15),
        claim_partial_commitment: h(16),
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

async fn start_sequencer() -> (String, jsonrpsee::server::ServerHandle) {
    let server = ServerBuilder::default()
        .build("127.0.0.1:0")
        .await
        .expect("sequencer binds");
    let address = server.local_addr().expect("sequencer address");
    let mut rpc = RpcModule::new(());
    rpc.register_method("checkHealth", |_, (), _| Ok::<_, ErrorObjectOwned>(()))
        .expect("health method");
    rpc.register_method("getChannelId", |_, (), _| {
        Ok::<_, ErrorObjectOwned>(hex::encode([41_u8; 32]))
    })
    .expect("channel method");
    (format!("http://{address}"), server.start(rpc))
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
        _directory: directory,
    }
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
    reason = "one route contract covers all eight additive methods and replay semantics"
)]
async fn all_xmr_v3_routes_are_authenticated_bound_and_fail_closed_without_guest_support() {
    let (depositor, depositor_key) = account(31);
    let (claimant, claimant_key) = account(32);
    let xmr_terms = terms(depositor, claimant, 42);
    let maker_runtime = runtime(Participant::Maker, claimant);
    let taker_runtime = runtime(Participant::Taker, depositor);
    let (sequencer_endpoint, sequencer) = start_sequencer().await;
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
    assert_remote_code(
        taker
            .client
            .prepare_native_xmr_escrow_v3(PrepareNativeXmrEscrowV3Request::new(
                context(Participant::Taker, "prepare-escrow"),
                taker_runtime.clone(),
                xmr_terms,
            ))
            .await,
        ErrorCode::Unavailable,
    );
    assert_remote_code(
        taker
            .client
            .prepare_native_xmr_claim_authorization_v3(
                PrepareNativeXmrClaimAuthorizationV3Request::new(
                    context(Participant::Taker, "prepare-authorization"),
                    taker_runtime.clone(),
                    xmr_terms,
                    XmrClaimPartialV3::new([77; 32]).expect("claim partial"),
                ),
            )
            .await,
        ErrorCode::Unavailable,
    );

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
