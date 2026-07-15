use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::Duration,
};

use jsonrpsee::{RpcModule, types::ErrorObjectOwned};
use lez_bridge_client::{
    BridgeClient, BridgeClientConfig, BridgeClientError, BridgeOperation, MAX_RPC_BODY_BYTES,
    METHOD_COMPLETE_WITNESSED_CLAIM, METHOD_DESCRIBE_RUNTIME, METHOD_OBSERVE_ESCROW,
    METHOD_OBSERVE_FINALIZED_WITNESSED_CLAIM, METHOD_OBSERVE_NATIVE_REFUND,
    METHOD_OBSERVE_REVEALING_CLAIM, METHOD_OBSERVE_WITNESSED_ESCROW, METHOD_PREPARE_NATIVE_ESCROW,
    METHOD_PREPARE_NATIVE_REFUND, METHOD_PREPARE_REVEALING_CLAIM, METHOD_PREPARE_WITNESSED_CLAIM,
    METHOD_PREPARE_WITNESSED_ESCROW, METHOD_SUBMIT_TRANSACTION, RUN_ID_HEADER, SIDECAR_ROLE_HEADER,
    SidecarCapability,
};
use lez_bridge_protocol::{
    AccountIds, AggregateBip340Signature, ChainClock, ChainPosition, ChainTip,
    CompleteWitnessedClaimRequest, CompleteWitnessedClaimResult, DescribeRuntimeRequest,
    DescribeRuntimeResult, DiscoveryWindow, ErrorCode, ErrorMessage, EscrowObservationTarget,
    EscrowState, ExactMessageBytes, ExactTransactionBytes, FinalizedBlockIdentity,
    FinalizedWitnessedClaimFacts, FundingObservation, Hex32, InitializationObservation,
    MessageContext, NativeCustodyFacts, NativeEscrowAccountObservation, NativeEscrowTerms,
    NativeEscrowTermsInput, NativeRefundObservation, NativeRefundObservationTarget,
    ObserveEscrowRequest, ObserveEscrowResult, ObserveFinalizedWitnessedClaimRequest,
    ObserveFinalizedWitnessedClaimResult, ObserveNativeRefundRequest, ObserveNativeRefundResult,
    ObserveRevealingClaimRequest, ObserveRevealingClaimResult, ObserveWitnessedEscrowRequest,
    ObserveWitnessedEscrowResult, Participant, PrepareNativeEscrowRequest,
    PrepareNativeEscrowResult, PrepareNativeRefundRequest, PrepareNativeRefundResult,
    PrepareRevealingClaimRequest, PrepareRevealingClaimResult, PrepareWitnessedClaimRequest,
    PrepareWitnessedClaimResult, PrepareWitnessedEscrowRequest, PrepareWitnessedEscrowResult,
    PreparedTransaction, PreparedWitnessedClaim, ProtocolErrorReply, RequestId,
    RevealingClaimObservation, RevealingClaimObservationTarget, RevealingPreimage, RunId,
    RuntimeCompatibility, RuntimeDescriptor, SubmissionOutcome, SubmitTransactionRequest,
    SubmitTransactionResult, TransactionId, WitnessedClaimInstructionFacts,
    WitnessedEscrowMetadataFacts, WitnessedFundingObservation, WitnessedInitializationObservation,
    WitnessedNativeEscrowTerms, WitnessedNativeEscrowTermsInput,
};
use secp256k1::{Keypair, Message, Secp256k1, SecretKey};
use serde_json::json;
use sha2::{Digest as _, Sha256};
use tower::ServiceBuilder;
use tower_http::validate_request::ValidateRequestHeaderLayer;

const MAKER_CAPABILITY: &str = "maker-capability-00000000000000000001";
const TAKER_CAPABILITY: &str = "taker-capability-00000000000000000001";
const TEST_RUN: &str = "rrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrr";
const MAX_TRANSACTION_BYTES: usize = 2_000_000;
// Keep a wide scheduling margin so a loaded CI runner dispatches the request
// before the client deadline while the handler still cannot complete in time.
const TIMEOUT_TEST_CLIENT_DEADLINE: Duration = Duration::from_millis(100);
const TIMEOUT_TEST_SERVER_DELAY: Duration = Duration::from_millis(500);
const TIMEOUT_TEST_DRAIN: Duration = Duration::from_millis(520);

#[derive(Clone, Copy, Debug, Default)]
enum Behavior {
    #[default]
    Happy,
    WrongEcho,
    UnknownDescribeField,
    TypedRemoteError,
    DuplicatePrepared,
    WrongSubmitId,
    SlowDescribe,
    SlowPrepare,
    SlowRefundPrepare,
    SlowRefundObserve,
    SlowSubmit,
    MaximumPrepared,
    UnknownRefundField,
    MutatedFinalizedMetadata,
    FinalizedBeforeWindow,
    FinalizedAfterWindow,
    FinalizedTipHashDisagreement,
    MutatedFinalizedSignature,
    DifferentFinalizedSigningKey,
    MutatedFinalizedSignatureMessage,
}

#[derive(Clone, Debug)]
struct Fixture {
    runtime: RuntimeDescriptor,
    behavior: Behavior,
    calls: Arc<Mutex<BTreeMap<&'static str, usize>>>,
}

impl Fixture {
    fn record(&self, method: &'static str) {
        let mut calls = self.calls.lock().expect("call recorder");
        *calls.entry(method).or_default() += 1;
    }

    fn calls(&self, method: &'static str) -> usize {
        *self
            .calls
            .lock()
            .expect("call recorder")
            .get(method)
            .unwrap_or(&0)
    }
}

struct MockSidecar {
    endpoint: String,
    fixture: Fixture,
    handle: jsonrpsee::server::ServerHandle,
}

async fn spawn_sidecar(
    runtime: RuntimeDescriptor,
    capability: &'static str,
    behavior: Behavior,
) -> MockSidecar {
    let fixture = Fixture {
        runtime,
        behavior,
        calls: Arc::default(),
    };
    let authorization = format!("Bearer {capability}");
    let expected_role = match fixture.runtime.sidecar_role {
        Participant::Maker => "maker",
        Participant::Taker => "taker",
    };
    let auth = ServiceBuilder::new()
        .layer(
            ValidateRequestHeaderLayer::has_header_value("authorization", &authorization)
                .expect("valid authorization header name"),
        )
        .layer(
            ValidateRequestHeaderLayer::has_header_value(RUN_ID_HEADER, TEST_RUN)
                .expect("valid run header name"),
        )
        .layer(
            ValidateRequestHeaderLayer::has_header_value(SIDECAR_ROLE_HEADER, expected_role)
                .expect("valid role header name"),
        );
    let server_config = jsonrpsee::server::ServerConfig::builder()
        .max_request_body_size(MAX_RPC_BODY_BYTES)
        .max_response_body_size(MAX_RPC_BODY_BYTES)
        .build();
    let server = jsonrpsee::server::ServerBuilder::with_config(server_config)
        .set_http_middleware(auth)
        .build("127.0.0.1:0")
        .await
        .expect("mock sidecar binds loopback");
    let address = server.local_addr().expect("mock sidecar address");
    let mut module = RpcModule::new(fixture.clone());
    register_methods(&mut module);

    let handle = server.start(module);
    MockSidecar {
        endpoint: format!("http://{address}"),
        fixture,
        handle,
    }
}

