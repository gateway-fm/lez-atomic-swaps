use std::{
    net::{Ipv4Addr, SocketAddr},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use async_trait::async_trait;
use common::{
    HashType,
    block::{BedrockStatus, Block, BlockBody, BlockHeader},
};
use indexer_service_protocol::Block as IndexedBlock;
use jsonrpsee::{RpcModule, server::ServerBuilder, types::ErrorObjectOwned};
use lez_bridge_client::{BridgeClient, BridgeClientConfig, SidecarCapability};
use lez_bridge_protocol::{
    ChainClock, Hex32, MessageContext, ObserveCurrentClockRequest, Participant, RequestId, RunId,
    RuntimeCompatibility, RuntimeDescriptor,
};
use lez_v0_2_sidecar::{
    BridgeRuntime, BridgeRuntimeError, BridgeServerCapability, BridgeServerConfig,
    FinalizedIndexerApi, HistoricalAccount, NativeEscrowPlanner, NativePrepareError, NonceSource,
    OfficialNodeRpc, program_id_to_hex, start_bridge_server,
};
use nssa::{AccountId, PrivateKey, PublicKey, Signature};

const CAPABILITY: &str = "current-clock-capability-00000001";
const CURRENT_HEIGHT: u64 = 71;
const CURRENT_TIMESTAMP_MS: u64 = 1_850_000_000_071;

#[derive(Clone, Copy, Debug)]
enum NodeBehavior {
    Stable,
    HeightMovement,
    SameHeightReplacement,
    ZeroTimestamp,
    ZeroHash,
    WrongGenesis,
}

#[derive(Debug)]
struct NodeState {
    behavior: NodeBehavior,
    last_block_reads: AtomicUsize,
    current_block_reads: AtomicUsize,
    submission_calls: AtomicUsize,
}

#[derive(Debug)]
struct NeverIndexer {
    calls: AtomicUsize,
}

#[async_trait]
impl FinalizedIndexerApi for NeverIndexer {
    async fn last_finalized_block_id(&self) -> Result<Option<u64>, BridgeRuntimeError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Err(BridgeRuntimeError::InvalidObservation)
    }

    async fn block_by_id(
        &self,
        _block_id: u64,
    ) -> Result<Option<IndexedBlock>, BridgeRuntimeError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Err(BridgeRuntimeError::InvalidObservation)
    }

    async fn block_by_hash(
        &self,
        _block_hash: [u8; 32],
    ) -> Result<Option<IndexedBlock>, BridgeRuntimeError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Err(BridgeRuntimeError::InvalidObservation)
    }

    async fn account_at_block(
        &self,
        _account_id: [u8; 32],
        _block_id: u64,
    ) -> Result<HistoricalAccount, BridgeRuntimeError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Err(BridgeRuntimeError::InvalidObservation)
    }
}

#[derive(Debug)]
struct FixedNonce;

#[async_trait]
impl NonceSource for FixedNonce {
    async fn account_nonce(&self, _account_id: AccountId) -> Result<u128, NativePrepareError> {
        Ok(0)
    }
}

fn h(byte: u8) -> Hex32 {
    Hex32::from_bytes([byte; 32])
}

fn block(block_id: u64, hash: [u8; 32], timestamp: u64) -> Block {
    Block {
        header: BlockHeader {
            block_id,
            prev_block_hash: HashType([u8::try_from(block_id.saturating_sub(1)).unwrap(); 32]),
            hash: HashType(hash),
            timestamp,
            signature: Signature { value: [1; 64] },
        },
        body: BlockBody {
            transactions: Vec::new(),
        },
        bedrock_status: BedrockStatus::Pending,
    }
}

fn runtime(private_key: &PrivateKey) -> RuntimeDescriptor {
    let signer = AccountId::from(&PublicKey::new_from_private_key(private_key));
    RuntimeDescriptor::new(
        Participant::Maker,
        RuntimeCompatibility::LeeV0_2_0,
        h(1),
        h(2),
        h(3),
        program_id_to_hex([4; 8]),
        Hex32::from_bytes(signer.into_value()),
    )
}

async fn node(
    behavior: NodeBehavior,
) -> (
    Arc<NodeState>,
    OfficialNodeRpc,
    jsonrpsee::server::ServerHandle,
) {
    let state = Arc::new(NodeState {
        behavior,
        last_block_reads: AtomicUsize::new(0),
        current_block_reads: AtomicUsize::new(0),
        submission_calls: AtomicUsize::new(0),
    });
    let server = ServerBuilder::default().build("127.0.0.1:0").await.unwrap();
    let address = server.local_addr().unwrap();
    let mut module = RpcModule::new(Arc::clone(&state));
    module
        .register_method("checkHealth", |_, _, _| Ok::<_, ErrorObjectOwned>(()))
        .unwrap();
    module
        .register_method("getChannelId", |_, _, _| {
            Ok::<_, ErrorObjectOwned>(hex::encode([2_u8; 32]))
        })
        .unwrap();
    module
        .register_method("getLastBlockId", |_, state, _| {
            let read = state.last_block_reads.fetch_add(1, Ordering::SeqCst);
            let height = if matches!(state.behavior, NodeBehavior::HeightMovement) && read > 0 {
                CURRENT_HEIGHT + 1
            } else {
                CURRENT_HEIGHT
            };
            Ok::<_, ErrorObjectOwned>(height)
        })
        .unwrap();
    module
        .register_method("getBlock", |params, state, _| {
            let block_id: u64 = params.one()?;
            let observed = if block_id == nssa::GENESIS_BLOCK_ID {
                let hash = if matches!(state.behavior, NodeBehavior::WrongGenesis) {
                    [9; 32]
                } else {
                    [3; 32]
                };
                block(nssa::GENESIS_BLOCK_ID, hash, 1_800_000_000_001)
            } else {
                let read = state.current_block_reads.fetch_add(1, Ordering::SeqCst);
                let hash = if matches!(state.behavior, NodeBehavior::ZeroHash) {
                    [0; 32]
                } else if matches!(state.behavior, NodeBehavior::SameHeightReplacement) && read > 0
                {
                    [72; 32]
                } else {
                    [71; 32]
                };
                let timestamp = if matches!(state.behavior, NodeBehavior::ZeroTimestamp) {
                    0
                } else {
                    CURRENT_TIMESTAMP_MS
                };
                block(block_id, hash, timestamp)
            };
            Ok::<_, ErrorObjectOwned>(Some(observed))
        })
        .unwrap();
    module
        .register_method("sendTransaction", |_, state, _| {
            state.submission_calls.fetch_add(1, Ordering::SeqCst);
            Ok::<_, ErrorObjectOwned>(HashType([0; 32]))
        })
        .unwrap();
    let handle = server.start(module);
    let node = OfficialNodeRpc::connect(&format!("http://{address}")).unwrap();
    (state, node, handle)
}

