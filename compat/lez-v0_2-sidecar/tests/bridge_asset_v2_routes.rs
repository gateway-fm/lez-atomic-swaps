#![cfg(target_os = "linux")]

use std::{
    fs,
    net::{Ipv4Addr, SocketAddr},
    os::unix::fs::PermissionsExt as _,
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
    Hex32, MessageContext, Participant, PrepareWitnessedAssetEscrowV2Request, RequestId, RunId,
    RuntimeCompatibility, RuntimeDescriptor, WitnessedLezAssetTermsV2, WitnessedTokenEscrowTermsV2,
    WitnessedTokenEscrowTermsV2Input,
};
use lez_v0_2_sidecar::{
    BridgeRuntime, BridgeRuntimeError, BridgeServerCapability, BridgeServerConfig,
    FinalizedIndexerApi, HistoricalAccount, NativeEscrowPlanner, NativePrepareError, NonceSource,
    OfficialNodeRpc, compute_metadata_pda, program_id_to_hex, start_bridge_server,
};
use nssa::{AccountId, PrivateKey, PublicKey};
use tempfile::TempDir;

const CAPABILITY: &str = "asset-v2-route-capability-000001";
const ESCROW_PROGRAM: [u32; 8] = [0x1020_3040; 8];
const LEGACY_TRANSFER_PROGRAM: [u32; 8] = [0x5060_7080; 8];

#[derive(Debug)]
struct CountingNonce {
    calls: Arc<AtomicUsize>,
}