fn register_methods(module: &mut RpcModule<Fixture>) {
    module
        .register_async_method(METHOD_DESCRIBE_RUNTIME, |params, fixture, _| async move {
            let request: DescribeRuntimeRequest = params.one()?;
            fixture.record(METHOD_DESCRIBE_RUNTIME);
            if matches!(fixture.behavior, Behavior::SlowDescribe) {
                tokio::time::sleep(TIMEOUT_TEST_SERVER_DELAY).await;
            }
            if matches!(fixture.behavior, Behavior::TypedRemoteError) {
                let reply = ProtocolErrorReply::new(
                    request.context,
                    ErrorCode::Unavailable,
                    ErrorMessage::new("secret-looking remote detail").expect("bounded message"),
                );
                return Err(ErrorObjectOwned::owned(
                    -32_010,
                    "LEZ bridge request failed",
                    Some(reply),
                ));
            }
            let context = response_context(&request.context, fixture.behavior);
            let result = DescribeRuntimeResult::new(context, fixture.runtime.clone());
            let mut value = serde_json::to_value(result).expect("serializable result");
            if matches!(fixture.behavior, Behavior::UnknownDescribeField) {
                value
                    .as_object_mut()
                    .expect("object result")
                    .insert("unexpected".to_owned(), json!(true));
            }
            Ok::<_, ErrorObjectOwned>(value)
        })
        .expect("describe method");
    register_existing_transaction_methods(module);
    register_refund_methods(module);
    register_submit_method(module);
}

fn register_existing_transaction_methods(module: &mut RpcModule<Fixture>) {
    register_witnessed_escrow_method(module);
    register_finalized_witnessed_claim_method(module);
    module
        .register_async_method(
            METHOD_PREPARE_NATIVE_ESCROW,
            |params, fixture, _| async move {
                let request: PrepareNativeEscrowRequest = params.one()?;
                fixture.record(METHOD_PREPARE_NATIVE_ESCROW);
                if matches!(fixture.behavior, Behavior::SlowPrepare) {
                    tokio::time::sleep(TIMEOUT_TEST_SERVER_DELAY).await;
                }
                let context = response_context(&request.context, fixture.behavior);
                let initialization = if matches!(fixture.behavior, Behavior::MaximumPrepared) {
                    prepared_sized(41, 11, MAX_TRANSACTION_BYTES)
                } else {
                    prepared(41, 11)
                };
                let funding = if matches!(fixture.behavior, Behavior::DuplicatePrepared) {
                    prepared(41, 11)
                } else if matches!(fixture.behavior, Behavior::MaximumPrepared) {
                    prepared_sized(42, 12, MAX_TRANSACTION_BYTES)
                } else {
                    prepared(42, 12)
                };
                Ok::<_, ErrorObjectOwned>(PrepareNativeEscrowResult::new(
                    context,
                    initialization,
                    funding,
                ))
            },
        )
        .expect("prepare escrow method");
    module
        .register_method(METHOD_OBSERVE_ESCROW, |params, fixture, _| {
            let request: ObserveEscrowRequest = params.one()?;
            fixture.record(METHOD_OBSERVE_ESCROW);
            Ok::<_, ErrorObjectOwned>(ObserveEscrowResult::new(
                response_context(&request.context, fixture.behavior),
                tip(50),
                InitializationObservation::UnknownOrPending,
                FundingObservation::UnknownOrPending,
                tip(50),
            ))
        })
        .expect("observe escrow method");
    module
        .register_method(METHOD_PREPARE_REVEALING_CLAIM, |params, fixture, _| {
            let request: PrepareRevealingClaimRequest = params.one()?;
            fixture.record(METHOD_PREPARE_REVEALING_CLAIM);
            Ok::<_, ErrorObjectOwned>(PrepareRevealingClaimResult::new(
                response_context(&request.context, fixture.behavior),
                prepared(43, 13),
            ))
        })
        .expect("prepare claim method");
    module
        .register_method(METHOD_PREPARE_WITNESSED_CLAIM, |params, fixture, _| {
            let request: PrepareWitnessedClaimRequest = params.one()?;
            fixture.record(METHOD_PREPARE_WITNESSED_CLAIM);
            let exact_message_bytes = ExactMessageBytes::new(vec![55; 64]).unwrap();
            let mut hasher = Sha256::new();
            hasher.update(b"/LEE/v0.3/Message/Public/\0\0\0\0\0\0\0");
            hasher.update(exact_message_bytes.as_slice());
            Ok::<_, ErrorObjectOwned>(PrepareWitnessedClaimResult::new(
                response_context(&request.context, fixture.behavior),
                PreparedWitnessedClaim::new(
                    request.context.request_id,
                    Hex32::from_bytes(hasher.finalize().into()),
                    exact_message_bytes,
                ),
            ))
        })
        .expect("prepare witnessed claim method");
    module
        .register_method(METHOD_COMPLETE_WITNESSED_CLAIM, |params, fixture, _| {
            let request: CompleteWitnessedClaimRequest = params.one()?;
            fixture.record(METHOD_COMPLETE_WITNESSED_CLAIM);
            Ok::<_, ErrorObjectOwned>(CompleteWitnessedClaimResult::new(
                response_context(&request.context, fixture.behavior),
                prepared(45, 15),
            ))
        })
        .expect("complete witnessed claim method");
    module
        .register_method(METHOD_OBSERVE_REVEALING_CLAIM, |params, fixture, _| {
            let request: ObserveRevealingClaimRequest = params.one()?;
            fixture.record(METHOD_OBSERVE_REVEALING_CLAIM);
            let result = ObserveRevealingClaimResult::new(
                response_context(&request.context, fixture.behavior),
                tip(51),
                RevealingClaimObservation::UnknownOrPending,
                tip(51),
            );
            Ok::<_, ErrorObjectOwned>(serde_json::to_value(result).expect("serializable result"))
        })
        .expect("observe claim method");
}

fn register_finalized_witnessed_claim_method(module: &mut RpcModule<Fixture>) {
    module
        .register_method(
            METHOD_OBSERVE_FINALIZED_WITNESSED_CLAIM,
            |params, fixture, _| {
                let request: ObserveFinalizedWitnessedClaimRequest = params.one()?;
                fixture.record(METHOD_OBSERVE_FINALIZED_WITNESSED_CLAIM);
                let prepared = prepared(45, 15);
                let block_id = match fixture.behavior {
                    Behavior::FinalizedBeforeWindow => request.window.start_height() - 1,
                    Behavior::FinalizedAfterWindow => {
                        request.window.start_height() + u64::from(request.window.max_blocks())
                    }
                    _ => request.window.start_height(),
                };
                let transaction = lez_bridge_protocol::ObservedTransactionFacts::new(
                    prepared.transaction_id,
                    prepared.exact_bytes,
                    ChainPosition::new(Hex32::from_bytes([80; 32]), block_id, 0),
                    AccountIds::new(vec![request.terms.aggregate_authority_account_id()]).unwrap(),
                    true,
                );
                let mut metadata = WitnessedEscrowMetadataFacts::from_witnessed_native_terms(
                    Hex32::from_bytes([82; 32]),
                    request.runtime.escrow_program_id,
                    Hex32::from_bytes([83; 32]),
                    &request.terms,
                    EscrowState::Claimed,
                );
                if matches!(fixture.behavior, Behavior::MutatedFinalizedMetadata) {
                    metadata.status = EscrowState::Funded;
                }
                let custody = NativeCustodyFacts::new(
                    Hex32::from_bytes([83; 32]),
                    request.terms.authenticated_transfer_program_id(),
                    0,
                );
                let aggregate_signature = finalized_signature(&request, fixture.behavior);
                let finalized_height =
                    if matches!(fixture.behavior, Behavior::FinalizedTipHashDisagreement) {
                        block_id
                    } else {
                        61
                    };
                Ok::<_, ErrorObjectOwned>(ObserveFinalizedWitnessedClaimResult::new(
                    response_context(&request.context, fixture.behavior),
                    ChainTip::new(Hex32::from_bytes([81; 32]), finalized_height),
                    FinalizedWitnessedClaimFacts::new(
                        transaction,
                        WitnessedClaimInstructionFacts::new(
                            request.runtime.escrow_program_id,
                            AccountIds::new(vec![
                                Hex32::from_bytes([82; 32]),
                                Hex32::from_bytes([83; 32]),
                                request.terms.claimant_account_id(),
                                request.terms.aggregate_authority_account_id(),
                            ])
                            .unwrap(),
                            request.terms.swap_id(),
                            request.terms.claimant_account_id(),
                            request.terms.aggregate_authority_account_id(),
                            request.claim,
                        ),
                        aggregate_signature,
                        FinalizedBlockIdentity::new(
                            block_id,
                            Hex32::from_bytes([80; 32]),
                            1_850_000_000_060,
                        ),
                        metadata,
                        custody,
                    ),
                ))
            },
        )
        .expect("observe finalized witnessed claim method");
}

