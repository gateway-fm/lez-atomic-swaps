use std::{sync::Arc, time::Duration};

use async_trait::async_trait;
use lee::{
    AccountId, PrivateKey, PublicKey, PublicTransaction,
    public_transaction::{Message, WitnessSet},
};
use lez_bridge_client::{BridgeClient, BridgeClientConfig, BridgeClientError, SidecarCapability};
use lez_bridge_protocol::{
    DescribeRuntimeRequest, Hex32, MessageContext, Participant, RequestId, RunId,
    RuntimeCompatibility, RuntimeDescriptor,
};
use lez_v0_2_sidecar::{
    DescribeServerCapability, DescribeServerConfig, DescribeServerHandle, HealthProbe,
    OfficialNodeRpc, RuntimeBoundary, RuntimeBoundaryError, RuntimeHealth,
    decode_official_public_transaction, start_describe_server,
};

const CAPABILITY: &str = "v02-describe-test-capability-000001";

fn h(byte: u8) -> Hex32 {
    Hex32::from_bytes([byte; 32])
}

fn signer(byte: u8) -> (AccountId, Hex32) {
    let private_key = PrivateKey::try_new([byte; 32]).unwrap();
    let account_id = AccountId::from(&PublicKey::new_from_private_key(&private_key));
    let protocol_id = Hex32::from_bytes(account_id.into_value());
    (account_id, protocol_id)
}

fn runtime(role: Participant, signer_account_id: Hex32) -> RuntimeDescriptor {
    RuntimeDescriptor::new(
        role,
        RuntimeCompatibility::LeeV0_2_0,
        h(1),
        h(2),
        h(3),
        h(4),
        signer_account_id,
    )
}

fn bridge_client(
    endpoint: &str,
    capability: &str,
    run_id: RunId,
    expected_runtime: RuntimeDescriptor,
) -> BridgeClient {
    BridgeClient::connect(BridgeClientConfig::new(
        endpoint,
        SidecarCapability::new(capability).unwrap(),
        run_id,
        expected_runtime,
        Duration::from_secs(2),
    ))
    .unwrap()
}

async fn start_healthy_taker_server(run_id: RunId) -> (DescribeServerHandle, RuntimeDescriptor) {
    let (official_signer, signer_id) = signer(11);
    let descriptor = runtime(Participant::Taker, signer_id);
    let boundary = RuntimeBoundary::new(
        descriptor.clone(),
        Participant::Taker,
        official_signer,
        Arc::new(FixedHealth {
            channel_id: [2; 32],
            healthy: true,
        }),
    )
    .unwrap();
    let server = start_describe_server(
        DescribeServerConfig::new(run_id, DescribeServerCapability::new(CAPABILITY).unwrap()),
        Arc::new(boundary),
    )
    .await
    .unwrap();
    (server, descriptor)
}

#[derive(Debug)]
struct FixedHealth {
    channel_id: [u8; 32],
    healthy: bool,
}

#[async_trait]
impl HealthProbe for FixedHealth {
    async fn check_health(&self) -> Result<RuntimeHealth, RuntimeBoundaryError> {
        if self.healthy {
            Ok(RuntimeHealth::new(self.channel_id))
        } else {
            Err(RuntimeBoundaryError::NodeUnavailable)
        }
    }
}

#[tokio::test]
async fn accepts_only_the_exact_lee_v0_2_role_signer_and_channel() {
    let (official_signer, signer_id) = signer(7);
    let descriptor = runtime(Participant::Maker, signer_id);
    let boundary = RuntimeBoundary::new(
        descriptor.clone(),
        Participant::Maker,
        official_signer,
        Arc::new(FixedHealth {
            channel_id: [2; 32],
            healthy: true,
        }),
    )
    .unwrap();

    assert_eq!(boundary.describe(), &descriptor);
    boundary.verify_health().await.unwrap();

    let (wrong_signer, _) = signer(8);
    assert_eq!(
        RuntimeBoundary::new(
            descriptor.clone(),
            Participant::Maker,
            wrong_signer,
            Arc::new(FixedHealth {
                channel_id: [2; 32],
                healthy: true,
            }),
        )
        .unwrap_err(),
        RuntimeBoundaryError::WrongSigner,
    );
    assert_eq!(
        RuntimeBoundary::new(
            descriptor.clone(),
            Participant::Taker,
            official_signer,
            Arc::new(FixedHealth {
                channel_id: [2; 32],
                healthy: true,
            }),
        )
        .unwrap_err(),
        RuntimeBoundaryError::WrongRole,
    );

    let mut legacy = descriptor.clone();
    legacy.compatibility = RuntimeCompatibility::NssaV0_1_2;
    assert_eq!(
        RuntimeBoundary::new(
            legacy,
            Participant::Maker,
            official_signer,
            Arc::new(FixedHealth {
                channel_id: [2; 32],
                healthy: true,
            }),
        )
        .unwrap_err(),
        RuntimeBoundaryError::WrongCompatibility,
    );

    let wrong_channel = RuntimeBoundary::new(
        descriptor,
        Participant::Maker,
        official_signer,
        Arc::new(FixedHealth {
            channel_id: [9; 32],
            healthy: true,
        }),
    )
    .unwrap();
    assert_eq!(
        wrong_channel.verify_health().await.unwrap_err(),
        RuntimeBoundaryError::WrongChannel,
    );
}

