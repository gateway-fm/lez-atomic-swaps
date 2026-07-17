#![cfg(target_os = "linux")]

use std::{
    fs,
    net::{Ipv4Addr, SocketAddr},
    os::unix::fs::PermissionsExt as _,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use async_trait::async_trait;
use indexer_service_protocol::Block;
use jsonrpsee::{RpcModule, server::ServerBuilder, types::ErrorObjectOwned};
use lez_bridge_client::{BridgeClient, BridgeClientConfig, SidecarCapability};
use lez_bridge_protocol::{
    Hex32, MessageContext, Participant, PrepareNativeRefundRequest, PrepareWitnessedEscrowRequest,
    RequestId, RunId, RuntimeCompatibility, RuntimeDescriptor, WitnessedNativeEscrowTerms,
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