fn finalized_signature(
    request: &ObserveFinalizedWitnessedClaimRequest,
    behavior: Behavior,
) -> AggregateBip340Signature {
    let secret_byte = if matches!(behavior, Behavior::DifferentFinalizedSigningKey) {
        12
    } else {
        11
    };
    let digest = if matches!(behavior, Behavior::MutatedFinalizedSignatureMessage) {
        [99; 32]
    } else {
        *request.claim.message_hash.as_bytes()
    };
    let secp = Secp256k1::new();
    let secret = SecretKey::from_slice(&[secret_byte; 32]).expect("fixture secret");
    let keypair = Keypair::from_secret_key(&secp, &secret);
    let mut signature = secp
        .sign_schnorr_no_aux_rand(&Message::from_digest(digest), &keypair)
        .serialize();
    if matches!(behavior, Behavior::MutatedFinalizedSignature) {
        signature[0] ^= 1;
    }
    AggregateBip340Signature::from_bytes(signature)
}

fn register_witnessed_escrow_method(module: &mut RpcModule<Fixture>) {
    module
        .register_method(METHOD_PREPARE_WITNESSED_ESCROW, |params, fixture, _| {
            let request: PrepareWitnessedEscrowRequest = params.one()?;
            fixture.record(METHOD_PREPARE_WITNESSED_ESCROW);
            Ok::<_, ErrorObjectOwned>(PrepareWitnessedEscrowResult::new(
                response_context(&request.context, fixture.behavior),
                prepared(46, 16),
                prepared(47, 17),
            ))
        })
        .expect("prepare witnessed escrow method");
    module
        .register_method(METHOD_OBSERVE_WITNESSED_ESCROW, |params, fixture, _| {
            let request: ObserveWitnessedEscrowRequest = params.one()?;
            fixture.record(METHOD_OBSERVE_WITNESSED_ESCROW);
            Ok::<_, ErrorObjectOwned>(ObserveWitnessedEscrowResult::new(
                response_context(&request.context, fixture.behavior),
                tip(52),
                WitnessedInitializationObservation::UnknownOrPending,
                WitnessedFundingObservation::UnknownOrPending,
                tip(52),
            ))
        })
        .expect("observe witnessed escrow method");
}

fn register_refund_methods(module: &mut RpcModule<Fixture>) {
    module
        .register_async_method(
            METHOD_PREPARE_NATIVE_REFUND,
            |params, fixture, _| async move {
                let request: PrepareNativeRefundRequest = params.one()?;
                fixture.record(METHOD_PREPARE_NATIVE_REFUND);
                if matches!(fixture.behavior, Behavior::SlowRefundPrepare) {
                    tokio::time::sleep(TIMEOUT_TEST_SERVER_DELAY).await;
                }
                if matches!(fixture.behavior, Behavior::TypedRemoteError) {
                    return Err(typed_remote_error(request.context));
                }
                let result = PrepareNativeRefundResult::new(
                    response_context(&request.context, fixture.behavior),
                    if matches!(fixture.behavior, Behavior::MaximumPrepared) {
                        prepared_sized(44, 14, MAX_TRANSACTION_BYTES)
                    } else {
                        prepared(44, 14)
                    },
                );
                Ok::<_, ErrorObjectOwned>(result)
            },
        )
        .expect("prepare refund method");
    module
        .register_async_method(
            METHOD_OBSERVE_NATIVE_REFUND,
            |params, fixture, _| async move {
                let request: ObserveNativeRefundRequest = params.one()?;
                fixture.record(METHOD_OBSERVE_NATIVE_REFUND);
                if matches!(fixture.behavior, Behavior::SlowRefundObserve) {
                    tokio::time::sleep(TIMEOUT_TEST_SERVER_DELAY).await;
                }
                if matches!(fixture.behavior, Behavior::TypedRemoteError) {
                    return Err(typed_remote_error(request.context));
                }
                let result = ObserveNativeRefundResult::new(
                    response_context(&request.context, fixture.behavior),
                    clock(52),
                    NativeEscrowAccountObservation::Absent,
                    NativeRefundObservation::UnknownOrPending,
                    clock(52),
                );
                let mut value = serde_json::to_value(result).expect("serializable result");
                if matches!(fixture.behavior, Behavior::UnknownRefundField) {
                    value
                        .as_object_mut()
                        .expect("object result")
                        .insert("unexpected".to_owned(), json!(true));
                }
                Ok::<_, ErrorObjectOwned>(value)
            },
        )
        .expect("observe refund method");
}

fn typed_remote_error(context: MessageContext) -> ErrorObjectOwned {
    ErrorObjectOwned::owned(
        -32_010,
        "LEZ bridge request failed",
        Some(ProtocolErrorReply::new(
            context,
            ErrorCode::Unavailable,
            ErrorMessage::new("secret-looking remote detail").expect("bounded message"),
        )),
    )
}

fn register_submit_method(module: &mut RpcModule<Fixture>) {
    module
        .register_async_method(METHOD_SUBMIT_TRANSACTION, |params, fixture, _| async move {
            let request: SubmitTransactionRequest = params.one()?;
            fixture.record(METHOD_SUBMIT_TRANSACTION);
            if matches!(fixture.behavior, Behavior::SlowSubmit) {
                tokio::time::sleep(TIMEOUT_TEST_SERVER_DELAY).await;
            }
            let transaction_id = if matches!(fixture.behavior, Behavior::WrongSubmitId) {
                txid(99)
            } else {
                request.transaction.transaction_id
            };
            Ok::<_, ErrorObjectOwned>(SubmitTransactionResult::new(
                response_context(&request.context, fixture.behavior),
                transaction_id,
                SubmissionOutcome::Accepted,
            ))
        })
        .expect("submit method");
}

fn response_context(context: &MessageContext, behavior: Behavior) -> MessageContext {
    if matches!(behavior, Behavior::WrongEcho) {
        MessageContext::new(
            context.run_id.clone(),
            RequestId::new("wrong-response-id").expect("request id"),
            context.sidecar_role,
        )
    } else {
        context.clone()
    }
}

fn hex32(byte: u8) -> Hex32 {
    Hex32::from_bytes([byte; 32])
}

fn txid(byte: u8) -> TransactionId {
    TransactionId::from_bytes([byte; 32])
}

fn prepared(id: u8, body: u8) -> PreparedTransaction {
    prepared_sized(id, body, 64)
}

fn prepared_sized(id: u8, body: u8, size: usize) -> PreparedTransaction {
    PreparedTransaction::new(
        txid(id),
        ExactTransactionBytes::new(vec![body; size]).expect("bounded transaction"),
    )
}

fn tip(byte: u8) -> ChainTip {
    ChainTip::new(hex32(byte), u64::from(byte))
}

fn clock(byte: u8) -> ChainClock {
    ChainClock::new(
        hex32(byte),
        u64::from(byte),
        1_800_000_000_000 + u64::from(byte),
    )
}

fn runtime(role: Participant, byte: u8) -> RuntimeDescriptor {
    RuntimeDescriptor::new(
        role,
        RuntimeCompatibility::NssaV0_1_2,
        hex32(byte),
        hex32(byte + 1),
        hex32(byte + 2),
        hex32(byte + 3),
        hex32(byte + 4),
    )
}

fn terms() -> NativeEscrowTerms {
    NativeEscrowTerms::new(NativeEscrowTermsInput {
        swap_id: hex32(1),
        terms_hash: hex32(2),
        secret_digest: hex32(3),
        depositor: Participant::Maker,
        depositor_account_id: hex32(4),
        claimant: Participant::Taker,
        claimant_account_id: hex32(5),
        amount: 10,
        refund_at_ms: 1_800_000_000_000,
        authenticated_transfer_program_id: hex32(6),
    })
    .expect("valid terms")
}