#[async_trait]
impl NonceSource for CountingNonce {
    async fn account_nonce(&self, _account_id: AccountId) -> Result<u128, NativePrepareError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(41)
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

fn ata(owner: AccountId, definition: AccountId) -> AccountId {
    let seed = ata_core::compute_ata_seed(owner, definition);
    ata_core::get_associated_token_account_id(&programs::ata().id(), &seed)
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

fn request(
    depositor: AccountId,
    claimant: AccountId,
    authority_key: &PublicKey,
) -> PrepareWitnessedAssetEscrowV2Request {
    let definition = AccountId::new([31; 32]);
    let authority = AccountId::from(authority_key);
    let swap_id = h(35);
    let metadata = compute_metadata_pda(&ESCROW_PROGRAM, swap_id.as_bytes());
    let terms = WitnessedTokenEscrowTermsV2::new(WitnessedTokenEscrowTermsV2Input {
        swap_id,
        terms_hash: h(36),
        depositor: Participant::Maker,
        depositor_owner_account_id: Hex32::from_bytes(depositor.into_value()),
        depositor_ata_account_id: Hex32::from_bytes(ata(depositor, definition).into_value()),
        claimant: Participant::Taker,
        claimant_owner_account_id: Hex32::from_bytes(claimant.into_value()),
        claimant_ata_account_id: Hex32::from_bytes(ata(claimant, definition).into_value()),
        custody_ata_account_id: Hex32::from_bytes(ata(metadata, definition).into_value()),
        token_program_id: program_id_to_hex(programs::token().id()),
        ata_program_id: program_id_to_hex(programs::ata().id()),
        token_definition_account_id: Hex32::from_bytes(definition.into_value()),
        aggregate_authority_account_id: Hex32::from_bytes(authority.into_value()),
        aggregate_x_only_public_key: Hex32::from_bytes(*authority_key.value()),
        amount: 75,
        refund_at_ms: 1_850_000_000_123,
    })
    .unwrap();
    PrepareWitnessedAssetEscrowV2Request::new(
        MessageContext::new(
            RunId::new("asset-v2-route-run-0001").unwrap(),
            RequestId::new("asset-v2-route-prepare-0001").unwrap(),
            Participant::Maker,
        ),
        runtime(depositor),
        WitnessedLezAssetTermsV2::custom_token(terms),
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
    (format!("http://{address}"), server.start(rpc))
}

#[tokio::test]
#[allow(
    clippy::too_many_lines,
    reason = "one real client journey proves authenticated routes, typed scanner outages, and durable process restart"
)]
async fn authenticated_custom_token_prepare_route_maps_to_the_official_planner() {
    let (depositor, _, _) = account(31);
    let (claimant, _, _) = account(32);
    let (_, _, authority_key) = account(33);
    let request = request(depositor, claimant, &authority_key);
    let descriptor = request.runtime.clone();
    let planner_directory = TempDir::new().unwrap();
    fs::set_permissions(planner_directory.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let planner_state = planner_directory.path();
    let nonce_calls = Arc::new(AtomicUsize::new(0));
    let planner = Arc::new(
        NativeEscrowPlanner::new_durable(
            Participant::Maker,
            PrivateKey::try_new([31; 32]).unwrap(),
            ESCROW_PROGRAM,
            LEGACY_TRANSFER_PROGRAM,
            descriptor.clone(),
            Arc::new(CountingNonce {
                calls: Arc::clone(&nonce_calls),
            }),
            planner_state,
        )
        .unwrap(),
    );
    let (sequencer_endpoint, sequencer) = start_sequencer().await;
    let runtime = Arc::new(BridgeRuntime::new(
        descriptor.clone(),
        planner,
        Arc::new(OfficialNodeRpc::connect(&sequencer_endpoint).unwrap()),
        Arc::new(UnusedIndexer),
    ));
    let directory = TempDir::new().unwrap();
    let server = start_bridge_server(
        BridgeServerConfig::new(
            request.context.run_id.clone(),
            BridgeServerCapability::new(CAPABILITY).unwrap(),
            directory.path().join("bridge-idempotency.json"),
            SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        ),
        runtime,
    )
    .await
    .unwrap();
    let client = BridgeClient::connect(BridgeClientConfig::new(
        server.endpoint(),
        SidecarCapability::new(CAPABILITY).unwrap(),
        request.context.run_id.clone(),
        descriptor.clone(),
        Duration::from_secs(2),
    ))
    .unwrap();

    let result = client
        .prepare_witnessed_asset_escrow_v2(request.clone())
        .await
        .unwrap();
    assert_eq!(result.effects.len(), 3);
    assert_eq!(nonce_calls.load(Ordering::SeqCst), 1);

    let window = lez_bridge_protocol::DiscoveryWindow::new(1, 1).unwrap();
    let initialization = client
        .classify_finalized_witnessed_asset_initialization_v2(
            lez_bridge_protocol::ClassifyFinalizedWitnessedAssetInitializationV2Request::new(
                MessageContext::new(
                    request.context.run_id.clone(),
                    RequestId::new("asset-v2-route-classify-init-0001").unwrap(),
                    Participant::Maker,
                ),
                descriptor.clone(),
                request.terms.clone(),
                result.effects[0].transaction.clone(),
                window,
            ),
        )
        .await
        .unwrap();
    assert!(matches!(
        initialization.outcome,
        lez_bridge_protocol::FinalizedWitnessedAssetScanOutcomeV2::Unavailable {
            reason:
                lez_bridge_protocol::FinalizedWitnessedAssetUnavailableReasonV2::HistoryUnavailable
        }
    ));
    let custody = client
        .classify_finalized_witnessed_asset_custody_creation_v2(
            lez_bridge_protocol::ClassifyFinalizedWitnessedAssetCustodyCreationV2Request::new(
                MessageContext::new(
                    request.context.run_id.clone(),
                    RequestId::new("asset-v2-route-classify-custody-0001").unwrap(),
                    Participant::Maker,
                ),
                descriptor.clone(),
                request.terms.clone(),
                result.effects[1].transaction.clone(),
                window,
            )
            .unwrap(),
        )
        .await
        .unwrap();
    assert!(matches!(
        custody.outcome,
        lez_bridge_protocol::FinalizedWitnessedAssetScanOutcomeV2::Unavailable {
            reason:
                lez_bridge_protocol::FinalizedWitnessedAssetUnavailableReasonV2::HistoryUnavailable
        }
    ));
    let funding = client
        .classify_finalized_witnessed_asset_funding_v2(
            lez_bridge_protocol::ClassifyFinalizedWitnessedAssetFundingV2Request::new(
                MessageContext::new(
                    request.context.run_id.clone(),
                    RequestId::new("asset-v2-route-classify-funding-0001").unwrap(),
                    Participant::Maker,
                ),
                descriptor.clone(),
                request.terms.clone(),
                result.effects[2].transaction.clone(),
                window,
            ),
        )
        .await
        .unwrap();
    assert!(matches!(
        funding.outcome,
        lez_bridge_protocol::FinalizedWitnessedAssetScanOutcomeV2::Unavailable {
            reason:
                lez_bridge_protocol::FinalizedWitnessedAssetUnavailableReasonV2::HistoryUnavailable
        }
    ));

    let observe_escrow = lez_bridge_protocol::ObserveWitnessedAssetEscrowV2Request::new(
        MessageContext::new(
            request.context.run_id.clone(),
            RequestId::new("asset-v2-route-observe-escrow-0001").unwrap(),
            Participant::Maker,
        ),
        descriptor.clone(),
        request.terms.clone(),
        result.effects.clone(),
        window,
    )
    .unwrap();
    assert!(
        client
            .observe_witnessed_asset_escrow_v2(observe_escrow)
            .await
            .is_err()
    );

    let refund_request = lez_bridge_protocol::PrepareWitnessedAssetRefundV2Request::new(
        MessageContext::new(
            request.context.run_id.clone(),
            RequestId::new("asset-v2-route-refund-0001").unwrap(),
            Participant::Maker,
        ),
        descriptor.clone(),
        request.terms.clone(),
    );
    let refund = client
        .prepare_witnessed_asset_refund_v2(refund_request.clone())
        .await
        .unwrap();
    let observe_refund = lez_bridge_protocol::ObserveWitnessedAssetRefundV2Request::new(
        MessageContext::new(
            request.context.run_id.clone(),
            RequestId::new("asset-v2-route-observe-refund-0001").unwrap(),
            Participant::Maker,
        ),
        descriptor.clone(),
        request.terms.clone(),
        lez_bridge_protocol::NativeRefundObservationTarget::StateOnly,
    );
    assert!(
        client
            .observe_witnessed_asset_refund_v2(observe_refund)
            .await
            .is_err()
    );

    server.stop().await.unwrap();
    let planner = Arc::new(
        NativeEscrowPlanner::new_durable(
            Participant::Maker,
            PrivateKey::try_new([31; 32]).unwrap(),
            ESCROW_PROGRAM,
            LEGACY_TRANSFER_PROGRAM,
            descriptor.clone(),
            Arc::new(CountingNonce {
                calls: Arc::clone(&nonce_calls),
            }),
            planner_state,
        )
        .unwrap(),
    );
    let runtime = Arc::new(BridgeRuntime::new(
        descriptor.clone(),
        planner,
        Arc::new(OfficialNodeRpc::connect(&sequencer_endpoint).unwrap()),
        Arc::new(UnusedIndexer),
    ));
    let server = start_bridge_server(
        BridgeServerConfig::new(
            request.context.run_id.clone(),
            BridgeServerCapability::new(CAPABILITY).unwrap(),
            directory.path().join("bridge-idempotency.json"),
            SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        ),
        runtime,
    )
    .await
    .unwrap();
    let client = BridgeClient::connect(BridgeClientConfig::new(
        server.endpoint(),
        SidecarCapability::new(CAPABILITY).unwrap(),
        request.context.run_id.clone(),
        descriptor,
        Duration::from_secs(2),
    ))
    .unwrap();
    let replayed = client
        .prepare_witnessed_asset_escrow_v2(request)
        .await
        .unwrap();
    assert_eq!(replayed, result);
    let replayed_refund = client
        .prepare_witnessed_asset_refund_v2(refund_request)
        .await
        .unwrap();
    assert_eq!(replayed_refund, refund);
    assert_eq!(nonce_calls.load(Ordering::SeqCst), 1);

    server.stop().await.unwrap();
    sequencer.stop().unwrap();
    sequencer.stopped().await;
}