#[tokio::test]
async fn health_gates_an_authenticated_role_bound_describe_only_server() {
    let (official_signer, signer_id) = signer(11);
    let run_id = RunId::new("v02-describe-run-0001").unwrap();
    let (server, descriptor) = start_healthy_taker_server(run_id.clone()).await;
    let client = bridge_client(
        server.endpoint(),
        CAPABILITY,
        run_id.clone(),
        descriptor.clone(),
    );

    let response = client
        .describe_runtime(DescribeRuntimeRequest::new(MessageContext::new(
            run_id,
            RequestId::new("describe-v02-0001").unwrap(),
            Participant::Taker,
        )))
        .await
        .unwrap();
    assert_eq!(response.runtime, descriptor);
    server.stop().await.unwrap();

    let unhealthy = RuntimeBoundary::new(
        runtime(Participant::Taker, signer_id),
        Participant::Taker,
        official_signer,
        Arc::new(FixedHealth {
            channel_id: [2; 32],
            healthy: false,
        }),
    )
    .unwrap();
    assert!(matches!(
        start_describe_server(
            DescribeServerConfig::new(
                RunId::new("v02-unhealthy-run-0001").unwrap(),
                DescribeServerCapability::new(CAPABILITY).unwrap(),
            ),
            Arc::new(unhealthy),
        )
        .await,
        Err(lez_v0_2_sidecar::DescribeServerError::Runtime(
            RuntimeBoundaryError::NodeUnavailable
        ))
    ));
}

#[tokio::test]
async fn bearer_run_and_role_cross_wiring_fails_at_the_http_boundary() {
    let run_id = RunId::new("v02-describe-run-0001").unwrap();
    let (server, descriptor) = start_healthy_taker_server(run_id.clone()).await;
    let wrong_capability = bridge_client(
        server.endpoint(),
        "wrong-describe-test-capability-0001",
        run_id.clone(),
        descriptor.clone(),
    );
    assert!(matches!(
        wrong_capability
            .describe_runtime(DescribeRuntimeRequest::new(MessageContext::new(
                run_id.clone(),
                RequestId::new("describe-wrong-capability").unwrap(),
                Participant::Taker,
            )))
            .await,
        Err(BridgeClientError::Transport { .. })
    ));

    let wrong_run = RunId::new("v02-describe-wrong-run").unwrap();
    let wrong_run_client = bridge_client(
        server.endpoint(),
        CAPABILITY,
        wrong_run.clone(),
        descriptor.clone(),
    );
    assert!(matches!(
        wrong_run_client
            .describe_runtime(DescribeRuntimeRequest::new(MessageContext::new(
                wrong_run,
                RequestId::new("describe-wrong-run").unwrap(),
                Participant::Taker,
            )))
            .await,
        Err(BridgeClientError::Transport { .. })
    ));

    let wrong_role_runtime = runtime(Participant::Maker, descriptor.signer_account_id);
    let wrong_role_client = bridge_client(
        server.endpoint(),
        CAPABILITY,
        run_id.clone(),
        wrong_role_runtime,
    );
    assert!(matches!(
        wrong_role_client
            .describe_runtime(DescribeRuntimeRequest::new(MessageContext::new(
                run_id,
                RequestId::new("describe-wrong-role").unwrap(),
                Participant::Maker,
            )))
            .await,
        Err(BridgeClientError::Transport { .. })
    ));
    server.stop().await.unwrap();
}

#[test]
fn decodes_only_canonical_signed_official_lee_public_transactions() {
    let private_key = PrivateKey::try_new([17; 32]).unwrap();
    let account_id = AccountId::from(&PublicKey::new_from_private_key(&private_key));
    let message =
        Message::new_preserialized([1; 8], vec![account_id], vec![0_u128.into()], vec![7, 8, 9]);
    let witness = WitnessSet::for_message(&message, &[&private_key]);
    let transaction = PublicTransaction::new(message, witness.clone());
    let exact_bytes = transaction.to_bytes();
    assert_eq!(
        decode_official_public_transaction(&exact_bytes).unwrap(),
        transaction
    );

    let different_message =
        Message::new_preserialized([1; 8], vec![account_id], vec![0_u128.into()], vec![9, 8, 7]);
    let invalid_signature = PublicTransaction::new(different_message, witness);
    assert_eq!(
        decode_official_public_transaction(&invalid_signature.to_bytes()).unwrap_err(),
        RuntimeBoundaryError::InvalidOfficialTransaction
    );
}

#[test]
fn official_node_rpc_accepts_only_explicit_loopback_http_endpoints() {
    for endpoint in [
        "http://localhost:3040/",
        "https://127.0.0.1:3040/",
        "http://127.0.0.1/",
        "http://user@127.0.0.1:3040/",
        "http://127.0.0.1:3040/path",
    ] {
        assert_eq!(
            OfficialNodeRpc::connect(endpoint).unwrap_err(),
            RuntimeBoundaryError::InvalidNodeEndpoint
        );
    }
    assert!(OfficialNodeRpc::connect("http://127.0.0.1:1/").is_ok());
}