fn witnessed_terms(runtime: &RuntimeDescriptor) -> WitnessedNativeEscrowTerms {
    WitnessedNativeEscrowTerms::new(WitnessedNativeEscrowTermsInput {
        swap_id: hex32(7),
        terms_hash: hex32(8),
        depositor: Participant::Taker,
        depositor_account_id: hex32(9),
        claimant: Participant::Maker,
        claimant_account_id: runtime.signer_account_id,
        aggregate_authority_account_id: hex32(10),
        aggregate_x_only_public_key: aggregate_x_only_public_key(11),
        amount: 20,
        refund_at_ms: 1_800_000_000_001,
        authenticated_transfer_program_id: hex32(16),
    })
    .expect("valid witnessed terms")
}

fn aggregate_x_only_public_key(secret_byte: u8) -> Hex32 {
    let secp = Secp256k1::new();
    let secret = SecretKey::from_slice(&[secret_byte; 32]).expect("fixture secret");
    let keypair = Keypair::from_secret_key(&secp, &secret);
    Hex32::from_bytes(keypair.x_only_public_key().0.serialize())
}

fn witnessed_deposit_terms(runtime: &RuntimeDescriptor) -> WitnessedNativeEscrowTerms {
    WitnessedNativeEscrowTerms::new(WitnessedNativeEscrowTermsInput {
        swap_id: hex32(17),
        terms_hash: hex32(18),
        depositor: Participant::Maker,
        depositor_account_id: runtime.signer_account_id,
        claimant: Participant::Taker,
        claimant_account_id: hex32(19),
        aggregate_authority_account_id: hex32(20),
        aggregate_x_only_public_key: hex32(21),
        amount: 30,
        refund_at_ms: 1_800_000_000_002,
        authenticated_transfer_program_id: hex32(22),
    })
    .expect("valid witnessed deposit terms")
}

fn context(run: &RunId, role: Participant, suffix: &str) -> MessageContext {
    MessageContext::new(
        run.clone(),
        RequestId::new(format!("request-{suffix}")).expect("request id"),
        role,
    )
}

fn client(
    endpoint: &str,
    capability: &str,
    run: &RunId,
    runtime: RuntimeDescriptor,
    timeout: Duration,
) -> BridgeClient {
    BridgeClient::connect(BridgeClientConfig::new(
        endpoint,
        SidecarCapability::new(capability).expect("valid capability"),
        run.clone(),
        runtime,
        timeout,
    ))
    .expect("valid bridge client")
}

async fn round_trip_refund(
    client: &BridgeClient,
    run: &RunId,
    expected_runtime: &RuntimeDescriptor,
) {
    let refund = client
        .prepare_native_refund(PrepareNativeRefundRequest::new(
            context(run, Participant::Maker, "prepare-refund"),
            expected_runtime.clone(),
            terms(),
        ))
        .await
        .expect("prepare refund");
    let _ = client
        .observe_native_refund(ObserveNativeRefundRequest::new(
            context(run, Participant::Maker, "observe-refund"),
            expected_runtime.clone(),
            terms(),
            NativeRefundObservationTarget::Exact {
                refund_transaction_id: refund.refund.transaction_id,
                window: DiscoveryWindow::new(40, 20).expect("bounded caller window"),
            },
        ))
        .await
        .expect("observe refund");
}

async fn round_trip_witnessed_claim(
    client: &BridgeClient,
    run: &RunId,
    expected_runtime: &RuntimeDescriptor,
    funding_transaction_id: TransactionId,
) -> (PreparedWitnessedClaim, PreparedTransaction) {
    let witnessed = client
        .prepare_witnessed_claim(PrepareWitnessedClaimRequest::new(
            context(run, Participant::Maker, "prepare-witnessed"),
            expected_runtime.clone(),
            witnessed_terms(expected_runtime),
            funding_transaction_id,
        ))
        .await
        .expect("prepare witnessed claim");
    let transcript = witnessed.claim.clone();
    let completed = client
        .complete_witnessed_claim(CompleteWitnessedClaimRequest::new(
            context(run, Participant::Maker, "complete-witnessed"),
            expected_runtime.clone(),
            witnessed.claim,
            AggregateBip340Signature::from_bytes([12; 64]),
        ))
        .await
        .expect("complete witnessed claim")
        .claim;
    (transcript, completed)
}

async fn round_trip_finalized_witnessed_claim(
    client: &BridgeClient,
    run: &RunId,
    expected_runtime: &RuntimeDescriptor,
    transcript: PreparedWitnessedClaim,
    completed: &PreparedTransaction,
) {
    let finalized = client
        .observe_finalized_witnessed_claim(ObserveFinalizedWitnessedClaimRequest::new(
            context(run, Participant::Maker, "observe-finalized-witnessed"),
            expected_runtime.clone(),
            witnessed_terms(expected_runtime),
            transcript,
            completed.transaction_id,
            DiscoveryWindow::new(60, 1).unwrap(),
        ))
        .await
        .expect("observe finalized witnessed claim");
    assert_eq!(
        finalized.claim.transaction.transaction_id,
        completed.transaction_id
    );
}

#[tokio::test]
async fn witnessed_escrow_prepare_is_role_correct_typed_and_does_not_submit() {
    let expected_runtime = runtime(Participant::Maker, 30);
    let sidecar = spawn_sidecar(expected_runtime.clone(), MAKER_CAPABILITY, Behavior::Happy).await;
    let run = RunId::new(TEST_RUN).expect("run id");
    let client = client(
        &sidecar.endpoint,
        MAKER_CAPABILITY,
        &run,
        expected_runtime.clone(),
        Duration::from_secs(1),
    );

    let prepared = client
        .prepare_witnessed_escrow(PrepareWitnessedEscrowRequest::new(
            context(&run, Participant::Maker, "prepare-witnessed-escrow"),
            expected_runtime.clone(),
            witnessed_deposit_terms(&expected_runtime),
        ))
        .await
        .expect("prepare witnessed escrow");

    assert_ne!(
        prepared.initialization.transaction_id,
        prepared.funding.transaction_id
    );
    assert_eq!(sidecar.fixture.calls(METHOD_PREPARE_WITNESSED_ESCROW), 1);
    assert_eq!(sidecar.fixture.calls(METHOD_SUBMIT_TRANSACTION), 0);

    let observed = client
        .observe_witnessed_escrow(ObserveWitnessedEscrowRequest::new(
            context(&run, Participant::Maker, "observe-witnessed-escrow"),
            expected_runtime.clone(),
            witnessed_deposit_terms(&expected_runtime),
            EscrowObservationTarget::Exact {
                initialization_transaction_id: prepared.initialization.transaction_id,
                funding_transaction_id: prepared.funding.transaction_id,
            },
        ))
        .await
        .expect("observe witnessed escrow");
    assert_eq!(observed.tip_before, observed.tip_after);
    assert_eq!(sidecar.fixture.calls(METHOD_OBSERVE_WITNESSED_ESCROW), 1);
    assert_eq!(sidecar.fixture.calls(METHOD_SUBMIT_TRANSACTION), 0);
}

async fn assert_finalized_observation_rejected(behavior: Behavior, suffix: &str) {
    let expected_runtime = runtime(Participant::Maker, 31);
    let sidecar = spawn_sidecar(expected_runtime.clone(), MAKER_CAPABILITY, behavior).await;
    let run = RunId::new(TEST_RUN).expect("run id");
    let client = client(
        &sidecar.endpoint,
        MAKER_CAPABILITY,
        &run,
        expected_runtime.clone(),
        Duration::from_secs(1),
    );
    let (transcript, completed) =
        round_trip_witnessed_claim(&client, &run, &expected_runtime, txid(30)).await;
    let error = client
        .observe_finalized_witnessed_claim(ObserveFinalizedWitnessedClaimRequest::new(
            context(&run, Participant::Maker, suffix),
            expected_runtime.clone(),
            witnessed_terms(&expected_runtime),
            transcript,
            completed.transaction_id,
            DiscoveryWindow::new(60, 1).unwrap(),
        ))
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        BridgeClientError::MalformedObservation {
            operation: BridgeOperation::ObserveFinalizedWitnessedClaim,
        }
    ));
}

