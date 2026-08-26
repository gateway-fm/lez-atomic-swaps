#![cfg(target_os = "linux")]

use std::{
    fs,
    net::{Ipv4Addr, SocketAddr},
    os::unix::fs::PermissionsExt as _,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use async_trait::async_trait;
use indexer_service_protocol::Block;
use jsonrpsee::{RpcModule, server::ServerBuilder, types::ErrorObjectOwned};
use lez_bridge_client::{BridgeClient, BridgeClientConfig, BridgeClientError, SidecarCapability};
use lez_bridge_protocol::{
    ErrorCode, Hex32, MessageContext, Participant, PrepareNativeXmrClaimAuthorizationV3Request,
    PrepareNativeXmrEscrowV3Request, RequestId, RunId, RuntimeCompatibility, RuntimeDescriptor,
    XmrClaimPartialV3, XmrNativeEscrowTermsV3, XmrNativeEscrowTermsV3Input,
};
use lez_v0_2_sidecar::{
    BridgeRuntime, BridgeRuntimeError, BridgeServerCapability, BridgeServerConfig,
    FinalizedIndexerApi, HistoricalAccount, NativeEscrowPlanner, NativePrepareError, NonceSource,
    OfficialNodeRpc, compute_custody_pda, compute_metadata_pda, program_id_to_hex,
    start_bridge_server,
};
use nssa::{AccountId, PrivateKey, PublicKey};
use sha2::{Digest as _, Sha256};
use tempfile::TempDir;

const CAPABILITY: &str = "xmr-escrow-route-capability-000001";
const RUN_ID: &str = "xmr-escrow-route-run";
const ESCROW_PROGRAM: [u32; 8] = [0x1020_3040; 8];
const TRANSFER_PROGRAM: [u32; 8] = [0x5060_7080; 8];
const SWAP_ID: [u8; 32] = [51; 32];
const CLAIM_PARTIAL_COMMITMENT_DOMAIN: &[u8] =
    b"logos.gateway.lez-xmr.claim-partial-commitment.v1\0";

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

fn account(byte: u8) -> (AccountId, PrivateKey, PublicKey) {
    let key = PrivateKey::try_new([byte; 32]).expect("valid private key");
    let public = PublicKey::new_from_private_key(&key);
    (AccountId::from(&public), key, public)
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

fn terms(depositor: AccountId, claimant: AccountId, amount: u128) -> XmrNativeEscrowTermsV3 {
    let (claim_authority, _, claim_key) = account(23);
    let (refund_authority, _, refund_key) = account(24);
    XmrNativeEscrowTermsV3::new(XmrNativeEscrowTermsV3Input {
        swap_id: Hex32::from_bytes(SWAP_ID),
        activation_commitment: h(2),
        escrow_program_id: program_id_to_hex(ESCROW_PROGRAM),
        authenticated_transfer_program_id: program_id_to_hex(TRANSFER_PROGRAM),
        metadata_account_id: Hex32::from_bytes(
            compute_metadata_pda(&ESCROW_PROGRAM, &SWAP_ID).into_value(),
        ),
        custody_account_id: Hex32::from_bytes(
            compute_custody_pda(&ESCROW_PROGRAM, &SWAP_ID).into_value(),
        ),
        depositor: Participant::Taker,
        depositor_account_id: Hex32::from_bytes(depositor.into_value()),
        claimant: Participant::Maker,
        claimant_account_id: Hex32::from_bytes(claimant.into_value()),
        claim_aggregate_x_only_public_key: Hex32::from_bytes(*claim_key.value()),
        claim_authority_account_id: Hex32::from_bytes(claim_authority.into_value()),
        refund_aggregate_x_only_public_key: Hex32::from_bytes(*refund_key.value()),
        refund_authority_account_id: Hex32::from_bytes(refund_authority.into_value()),
        maker_dleq_transcript_commitment: h(13),
        taker_dleq_transcript_commitment: h(14),
        claim_partial_context_binding: h(15),
        claim_partial_commitment: claim_partial_commitment(h(15), [77; 32]),
        amount,
        refund_at_ms: 10_000,
        punish_at_ms: 20_000,
        claim_message_hash: h(17),
        refund_message_hash: h(18),
        punish_message_hash: h(19),
    })
    .expect("valid XMR terms")
}

fn request(
    descriptor: RuntimeDescriptor,
    terms: &XmrNativeEscrowTermsV3,
    request_id: &str,
) -> PrepareNativeXmrEscrowV3Request {
    PrepareNativeXmrEscrowV3Request::new(
        MessageContext::new(
            RunId::new(RUN_ID).expect("run id"),
            RequestId::new(request_id).expect("request id"),
            Participant::Taker,
        ),
        descriptor,
        *terms,
    )
}

fn authorization_request(
    descriptor: RuntimeDescriptor,
    terms: &XmrNativeEscrowTermsV3,
) -> PrepareNativeXmrClaimAuthorizationV3Request {
    PrepareNativeXmrClaimAuthorizationV3Request::new(
        MessageContext::new(
            RunId::new(RUN_ID).expect("run id"),
            RequestId::new("xmr-claim-authorization").expect("request id"),
            Participant::Taker,
        ),
        descriptor,
        *terms,
        XmrClaimPartialV3::new([77; 32]).expect("claim partial"),
    )
}

async fn start_node() -> (String, jsonrpsee::server::ServerHandle, Arc<AtomicUsize>) {
    let sends = Arc::new(AtomicUsize::new(0));
    let server = ServerBuilder::default()
        .build("127.0.0.1:0")
        .await
        .expect("node binds");
    let address = server.local_addr().expect("node address");
    let mut rpc = RpcModule::new(Arc::clone(&sends));
    rpc.register_method("checkHealth", |_, _, _| Ok::<_, ErrorObjectOwned>(()))
        .expect("health method");
    rpc.register_method("getChannelId", |_, _, _| {
        Ok::<_, ErrorObjectOwned>(hex::encode([41_u8; 32]))
    })
    .expect("channel method");
    rpc.register_method("sendTransaction", |_, sends, _| {
        sends.fetch_add(1, Ordering::SeqCst);
        Ok::<_, ErrorObjectOwned>(hex::encode([42_u8; 32]))
    })
    .expect("send method");
    (format!("http://{address}"), server.start(rpc), sends)
}

async fn start_sidecar_at(
    directory: &Path,
    descriptor: RuntimeDescriptor,
    signer_key: PrivateKey,
    nonce: Arc<CountingNonce>,
    node_endpoint: &str,
) -> (BridgeClient, lez_v0_2_sidecar::BridgeServerHandle) {
    let planner = Arc::new(
        NativeEscrowPlanner::new_durable(
            Participant::Taker,
            signer_key,
            ESCROW_PROGRAM,
            TRANSFER_PROGRAM,
            descriptor.clone(),
            nonce,
            directory.join("planner"),
        )
        .expect("planner"),
    );
    let runtime = Arc::new(BridgeRuntime::new(
        descriptor.clone(),
        planner,
        Arc::new(OfficialNodeRpc::connect(node_endpoint).expect("official node")),
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
    let client = BridgeClient::connect(BridgeClientConfig::new(
        server.endpoint(),
        SidecarCapability::new(CAPABILITY).expect("client capability"),
        RunId::new(RUN_ID).expect("run id"),
        descriptor,
        Duration::from_secs(2),
    ))
    .expect("client");
    (client, server)
}

#[tokio::test]
async fn authenticated_route_recovers_after_fresh_server_and_never_submits() {
    let root = TempDir::new().expect("state root");
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).expect("private root");
    fs::create_dir(root.path().join("planner")).expect("planner directory");
    fs::set_permissions(
        root.path().join("planner"),
        fs::Permissions::from_mode(0o700),
    )
    .expect("private planner directory");
    let (depositor, key, _) = account(21);
    let (claimant, _, _) = account(22);
    let descriptor = runtime(depositor);
    let xmr_terms = terms(depositor, claimant, 75);
    let prepare = request(descriptor.clone(), &xmr_terms, "xmr-escrow-prepare");
    let authorize = authorization_request(descriptor.clone(), &xmr_terms);
    let (node_endpoint, node, sends) = start_node().await;
    let first_nonce = Arc::new(CountingNonce {
        value: 41,
        calls: AtomicUsize::new(0),
    });
    let (client, server) = start_sidecar_at(
        root.path(),
        descriptor.clone(),
        key,
        Arc::clone(&first_nonce),
        &node_endpoint,
    )
    .await;
    let first = client
        .prepare_native_xmr_escrow_v3(prepare.clone())
        .await
        .expect("first exact plan");
    let first_authorization = client
        .prepare_native_xmr_claim_authorization_v3(authorize.clone())
        .await
        .expect("first exact authorization");
    assert_eq!(first_nonce.calls.load(Ordering::SeqCst), 1);
    assert_eq!(sends.load(Ordering::SeqCst), 0);
    server.stop().await.expect("first server stops");

    let (_, restart_key, _) = account(21);
    let restart_nonce = Arc::new(CountingNonce {
        value: 999,
        calls: AtomicUsize::new(0),
    });
    let (restarted_client, restarted_server) = start_sidecar_at(
        root.path(),
        descriptor.clone(),
        restart_key,
        Arc::clone(&restart_nonce),
        &node_endpoint,
    )
    .await;
    let recovered = restarted_client
        .prepare_native_xmr_escrow_v3(prepare.clone())
        .await
        .expect("server replay");
    assert_eq!(recovered, first);
    let recovered_authorization = restarted_client
        .prepare_native_xmr_claim_authorization_v3(authorize)
        .await
        .expect("server restores escrow before exact authorization");
    assert_eq!(recovered_authorization, first_authorization);
    assert_eq!(restart_nonce.calls.load(Ordering::SeqCst), 0);
    assert_eq!(sends.load(Ordering::SeqCst), 0);

    let fresh_client = BridgeClient::connect(BridgeClientConfig::new(
        restarted_server.endpoint(),
        SidecarCapability::new(CAPABILITY).expect("fresh capability"),
        RunId::new(RUN_ID).expect("run id"),
        descriptor.clone(),
        Duration::from_secs(2),
    ))
    .expect("fresh collision client");
    let collision = request(
        descriptor.clone(),
        &terms(depositor, claimant, 76),
        "xmr-escrow-prepare",
    );
    let Err(BridgeClientError::Remote(collision_error)) =
        fresh_client.prepare_native_xmr_escrow_v3(collision).await
    else {
        panic!("request-id collision must fail closed");
    };
    assert_eq!(collision_error.code(), ErrorCode::InvalidRequest);

    let distinct = request(
        descriptor,
        &terms(depositor, claimant, 76),
        "xmr-escrow-distinct",
    );
    let Err(BridgeClientError::Remote(distinct_error)) =
        fresh_client.prepare_native_xmr_escrow_v3(distinct).await
    else {
        panic!("durable terms drift must fail closed");
    };
    assert_eq!(distinct_error.code(), ErrorCode::InvalidTransaction);
    assert_eq!(sends.load(Ordering::SeqCst), 0);

    restarted_server
        .stop()
        .await
        .expect("restarted server stops");
    node.stop().expect("node stops");
    node.stopped().await;
}