fn bridge_runtime(
    descriptor: RuntimeDescriptor,
    private_key: PrivateKey,
    node: OfficialNodeRpc,
    indexer: Arc<NeverIndexer>,
) -> BridgeRuntime {
    let planner = NativeEscrowPlanner::new(
        Participant::Maker,
        private_key,
        [4; 8],
        [5; 8],
        descriptor.clone(),
        Arc::new(FixedNonce),
    )
    .unwrap();
    BridgeRuntime::new(descriptor, Arc::new(planner), Arc::new(node), indexer)
}

fn request(run_id: &RunId, descriptor: &RuntimeDescriptor) -> ObserveCurrentClockRequest {
    ObserveCurrentClockRequest::new(
        MessageContext::new(
            run_id.clone(),
            RequestId::new("observe-current-clock-0001").unwrap(),
            Participant::Maker,
        ),
        descriptor.clone(),
    )
}

#[tokio::test]
async fn authenticated_current_clock_is_repeatable_current_and_never_submits() {
    let private_key = PrivateKey::try_new([6; 32]).unwrap();
    let descriptor = runtime(&private_key);
    let run_id = RunId::new("current-clock-run-0000000000000000000000000001").unwrap();
    let (node_state, node, node_handle) = node(NodeBehavior::Stable).await;
    let indexer = Arc::new(NeverIndexer {
        calls: AtomicUsize::new(0),
    });
    let runtime = Arc::new(bridge_runtime(
        descriptor.clone(),
        private_key,
        node,
        Arc::clone(&indexer),
    ));
    let state_directory = tempfile::tempdir().unwrap();
    let bridge = start_bridge_server(
        BridgeServerConfig::new(
            run_id.clone(),
            BridgeServerCapability::new(CAPABILITY).unwrap(),
            state_directory.path().join("idempotency.json"),
            SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        ),
        runtime,
    )
    .await
    .unwrap();

    let observe = || async {
        BridgeClient::connect(BridgeClientConfig::new(
            bridge.endpoint(),
            SidecarCapability::new(CAPABILITY).unwrap(),
            run_id.clone(),
            descriptor.clone(),
            Duration::from_secs(2),
        ))
        .unwrap()
        .observe_current_clock(request(&run_id, &descriptor))
        .await
    };
    let first = observe().await.unwrap();
    let second = observe().await.unwrap();

    assert_eq!(first, second);
    assert_eq!(
        first.clock,
        ChainClock::new(h(71), CURRENT_HEIGHT, CURRENT_TIMESTAMP_MS)
    );
    assert_eq!(first.runtime, descriptor);
    assert_eq!(node_state.last_block_reads.load(Ordering::SeqCst), 6);
    assert_eq!(node_state.current_block_reads.load(Ordering::SeqCst), 4);
    assert_eq!(node_state.submission_calls.load(Ordering::SeqCst), 0);
    assert_eq!(indexer.calls.load(Ordering::SeqCst), 0);

    bridge.stop().await.unwrap();
    node_handle.stop().unwrap();
    node_handle.stopped().await;
}

#[tokio::test]
async fn current_clock_rejects_movement_replacement_zero_time_and_identity_drift() {
    for (behavior, expected) in [
        (NodeBehavior::HeightMovement, BridgeRuntimeError::MovingTip),
        (
            NodeBehavior::SameHeightReplacement,
            BridgeRuntimeError::MovingTip,
        ),
        (
            NodeBehavior::ZeroTimestamp,
            BridgeRuntimeError::InvalidObservation,
        ),
        (
            NodeBehavior::ZeroHash,
            BridgeRuntimeError::InvalidObservation,
        ),
        (
            NodeBehavior::WrongGenesis,
            BridgeRuntimeError::InvalidObservation,
        ),
    ] {
        let private_key = PrivateKey::try_new([6; 32]).unwrap();
        let descriptor = runtime(&private_key);
        let run_id = RunId::new("current-clock-run-0000000000000000000000000001").unwrap();
        let (node_state, node, node_handle) = node(behavior).await;
        let indexer = Arc::new(NeverIndexer {
            calls: AtomicUsize::new(0),
        });
        let runtime = bridge_runtime(descriptor.clone(), private_key, node, Arc::clone(&indexer));

        assert_eq!(
            runtime
                .observe_current_clock(&request(&run_id, &descriptor))
                .await
                .unwrap_err(),
            expected
        );
        assert_eq!(node_state.submission_calls.load(Ordering::SeqCst), 0);
        assert_eq!(indexer.calls.load(Ordering::SeqCst), 0);
        node_handle.stop().unwrap();
        node_handle.stopped().await;
    }
}