#[tokio::test]
async fn finalized_witnessed_claim_rejects_mutated_terminal_facts() {
    assert_finalized_observation_rejected(
        Behavior::MutatedFinalizedMetadata,
        "observe-mutated-finalized",
    )
    .await;
}

#[tokio::test]
async fn finalized_witnessed_claim_rejects_containing_block_outside_requested_window() {
    assert_finalized_observation_rejected(Behavior::FinalizedBeforeWindow, "observe-before-window")
        .await;
    assert_finalized_observation_rejected(Behavior::FinalizedAfterWindow, "observe-after-window")
        .await;
}

#[tokio::test]
async fn finalized_witnessed_claim_rejects_same_height_tip_hash_disagreement() {
    assert_finalized_observation_rejected(
        Behavior::FinalizedTipHashDisagreement,
        "observe-tip-hash-disagreement",
    )
    .await;
}

#[tokio::test]
async fn finalized_witnessed_claim_rejects_signature_key_and_message_mutations() {
    for (behavior, suffix) in [
        (
            Behavior::MutatedFinalizedSignature,
            "observe-mutated-signature",
        ),
        (
            Behavior::DifferentFinalizedSigningKey,
            "observe-different-signing-key",
        ),
        (
            Behavior::MutatedFinalizedSignatureMessage,
            "observe-mutated-signature-message",
        ),
    ] {
        assert_finalized_observation_rejected(behavior, suffix).await;
    }
}

fn assert_all_method_calls(fixture: &Fixture) {
    for method in [
        METHOD_DESCRIBE_RUNTIME,
        METHOD_PREPARE_NATIVE_ESCROW,
        METHOD_OBSERVE_ESCROW,
        METHOD_PREPARE_REVEALING_CLAIM,
        METHOD_OBSERVE_REVEALING_CLAIM,
        METHOD_PREPARE_WITNESSED_CLAIM,
        METHOD_COMPLETE_WITNESSED_CLAIM,
        METHOD_OBSERVE_FINALIZED_WITNESSED_CLAIM,
        METHOD_PREPARE_NATIVE_REFUND,
        METHOD_OBSERVE_NATIVE_REFUND,
        METHOD_SUBMIT_TRANSACTION,
    ] {
        assert_eq!(fixture.calls(method), 1, "method {method}");
        assert!(method.starts_with("lez_bridge.v1."));
    }
}

#[tokio::test]
async fn all_versioned_methods_round_trip_typed_protocol_values_once() {
    let expected_runtime = runtime(Participant::Maker, 10);
    let sidecar = spawn_sidecar(expected_runtime.clone(), MAKER_CAPABILITY, Behavior::Happy).await;
    let run = RunId::new(TEST_RUN).expect("run id");
    let client = client(
        &sidecar.endpoint,
        MAKER_CAPABILITY,
        &run,
        expected_runtime.clone(),
        Duration::from_secs(1),
    );

    let described = client
        .describe_runtime(DescribeRuntimeRequest::new(context(
            &run,
            Participant::Maker,
            "describe",
        )))
        .await
        .expect("describe");
    assert_eq!(described.runtime, expected_runtime);

    let escrow = client
        .prepare_native_escrow(PrepareNativeEscrowRequest::new(
            context(&run, Participant::Maker, "prepare-escrow"),
            expected_runtime.clone(),
            terms(),
        ))
        .await
        .expect("prepare escrow");
    assert_ne!(
        escrow.initialization.transaction_id,
        escrow.funding.transaction_id
    );

    let _ = client
        .observe_escrow(ObserveEscrowRequest::new(
            context(&run, Participant::Maker, "observe-escrow"),
            expected_runtime.clone(),
            terms(),
            EscrowObservationTarget::Exact {
                initialization_transaction_id: escrow.initialization.transaction_id,
                funding_transaction_id: escrow.funding.transaction_id,
            },
        ))
        .await
        .expect("observe escrow");

    let claim = client
        .prepare_revealing_claim(PrepareRevealingClaimRequest::new(
            context(&run, Participant::Maker, "prepare-claim"),
            expected_runtime.clone(),
            terms(),
            escrow.funding.transaction_id,
            RevealingPreimage::new([7; 32]),
        ))
        .await
        .expect("prepare claim");

    let _ = client
        .observe_revealing_claim(ObserveRevealingClaimRequest::new(
            context(&run, Participant::Maker, "observe-claim"),
            expected_runtime.clone(),
            terms(),
            RevealingClaimObservationTarget::Exact {
                claim_transaction_id: claim.claim.transaction_id,
            },
        ))
        .await
        .expect("observe claim");

    let (witnessed_transcript, completed_witnessed) = round_trip_witnessed_claim(
        &client,
        &run,
        &expected_runtime,
        escrow.funding.transaction_id,
    )
    .await;

    round_trip_finalized_witnessed_claim(
        &client,
        &run,
        &expected_runtime,
        witnessed_transcript,
        &completed_witnessed,
    )
    .await;

    round_trip_refund(&client, &run, &expected_runtime).await;

    let expected_submission_id = completed_witnessed.transaction_id;
    let submitted = client
        .submit_transaction(SubmitTransactionRequest::new(
            context(&run, Participant::Maker, "submit"),
            expected_runtime.clone(),
            completed_witnessed,
        ))
        .await
        .expect("submit");
    assert_eq!(submitted.transaction_id, expected_submission_id);
    assert_eq!(submitted.outcome, SubmissionOutcome::Accepted);

    assert_all_method_calls(&sidecar.fixture);
}

#[tokio::test]
async fn distinct_actor_capabilities_roles_and_contexts_fail_closed_when_cross_wired() {
    let maker_runtime = runtime(Participant::Maker, 20);
    let taker_runtime = runtime(Participant::Taker, 40);
    let maker = spawn_sidecar(maker_runtime.clone(), MAKER_CAPABILITY, Behavior::Happy).await;
    let taker = spawn_sidecar(taker_runtime.clone(), TAKER_CAPABILITY, Behavior::Happy).await;
    let run = RunId::new(TEST_RUN).expect("run id");
    let maker_client = client(
        &maker.endpoint,
        MAKER_CAPABILITY,
        &run,
        maker_runtime.clone(),
        Duration::from_secs(1),
    );
    let taker_client = client(
        &taker.endpoint,
        TAKER_CAPABILITY,
        &run,
        taker_runtime,
        Duration::from_secs(1),
    );

    let _ = maker_client
        .describe_runtime(DescribeRuntimeRequest::new(context(
            &run,
            Participant::Maker,
            "maker-ok",
        )))
        .await
        .expect("maker isolated");
    let _ = taker_client
        .describe_runtime(DescribeRuntimeRequest::new(context(
            &run,
            Participant::Taker,
            "taker-ok",
        )))
        .await
        .expect("taker isolated");

    let cross_capability = client(
        &maker.endpoint,
        TAKER_CAPABILITY,
        &run,
        maker_runtime.clone(),
        Duration::from_secs(1),
    );
    assert!(matches!(
        cross_capability
            .describe_runtime(DescribeRuntimeRequest::new(context(
                &run,
                Participant::Maker,
                "wrong-cap",
            )))
            .await,
        Err(BridgeClientError::Transport { .. })
    ));
}

#[tokio::test]
async fn run_role_and_complete_runtime_cross_wiring_fails_closed() {
    let maker_runtime = runtime(Participant::Maker, 20);
    let maker = spawn_sidecar(maker_runtime.clone(), MAKER_CAPABILITY, Behavior::Happy).await;
    let run = RunId::new(TEST_RUN).expect("run id");
    let maker_client = client(
        &maker.endpoint,
        MAKER_CAPABILITY,
        &run,
        maker_runtime.clone(),
        Duration::from_secs(1),
    );

    let wrong_run = RunId::new("wrong-isolated-run").expect("run id");
    let run_crosswire = client(
        &maker.endpoint,
        MAKER_CAPABILITY,
        &wrong_run,
        maker_runtime.clone(),
        Duration::from_secs(1),
    );
    assert!(matches!(
        run_crosswire
            .describe_runtime(DescribeRuntimeRequest::new(context(
                &wrong_run,
                Participant::Maker,
                "wrong-run",
            )))
            .await,
        Err(BridgeClientError::Transport { .. })
    ));

    let role_crosswire = client(
        &maker.endpoint,
        MAKER_CAPABILITY,
        &run,
        runtime(Participant::Taker, 40),
        Duration::from_secs(1),
    );
    assert!(matches!(
        role_crosswire
            .describe_runtime(DescribeRuntimeRequest::new(context(
                &run,
                Participant::Taker,
                "wrong-role-header",
            )))
            .await,
        Err(BridgeClientError::Transport { .. })
    ));

    let wrong_runtime = runtime(Participant::Maker, 21);
    let runtime_crosswire = client(
        &maker.endpoint,
        MAKER_CAPABILITY,
        &run,
        wrong_runtime,
        Duration::from_secs(1),
    );
    assert!(matches!(
        runtime_crosswire
            .describe_runtime(DescribeRuntimeRequest::new(context(
                &run,
                Participant::Maker,
                "wrong-runtime",
            )))
            .await,
        Err(BridgeClientError::RuntimeMismatch)
    ));

    let before = maker.fixture.calls(METHOD_DESCRIBE_RUNTIME);
    assert!(matches!(
        maker_client
            .describe_runtime(DescribeRuntimeRequest::new(context(
                &run,
                Participant::Taker,
                "wrong-role",
            )))
            .await,
        Err(BridgeClientError::RequestContextMismatch { .. })
    ));
    assert_eq!(maker.fixture.calls(METHOD_DESCRIBE_RUNTIME), before);
}

#[tokio::test]
async fn wrong_echo_replay_unknown_fields_and_invalid_prepared_results_are_rejected() {
    let expected_runtime = runtime(Participant::Maker, 60);
    let run = RunId::new(TEST_RUN).expect("run id");

    let wrong_echo = spawn_sidecar(
        expected_runtime.clone(),
        MAKER_CAPABILITY,
        Behavior::WrongEcho,
    )
    .await;
    let wrong_echo_client = client(
        &wrong_echo.endpoint,
        MAKER_CAPABILITY,
        &run,
        expected_runtime.clone(),
        Duration::from_secs(1),
    );
    assert!(matches!(
        wrong_echo_client
            .describe_runtime(DescribeRuntimeRequest::new(context(
                &run,
                Participant::Maker,
                "wrong-echo",
            )))
            .await,
        Err(BridgeClientError::ResponseContextMismatch { .. })
    ));

    let strict = spawn_sidecar(
        expected_runtime.clone(),
        MAKER_CAPABILITY,
        Behavior::UnknownDescribeField,
    )
    .await;
    let strict_client = client(
        &strict.endpoint,
        MAKER_CAPABILITY,
        &run,
        expected_runtime.clone(),
        Duration::from_secs(1),
    );
    assert!(matches!(
        strict_client
            .describe_runtime(DescribeRuntimeRequest::new(context(
                &run,
                Participant::Maker,
                "unknown-field",
            )))
            .await,
        Err(BridgeClientError::InvalidResponse { .. })
    ));

    let duplicate = spawn_sidecar(
        expected_runtime.clone(),
        MAKER_CAPABILITY,
        Behavior::DuplicatePrepared,
    )
    .await;
    let duplicate_client = client(
        &duplicate.endpoint,
        MAKER_CAPABILITY,
        &run,
        expected_runtime.clone(),
        Duration::from_secs(1),
    );
    assert!(matches!(
        duplicate_client
            .prepare_native_escrow(PrepareNativeEscrowRequest::new(
                context(&run, Participant::Maker, "duplicate-prepared"),
                expected_runtime.clone(),
                terms(),
            ))
            .await,
        Err(BridgeClientError::MalformedPreparedTransaction { .. })
    ));

    let replay = spawn_sidecar(expected_runtime.clone(), MAKER_CAPABILITY, Behavior::Happy).await;
    let replay_client = client(
        &replay.endpoint,
        MAKER_CAPABILITY,
        &run,
        expected_runtime,
        Duration::from_secs(1),
    );
    let replay_context = context(&run, Participant::Maker, "same-request");
    let _ = replay_client
        .describe_runtime(DescribeRuntimeRequest::new(replay_context.clone()))
        .await
        .expect("first request");
    assert!(matches!(
        replay_client
            .describe_runtime(DescribeRuntimeRequest::new(replay_context))
            .await,
        Err(BridgeClientError::RequestIdReused { .. })
    ));
    assert_eq!(replay.fixture.calls(METHOD_DESCRIBE_RUNTIME), 1);
}

#[tokio::test]
async fn refund_echo_and_strict_response_checks_fail_closed() {
    let expected_runtime = runtime(Participant::Maker, 70);
    let run = RunId::new(TEST_RUN).expect("run id");
    let wrong_echo = spawn_sidecar(
        expected_runtime.clone(),
        MAKER_CAPABILITY,
        Behavior::WrongEcho,
    )
    .await;
    let wrong_echo_client = client(
        &wrong_echo.endpoint,
        MAKER_CAPABILITY,
        &run,
        expected_runtime.clone(),
        Duration::from_secs(1),
    );
    assert!(matches!(
        wrong_echo_client
            .prepare_native_refund(PrepareNativeRefundRequest::new(
                context(&run, Participant::Maker, "refund-wrong-echo"),
                expected_runtime.clone(),
                terms(),
            ))
            .await,
        Err(BridgeClientError::ResponseContextMismatch {
            operation: BridgeOperation::PrepareNativeRefund,
        })
    ));
    assert!(matches!(
        wrong_echo_client
            .observe_native_refund(ObserveNativeRefundRequest::new(
                context(&run, Participant::Maker, "observe-refund-wrong-echo"),
                expected_runtime.clone(),
                terms(),
                NativeRefundObservationTarget::StateOnly,
            ))
            .await,
        Err(BridgeClientError::ResponseContextMismatch {
            operation: BridgeOperation::ObserveNativeRefund,
        })
    ));
    assert_eq!(wrong_echo.fixture.calls(METHOD_PREPARE_NATIVE_REFUND), 1);
    assert_eq!(wrong_echo.fixture.calls(METHOD_OBSERVE_NATIVE_REFUND), 1);

    let strict = spawn_sidecar(
        expected_runtime.clone(),
        MAKER_CAPABILITY,
        Behavior::UnknownRefundField,
    )
    .await;
    let strict_client = client(
        &strict.endpoint,
        MAKER_CAPABILITY,
        &run,
        expected_runtime.clone(),
        Duration::from_secs(1),
    );
    assert!(matches!(
        strict_client
            .observe_native_refund(ObserveNativeRefundRequest::new(
                context(&run, Participant::Maker, "refund-unknown-field"),
                expected_runtime.clone(),
                terms(),
                NativeRefundObservationTarget::StateOnly,
            ))
            .await,
        Err(BridgeClientError::InvalidResponse {
            operation: BridgeOperation::ObserveNativeRefund,
        })
    ));
    assert_eq!(strict.fixture.calls(METHOD_OBSERVE_NATIVE_REFUND), 1);
}

#[tokio::test]
async fn refund_runtime_role_and_request_id_checks_fail_before_transport() {
    let expected_runtime = runtime(Participant::Maker, 70);
    let run = RunId::new(TEST_RUN).expect("run id");
    let happy = spawn_sidecar(expected_runtime.clone(), MAKER_CAPABILITY, Behavior::Happy).await;
    let happy_client = client(
        &happy.endpoint,
        MAKER_CAPABILITY,
        &run,
        expected_runtime.clone(),
        Duration::from_secs(1),
    );
    let shared = context(&run, Participant::Maker, "refund-shared-id");
    let _ = happy_client
        .prepare_native_refund(PrepareNativeRefundRequest::new(
            shared.clone(),
            expected_runtime.clone(),
            terms(),
        ))
        .await
        .expect("first request id use");
    assert!(matches!(
        happy_client
            .observe_native_refund(ObserveNativeRefundRequest::new(
                shared,
                expected_runtime.clone(),
                terms(),
                NativeRefundObservationTarget::StateOnly,
            ))
            .await,
        Err(BridgeClientError::RequestIdReused {
            operation: BridgeOperation::ObserveNativeRefund,
        })
    ));
    assert_eq!(happy.fixture.calls(METHOD_PREPARE_NATIVE_REFUND), 1);
    assert_eq!(happy.fixture.calls(METHOD_OBSERVE_NATIVE_REFUND), 0);

    assert!(matches!(
        happy_client
            .prepare_native_refund(PrepareNativeRefundRequest::new(
                context(&run, Participant::Maker, "refund-wrong-runtime"),
                runtime(Participant::Maker, 71),
                terms(),
            ))
            .await,
        Err(BridgeClientError::RequestContextMismatch {
            operation: BridgeOperation::PrepareNativeRefund,
        })
    ));
    assert!(matches!(
        happy_client
            .observe_native_refund(ObserveNativeRefundRequest::new(
                context(&run, Participant::Taker, "refund-wrong-role"),
                expected_runtime,
                terms(),
                NativeRefundObservationTarget::StateOnly,
            ))
            .await,
        Err(BridgeClientError::RequestContextMismatch {
            operation: BridgeOperation::ObserveNativeRefund,
        })
    ));
    assert_eq!(happy.fixture.calls(METHOD_PREPARE_NATIVE_REFUND), 1);
    assert_eq!(happy.fixture.calls(METHOD_OBSERVE_NATIVE_REFUND), 0);
}

#[tokio::test]
async fn refund_typed_remote_failures_are_single_attempt() {
    let expected_runtime = runtime(Participant::Maker, 75);
    let run = RunId::new(TEST_RUN).expect("run id");
    let remote = spawn_sidecar(
        expected_runtime.clone(),
        MAKER_CAPABILITY,
        Behavior::TypedRemoteError,
    )
    .await;
    let remote_client = client(
        &remote.endpoint,
        MAKER_CAPABILITY,
        &run,
        expected_runtime.clone(),
        Duration::from_secs(1),
    );
    for error in [
        remote_client
            .prepare_native_refund(PrepareNativeRefundRequest::new(
                context(&run, Participant::Maker, "refund-remote"),
                expected_runtime.clone(),
                terms(),
            ))
            .await
            .expect_err("typed refund preparation error"),
        remote_client
            .observe_native_refund(ObserveNativeRefundRequest::new(
                context(&run, Participant::Maker, "observe-refund-remote"),
                expected_runtime.clone(),
                terms(),
                NativeRefundObservationTarget::StateOnly,
            ))
            .await
            .expect_err("typed refund observation error"),
    ] {
        let BridgeClientError::Remote(remote_error) = error else {
            panic!("wrong error class");
        };
        assert_eq!(remote_error.code(), ErrorCode::Unavailable);
    }
    assert_eq!(remote.fixture.calls(METHOD_PREPARE_NATIVE_REFUND), 1);
    assert_eq!(remote.fixture.calls(METHOD_OBSERVE_NATIVE_REFUND), 1);
}

#[tokio::test]
async fn refund_timeouts_are_single_attempt() {
    let expected_runtime = runtime(Participant::Maker, 75);
    let run = RunId::new(TEST_RUN).expect("run id");
    let slow_prepare = spawn_sidecar(
        expected_runtime.clone(),
        MAKER_CAPABILITY,
        Behavior::SlowRefundPrepare,
    )
    .await;
    let slow_prepare_client = client(
        &slow_prepare.endpoint,
        MAKER_CAPABILITY,
        &run,
        expected_runtime.clone(),
        TIMEOUT_TEST_CLIENT_DEADLINE,
    );
    assert!(matches!(
        slow_prepare_client
            .prepare_native_refund(PrepareNativeRefundRequest::new(
                context(&run, Participant::Maker, "refund-timeout"),
                expected_runtime.clone(),
                terms(),
            ))
            .await,
        Err(BridgeClientError::Timeout {
            operation: BridgeOperation::PrepareNativeRefund,
        })
    ));
    tokio::time::sleep(TIMEOUT_TEST_DRAIN).await;
    assert_eq!(slow_prepare.fixture.calls(METHOD_PREPARE_NATIVE_REFUND), 1);

    let slow_observe = spawn_sidecar(
        expected_runtime.clone(),
        MAKER_CAPABILITY,
        Behavior::SlowRefundObserve,
    )
    .await;
    let slow_observe_client = client(
        &slow_observe.endpoint,
        MAKER_CAPABILITY,
        &run,
        expected_runtime.clone(),
        TIMEOUT_TEST_CLIENT_DEADLINE,
    );
    assert!(matches!(
        slow_observe_client
            .observe_native_refund(ObserveNativeRefundRequest::new(
                context(&run, Participant::Maker, "observe-refund-timeout"),
                expected_runtime.clone(),
                terms(),
                NativeRefundObservationTarget::StateOnly,
            ))
            .await,
        Err(BridgeClientError::Timeout {
            operation: BridgeOperation::ObserveNativeRefund,
        })
    ));
    tokio::time::sleep(TIMEOUT_TEST_DRAIN).await;
    assert_eq!(slow_observe.fixture.calls(METHOD_OBSERVE_NATIVE_REFUND), 1);
}

#[tokio::test]
async fn refund_stopped_transport_is_not_retried() {
    let expected_runtime = runtime(Participant::Maker, 75);
    let run = RunId::new(TEST_RUN).expect("run id");
    let stopped = spawn_sidecar(expected_runtime.clone(), MAKER_CAPABILITY, Behavior::Happy).await;
    let endpoint = stopped.endpoint.clone();
    let fixture = stopped.fixture.clone();
    stopped.handle.stop().expect("stop mock sidecar");
    stopped.handle.stopped().await;
    let stopped_client = client(
        &endpoint,
        MAKER_CAPABILITY,
        &run,
        expected_runtime.clone(),
        Duration::from_millis(50),
    );
    let stopped_context = context(&run, Participant::Maker, "refund-transport");
    assert!(matches!(
        stopped_client
            .prepare_native_refund(PrepareNativeRefundRequest::new(
                stopped_context.clone(),
                expected_runtime.clone(),
                terms(),
            ))
            .await,
        Err(BridgeClientError::Transport {
            operation: BridgeOperation::PrepareNativeRefund,
        })
    ));
    assert!(matches!(
        stopped_client
            .prepare_native_refund(PrepareNativeRefundRequest::new(
                stopped_context,
                expected_runtime,
                terms(),
            ))
            .await,
        Err(BridgeClientError::RequestIdReused {
            operation: BridgeOperation::PrepareNativeRefund,
        })
    ));
    assert_eq!(fixture.calls(METHOD_PREPARE_NATIVE_REFUND), 0);
}

#[tokio::test]
async fn timeout_remote_error_and_unknown_submit_outcome_remain_distinct_without_retries() {
    let expected_runtime = runtime(Participant::Maker, 80);
    let run = RunId::new(TEST_RUN).expect("run id");

    let slow = spawn_sidecar(
        expected_runtime.clone(),
        MAKER_CAPABILITY,
        Behavior::SlowDescribe,
    )
    .await;
    let slow_client = client(
        &slow.endpoint,
        MAKER_CAPABILITY,
        &run,
        expected_runtime.clone(),
        TIMEOUT_TEST_CLIENT_DEADLINE,
    );
    assert!(matches!(
        slow_client
            .describe_runtime(DescribeRuntimeRequest::new(context(
                &run,
                Participant::Maker,
                "timeout",
            )))
            .await,
        Err(BridgeClientError::Timeout { .. })
    ));
    tokio::time::sleep(TIMEOUT_TEST_DRAIN).await;
    assert_eq!(slow.fixture.calls(METHOD_DESCRIBE_RUNTIME), 1);

    let remote = spawn_sidecar(
        expected_runtime.clone(),
        MAKER_CAPABILITY,
        Behavior::TypedRemoteError,
    )
    .await;
    let remote_client = client(
        &remote.endpoint,
        MAKER_CAPABILITY,
        &run,
        expected_runtime.clone(),
        Duration::from_secs(1),
    );
    let error = remote_client
        .describe_runtime(DescribeRuntimeRequest::new(context(
            &run,
            Participant::Maker,
            "remote",
        )))
        .await
        .expect_err("typed remote error");
    let BridgeClientError::Remote(remote) = &error else {
        panic!("wrong error class: {error:?}");
    };
    assert_eq!(remote.code(), ErrorCode::Unavailable);
    assert_eq!(remote.message().as_str(), "secret-looking remote detail");
    assert!(!format!("{error:?} {error}").contains("secret-looking"));

    let wrong_submit = spawn_sidecar(
        expected_runtime.clone(),
        MAKER_CAPABILITY,
        Behavior::WrongSubmitId,
    )
    .await;
    let submit_client = client(
        &wrong_submit.endpoint,
        MAKER_CAPABILITY,
        &run,
        expected_runtime.clone(),
        Duration::from_secs(1),
    );
    assert!(matches!(
        submit_client
            .submit_transaction(SubmitTransactionRequest::new(
                context(&run, Participant::Maker, "wrong-submit-id"),
                expected_runtime,
                prepared(43, 13),
            ))
            .await,
        Err(BridgeClientError::SubmitTransactionIdMismatch)
    ));
    assert_eq!(wrong_submit.fixture.calls(METHOD_SUBMIT_TRANSACTION), 1);
}

#[tokio::test]
async fn randomized_prepare_and_exact_submit_timeouts_are_single_attempt_unknown_outcomes() {
    let expected_runtime = runtime(Participant::Maker, 90);
    let run = RunId::new(TEST_RUN).expect("run id");

    let slow_prepare = spawn_sidecar(
        expected_runtime.clone(),
        MAKER_CAPABILITY,
        Behavior::SlowPrepare,
    )
    .await;
    let prepare_client = client(
        &slow_prepare.endpoint,
        MAKER_CAPABILITY,
        &run,
        expected_runtime.clone(),
        TIMEOUT_TEST_CLIENT_DEADLINE,
    );
    assert!(matches!(
        prepare_client
            .prepare_native_escrow(PrepareNativeEscrowRequest::new(
                context(&run, Participant::Maker, "prepare-timeout"),
                expected_runtime.clone(),
                terms(),
            ))
            .await,
        Err(BridgeClientError::Timeout { .. })
    ));
    tokio::time::sleep(TIMEOUT_TEST_DRAIN).await;
    assert_eq!(slow_prepare.fixture.calls(METHOD_PREPARE_NATIVE_ESCROW), 1);

    let slow_submit = spawn_sidecar(
        expected_runtime.clone(),
        MAKER_CAPABILITY,
        Behavior::SlowSubmit,
    )
    .await;
    let submit_client = client(
        &slow_submit.endpoint,
        MAKER_CAPABILITY,
        &run,
        expected_runtime.clone(),
        TIMEOUT_TEST_CLIENT_DEADLINE,
    );
    assert!(matches!(
        submit_client
            .submit_transaction(SubmitTransactionRequest::new(
                context(&run, Participant::Maker, "submit-timeout"),
                expected_runtime,
                prepared(44, 14),
            ))
            .await,
        Err(BridgeClientError::Timeout { .. })
    ));
    tokio::time::sleep(TIMEOUT_TEST_DRAIN).await;
    assert_eq!(slow_submit.fixture.calls(METHOD_SUBMIT_TRANSACTION), 1);
}

#[tokio::test]
async fn maximum_prepare_pair_fits_transport_while_over_protocol_bytes_are_rejected() {
    assert_eq!(TEST_RUN.len(), 64);
    let expected_runtime = runtime(Participant::Maker, 95);
    let sidecar = spawn_sidecar(
        expected_runtime.clone(),
        MAKER_CAPABILITY,
        Behavior::MaximumPrepared,
    )
    .await;
    let run = RunId::new(TEST_RUN).expect("maximum-width run id");
    let client = client(
        &sidecar.endpoint,
        MAKER_CAPABILITY,
        &run,
        expected_runtime.clone(),
        Duration::from_secs(10),
    );
    let maximum_request_id =
        RequestId::new("qqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqq")
            .expect("maximum-width request id");
    let result = client
        .prepare_native_escrow(PrepareNativeEscrowRequest::new(
            MessageContext::new(run, maximum_request_id, Participant::Maker),
            expected_runtime.clone(),
            terms(),
        ))
        .await
        .expect("two maximum protocol transactions fit the transport bound");

    assert_eq!(
        result.initialization.exact_bytes.as_slice().len(),
        MAX_TRANSACTION_BYTES
    );
    assert_eq!(
        result.funding.exact_bytes.as_slice().len(),
        MAX_TRANSACTION_BYTES
    );
    let maximum_envelope = serde_json::to_vec(&json!({
        "jsonrpc": "2.0",
        "result": &result,
        "id": u64::MAX,
    }))
    .expect("maximum response envelope serializes");
    assert_eq!(maximum_envelope.len(), 5_333_829);
    assert!(maximum_envelope.len() > 5_333_336);
    assert!(maximum_envelope.len() <= MAX_RPC_BODY_BYTES as usize);
    assert_eq!(MAX_RPC_BODY_BYTES, 5_500_000);

    let maximum_refund = client
        .prepare_native_refund(PrepareNativeRefundRequest::new(
            context(
                &RunId::new(TEST_RUN).expect("run id"),
                Participant::Maker,
                "maximum-refund",
            ),
            expected_runtime,
            terms(),
        ))
        .await
        .expect("one maximum refund transaction fits the transport bound");
    assert_eq!(
        maximum_refund.refund.exact_bytes.as_slice().len(),
        MAX_TRANSACTION_BYTES
    );

    assert!(ExactTransactionBytes::new(vec![0; MAX_TRANSACTION_BYTES + 1]).is_err());
}

#[test]
fn endpoint_and_capability_configuration_is_loopback_only_bounded_and_redacted() {
    let expected_runtime = runtime(Participant::Maker, 100);
    let run = RunId::new("config-run-00001").expect("run id");
    for endpoint in [
        "http://localhost:1234",
        "http://192.0.2.1:1234",
        "https://127.0.0.1:1234",
        "http://127.0.0.1",
        "http://127.0.0.1:0",
        "http://user@127.0.0.1:1234",
        "http://127.0.0.1:1234/path",
        "http://127.0.0.1:1234?proxy=elsewhere",
        "http://127.0.0.1:1234#fragment",
    ] {
        let result = BridgeClient::connect(BridgeClientConfig::new(
            endpoint,
            SidecarCapability::new(MAKER_CAPABILITY).expect("valid capability"),
            run.clone(),
            expected_runtime.clone(),
            Duration::from_secs(1),
        ));
        assert!(result.is_err(), "accepted endpoint {endpoint}");
    }

    assert!(SidecarCapability::new("short").is_err());
    assert!(SidecarCapability::new("capability with whitespace 00000000000000").is_err());
    let capability = SidecarCapability::new(MAKER_CAPABILITY).expect("valid capability");
    assert_eq!(format!("{capability:?}"), "SidecarCapability([REDACTED])");
    let config = BridgeClientConfig::new(
        "http://127.0.0.1:1234",
        capability,
        run,
        expected_runtime,
        Duration::from_secs(1),
    );
    assert!(!format!("{config:?}").contains(MAKER_CAPABILITY));
}

#[test]
fn sensitive_protocol_values_are_not_exposed_by_debug() {
    let request = PrepareRevealingClaimRequest::new(
        MessageContext::new(
            RunId::new("redaction-run-01").expect("run id"),
            RequestId::new("redaction-request").expect("request id"),
            Participant::Maker,
        ),
        runtime(Participant::Maker, 110),
        terms(),
        txid(4),
        RevealingPreimage::new([0xab; 32]),
    );
    let prepared = prepared(9, 0xcd);
    assert!(!format!("{request:?}").contains(&"ab".repeat(32)));
    assert!(!format!("{prepared:?}").contains(&"cd".repeat(64)));
}
