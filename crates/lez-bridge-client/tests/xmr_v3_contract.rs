use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::Duration,
};

use jsonrpsee::RpcModule;
use lez_bridge_client::{
    BridgeClient, BridgeClientConfig, BridgeClientError, BridgeOperation, ConfigurationError,
    METHOD_CLASSIFY_FINALIZED_NATIVE_XMR_EFFECT_V3, METHOD_COMPLETE_NATIVE_XMR_CLAIM_V3,
    METHOD_COMPLETE_NATIVE_XMR_REFUND_V3, METHOD_PREPARE_CURRENT_PROFILE_CLOCK,
    METHOD_PREPARE_NATIVE_XMR_CLAIM_AUTHORIZATION_V3, METHOD_PREPARE_NATIVE_XMR_CLAIM_V3,
    METHOD_PREPARE_NATIVE_XMR_ESCROW_V3, METHOD_PREPARE_NATIVE_XMR_PUNISH_V3,
    METHOD_PREPARE_NATIVE_XMR_REFUND_V3, METHOD_SUBMIT_NATIVE_XMR_CLAIM_AUTHORIZATION_V3,
    METHOD_VERIFY_CURRENT_PROFILE_CLOCK, RUN_ID_HEADER, SIDECAR_ROLE_HEADER, SidecarCapability,
    XmrReleaseClient,
};
use lez_bridge_protocol::{
    AggregateBip340Signature, ChainClock, ClassifyFinalizedNativeXmrEffectV3Request,
    ClassifyFinalizedNativeXmrEffectV3Result, CompleteNativeXmrClaimV3Request,
    CompleteNativeXmrClaimV3Result, CompleteNativeXmrRefundV3Request,
    CompleteNativeXmrRefundV3Result, CurrentProfileClockAccountSnapshot, DiscoveryWindow,
    ExactMessageBytes, ExactTransactionBytes, FinalizedNativeXmrScanOutcomeV3,
    FinalizedNativeXmrTransactionTargetV3, FinalizedNativeXmrUnavailableReasonV3, Hex32,
    MessageContext, Participant, PrepareCurrentProfileClockRequest,
    PrepareCurrentProfileClockResult, PrepareNativeXmrClaimAuthorizationV3Request,
    PrepareNativeXmrClaimAuthorizationV3Result, PrepareNativeXmrClaimV3Request,
    PrepareNativeXmrClaimV3Result, PrepareNativeXmrEscrowV3Request, PrepareNativeXmrEscrowV3Result,
    PrepareNativeXmrPunishV3Request, PrepareNativeXmrPunishV3Result,
    PrepareNativeXmrRefundV3Request, PrepareNativeXmrRefundV3Result, PreparedTransaction,
    PreparedWitnessedClaim, RequestId, RunId, RuntimeCompatibility, RuntimeDescriptor,
    SubmissionOutcome, SubmitNativeXmrClaimAuthorizationV3Request,
    SubmitNativeXmrClaimAuthorizationV3Result, SubmitTransactionResult, TransactionId,
    VerifyCurrentProfileClockRequest, VerifyCurrentProfileClockResult, XmrClaimPartialV3,
    XmrNativeEffectV3, XmrNativeEscrowTermsV3, XmrNativeEscrowTermsV3Input,
};
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest as _, Sha256};
use tower::ServiceBuilder;
use tower_http::validate_request::ValidateRequestHeaderLayer;

const CAPABILITY: &str = "xmr-v3-capability-000000000000000001";
const TEST_RUN: &str = "xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx";
const SLOW_DELAY: Duration = Duration::from_millis(300);
const CLIENT_TIMEOUT: Duration = Duration::from_millis(50);
const METHODS: [&str; 8] = [
    METHOD_PREPARE_NATIVE_XMR_CLAIM_V3,
    METHOD_COMPLETE_NATIVE_XMR_CLAIM_V3,
    METHOD_PREPARE_NATIVE_XMR_REFUND_V3,
    METHOD_COMPLETE_NATIVE_XMR_REFUND_V3,
    METHOD_PREPARE_NATIVE_XMR_PUNISH_V3,
    METHOD_PREPARE_NATIVE_XMR_ESCROW_V3,
    METHOD_PREPARE_NATIVE_XMR_CLAIM_AUTHORIZATION_V3,
    METHOD_CLASSIFY_FINALIZED_NATIVE_XMR_EFFECT_V3,
];

#[derive(Clone, Copy, Debug, Default)]
enum Behavior {
    #[default]
    Happy,
    Slow,
    OversizedResponse,
    WrongContext,
    WrongAuthorizationId,
    WrongTerms,
    WrongTarget,
    WrongEffect,
    TamperedClock,
}

#[derive(Clone, Debug)]
struct Fixture {
    behavior: Behavior,
    calls: Arc<Mutex<BTreeMap<&'static str, usize>>>,
}

impl Fixture {
    fn record(&self, method: &'static str) {
        *self
            .calls
            .lock()
            .expect("call recorder")
            .entry(method)
            .or_default() += 1;
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
    _handle: jsonrpsee::server::ServerHandle,
}

async fn spawn_sidecar(role: Participant, behavior: Behavior) -> MockSidecar {
    let fixture = Fixture {
        behavior,
        calls: Arc::default(),
    };
    let authorization = format!("Bearer {CAPABILITY}");
    let middleware = ServiceBuilder::new()
        .layer(
            ValidateRequestHeaderLayer::has_header_value("authorization", &authorization)
                .expect("authorization header"),
        )
        .layer(
            ValidateRequestHeaderLayer::has_header_value(RUN_ID_HEADER, TEST_RUN)
                .expect("run header"),
        )
        .layer(
            ValidateRequestHeaderLayer::has_header_value(
                SIDECAR_ROLE_HEADER,
                match role {
                    Participant::Maker => "maker",
                    Participant::Taker => "taker",
                },
            )
            .expect("role header"),
        );
    let server = jsonrpsee::server::ServerBuilder::default()
        .set_http_middleware(middleware)
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
        _handle: handle,
    }
}

#[allow(clippy::too_many_lines)]
fn register_methods(module: &mut RpcModule<Fixture>) {
    module
        .register_async_method(
            METHOD_PREPARE_NATIVE_XMR_CLAIM_V3,
            |params, fixture, _| async move {
                let request: PrepareNativeXmrClaimV3Request = params.one()?;
                fixture.record(METHOD_PREPARE_NATIVE_XMR_CLAIM_V3);
                if matches!(fixture.behavior, Behavior::Slow) {
                    tokio::time::sleep(SLOW_DELAY).await;
                }
                if matches!(fixture.behavior, Behavior::OversizedResponse) {
                    return Ok::<_, jsonrpsee::types::ErrorObjectOwned>(Value::String("x".repeat(
                        usize::try_from(lez_bridge_client::MAX_RPC_BODY_BYTES).unwrap() + 1,
                    )));
                }
                json_value(
                    PrepareNativeXmrClaimV3Result::new(
                        response_context(&request.context, fixture.behavior),
                        response_terms(&request.terms, fixture.behavior),
                        claim_transcript(),
                    )
                    .expect("valid claim response"),
                )
            },
        )
        .expect("register claim preparation");
    module
        .register_async_method(
            METHOD_COMPLETE_NATIVE_XMR_CLAIM_V3,
            |params, fixture, _| async move {
                let request: CompleteNativeXmrClaimV3Request = params.one()?;
                fixture.record(METHOD_COMPLETE_NATIVE_XMR_CLAIM_V3);
                json_value(CompleteNativeXmrClaimV3Result::new(
                    response_context(&request.context, fixture.behavior),
                    response_terms(&request.terms, fixture.behavior),
                    tx(31),
                ))
            },
        )
        .expect("register claim completion");
    module
        .register_async_method(
            METHOD_PREPARE_NATIVE_XMR_REFUND_V3,
            |params, fixture, _| async move {
                let request: PrepareNativeXmrRefundV3Request = params.one()?;
                fixture.record(METHOD_PREPARE_NATIVE_XMR_REFUND_V3);
                json_value(
                    PrepareNativeXmrRefundV3Result::new(
                        response_context(&request.context, fixture.behavior),
                        response_terms(&request.terms, fixture.behavior),
                        refund_transcript(),
                    )
                    .expect("valid refund response"),
                )
            },
        )
        .expect("register refund preparation");
    module
        .register_async_method(
            METHOD_COMPLETE_NATIVE_XMR_REFUND_V3,
            |params, fixture, _| async move {
                let request: CompleteNativeXmrRefundV3Request = params.one()?;
                fixture.record(METHOD_COMPLETE_NATIVE_XMR_REFUND_V3);
                json_value(CompleteNativeXmrRefundV3Result::new(
                    response_context(&request.context, fixture.behavior),
                    response_terms(&request.terms, fixture.behavior),
                    tx(32),
                ))
            },
        )
        .expect("register refund completion");
    module
        .register_async_method(
            METHOD_PREPARE_NATIVE_XMR_PUNISH_V3,
            |params, fixture, _| async move {
                let request: PrepareNativeXmrPunishV3Request = params.one()?;
                fixture.record(METHOD_PREPARE_NATIVE_XMR_PUNISH_V3);
                json_value(PrepareNativeXmrPunishV3Result::new(
                    response_context(&request.context, fixture.behavior),
                    response_terms(&request.terms, fixture.behavior),
                    tx(33),
                ))
            },
        )
        .expect("register punishment preparation");
    module
        .register_async_method(
            METHOD_PREPARE_NATIVE_XMR_ESCROW_V3,
            |params, fixture, _| async move {
                let request: PrepareNativeXmrEscrowV3Request = params.one()?;
                fixture.record(METHOD_PREPARE_NATIVE_XMR_ESCROW_V3);
                json_value(PrepareNativeXmrEscrowV3Result::new(
                    response_context(&request.context, fixture.behavior),
                    response_terms(&request.terms, fixture.behavior),
                    tx(34),
                    tx(35),
                ))
            },
        )
        .expect("register escrow preparation");
    module
        .register_async_method(
            METHOD_PREPARE_NATIVE_XMR_CLAIM_AUTHORIZATION_V3,
            |params, fixture, _| async move {
                let request: PrepareNativeXmrClaimAuthorizationV3Request = params.one()?;
                fixture.record(METHOD_PREPARE_NATIVE_XMR_CLAIM_AUTHORIZATION_V3);
                json_value(PrepareNativeXmrClaimAuthorizationV3Result::new(
                    response_context(&request.context, fixture.behavior),
                    response_terms(&request.terms, fixture.behavior),
                    tx(36),
                ))
            },
        )
        .expect("register claim authorization");
    module
        .register_async_method(
            METHOD_SUBMIT_NATIVE_XMR_CLAIM_AUTHORIZATION_V3,
            |params, fixture, _| async move {
                let request: SubmitNativeXmrClaimAuthorizationV3Request = params.one()?;
                fixture.record(METHOD_SUBMIT_NATIVE_XMR_CLAIM_AUTHORIZATION_V3);
                let authorization_transaction_id =
                    if matches!(fixture.behavior, Behavior::WrongAuthorizationId) {
                        TransactionId::from_bytes([99; 32])
                    } else {
                        request.authorization.transaction_id
                    };
                json_value(SubmitNativeXmrClaimAuthorizationV3Result::new(
                    response_context(&request.context, fixture.behavior),
                    response_terms(&request.terms, fixture.behavior),
                    authorization_transaction_id,
                    SubmissionOutcome::Accepted,
                ))
            },
        )
        .expect("register claim-authorization submission");
    module
        .register_async_method(
            METHOD_CLASSIFY_FINALIZED_NATIVE_XMR_EFFECT_V3,
            |params, fixture, _| async move {
                let request: ClassifyFinalizedNativeXmrEffectV3Request = params.one()?;
                fixture.record(METHOD_CLASSIFY_FINALIZED_NATIVE_XMR_EFFECT_V3);
                let target = if matches!(fixture.behavior, Behavior::WrongTarget) {
                    FinalizedNativeXmrTransactionTargetV3::DiscoverByTerms {}
                } else {
                    request.target
                };
                let effect = if matches!(fixture.behavior, Behavior::WrongEffect) {
                    XmrNativeEffectV3::Refund
                } else {
                    request.effect
                };
                json_value(
                    ClassifyFinalizedNativeXmrEffectV3Result::new(
                        response_context(&request.context, fixture.behavior),
                        response_terms(&request.terms, fixture.behavior),
                        effect,
                        target,
                        FinalizedNativeXmrScanOutcomeV3::unavailable(
                            FinalizedNativeXmrUnavailableReasonV3::HistoryUnavailable,
                        ),
                    )
                    .expect("valid classifier response"),
                )
            },
        )
        .expect("register finalized classifier");
    module
        .register_async_method(
            METHOD_PREPARE_CURRENT_PROFILE_CLOCK,
            |params, fixture, _| async move {
                let request: PrepareCurrentProfileClockRequest = params.one()?;
                fixture.record(METHOD_PREPARE_CURRENT_PROFILE_CLOCK);
                json_value(PrepareCurrentProfileClockResult {
                    context: request.context,
                    runtime: request.runtime,
                    terms: request.terms,
                    recipient_account_id: request.recipient_account_id,
                    exclusive_punish_at_ms: request.exclusive_punish_at_ms,
                    transaction: tx(44),
                    clock_before: ChainClock::new(h(45), 10, 1_000),
                    sender_before: clock_account(h(7), 10, 2, h(46)),
                    recipient_before: clock_account(h(8), 5, 1, h(47)),
                    metadata_account_sha256_before: h(48),
                    custody_account_sha256_before: h(49),
                })
            },
        )
        .expect("register clock preparation");
    module
        .register_async_method(
            METHOD_VERIFY_CURRENT_PROFILE_CLOCK,
            |params, fixture, _| async move {
                let request: VerifyCurrentProfileClockRequest = params.one()?;
                fixture.record(METHOD_VERIFY_CURRENT_PROFILE_CLOCK);
                let preparation = request.preparation;
                let recipient_balance = if matches!(fixture.behavior, Behavior::TamperedClock) {
                    preparation.recipient_before.balance
                } else {
                    preparation.recipient_before.balance + 1
                };
                let sender_after = CurrentProfileClockAccountSnapshot::new(
                    preparation.sender_before.account_id,
                    preparation.sender_before.balance - 1,
                    preparation.sender_before.nonce + 1,
                    preparation.sender_before.program_owner,
                    h(50),
                );
                let recipient_after = CurrentProfileClockAccountSnapshot::new(
                    preparation.recipient_before.account_id,
                    recipient_balance,
                    preparation.recipient_before.nonce,
                    preparation.recipient_before.program_owner,
                    h(51),
                );
                json_value(VerifyCurrentProfileClockResult {
                    context: request.context,
                    runtime: request.runtime,
                    terms: preparation.terms,
                    recipient_account_id: preparation.recipient_account_id,
                    exclusive_punish_at_ms: preparation.exclusive_punish_at_ms,
                    transaction_id: preparation.transaction.transaction_id,
                    submission_request_id: request.submission.context.request_id,
                    submission_outcome: request.submission.outcome,
                    node_submission_attempts: u8::from(
                        request.submission.outcome == SubmissionOutcome::Accepted,
                    ),
                    transfer_amount: 1,
                    clock_before: preparation.clock_before,
                    clock_after: ChainClock::new(h(52), 11, 2_000),
                    sender_before: preparation.sender_before,
                    sender_after,
                    recipient_before: preparation.recipient_before,
                    recipient_after,
                    metadata_account_sha256_before: preparation.metadata_account_sha256_before,
                    metadata_account_sha256_after: preparation.metadata_account_sha256_before,
                    custody_account_sha256_before: preparation.custody_account_sha256_before,
                    custody_account_sha256_after: preparation.custody_account_sha256_before,
                    escrow_accounts_byte_identical: true,
                    accounting_verified: true,
                    local_only: true,
                    retry_policy: "one_node_submission_attempt_no_retry_poll_only".to_owned(),
                })
            },
        )
        .expect("register clock verification");
}
fn json_value(value: impl Serialize) -> Result<Value, jsonrpsee::types::ErrorObjectOwned> {
    serde_json::to_value(value).map_err(|error| {
        jsonrpsee::types::ErrorObjectOwned::owned(-32_000, error.to_string(), None::<Value>)
    })
}

#[allow(clippy::too_many_lines)]
#[tokio::test]
async fn clock_preparation_rejects_wrong_role_before_transport() {
    let sidecar = spawn_sidecar(Participant::Maker, Behavior::Happy).await;
    let runtime = runtime(Participant::Maker);
    let client = client(&sidecar.endpoint, runtime.clone(), Duration::from_secs(1));
    let terms = terms(40);
    let input = terms.to_input();
    let error = client
        .prepare_current_profile_clock(PrepareCurrentProfileClockRequest::new(
            context(&run_id(), Participant::Maker, "clock-wrong-role"),
            runtime,
            terms,
            input.claimant_account_id,
            input.punish_at_ms,
        ))
        .await
        .expect_err("Maker clock preparation must fail before transport");

    assert!(matches!(
        error,
        BridgeClientError::MalformedObservation {
            operation: BridgeOperation::PrepareCurrentProfileClock
        }
    ));
    assert_eq!(
        sidecar.fixture.calls(METHOD_PREPARE_CURRENT_PROFILE_CLOCK),
        0
    );
}

#[tokio::test]
async fn clock_verification_rejects_tampered_delta_after_one_call() {
    let sidecar = spawn_sidecar(Participant::Taker, Behavior::TamperedClock).await;
    let runtime = runtime(Participant::Taker);
    let client = client(&sidecar.endpoint, runtime.clone(), Duration::from_secs(1));
    let terms = terms(40);
    let input = terms.to_input();
    let preparation = client
        .prepare_current_profile_clock(PrepareCurrentProfileClockRequest::new(
            context(&run_id(), Participant::Taker, "clock-prepare"),
            runtime.clone(),
            terms,
            input.claimant_account_id,
            input.punish_at_ms,
        ))
        .await
        .expect("valid clock preparation");
    let transaction_id = preparation.transaction.transaction_id;
    let submission = SubmitTransactionResult::new(
        MessageContext::new(
            run_id(),
            transaction_id.submission_request_id(),
            Participant::Taker,
        ),
        transaction_id,
        SubmissionOutcome::Accepted,
    );
    let error = client
        .verify_current_profile_clock(VerifyCurrentProfileClockRequest {
            context: context(&run_id(), Participant::Taker, "clock-verify"),
            runtime,
            preparation,
            submission,
        })
        .await
        .expect_err("tampered recipient delta must fail closed");

    assert!(matches!(
        error,
        BridgeClientError::MalformedObservation {
            operation: BridgeOperation::VerifyCurrentProfileClock
        }
    ));
    assert_eq!(
        sidecar.fixture.calls(METHOD_PREPARE_CURRENT_PROFILE_CLOCK),
        1
    );
    assert_eq!(
        sidecar.fixture.calls(METHOD_VERIFY_CURRENT_PROFILE_CLOCK),
        1
    );
}

#[tokio::test]
async fn xmr_v3_methods_route_once_with_exact_run_and_role_headers() {
    let maker = spawn_sidecar(Participant::Maker, Behavior::Happy).await;
    let taker = spawn_sidecar(Participant::Taker, Behavior::Happy).await;
    let run = run_id();
    let maker_client = client(
        &maker.endpoint,
        runtime(Participant::Maker),
        Duration::from_secs(1),
    );
    let taker_client = client(
        &taker.endpoint,
        runtime(Participant::Taker),
        Duration::from_secs(1),
    );
    let terms = terms(42);

    let _ = maker_client
        .prepare_native_xmr_claim_v3(PrepareNativeXmrClaimV3Request::new(
            context(&run, Participant::Maker, "prepare-claim"),
            runtime(Participant::Maker),
            terms,
        ))
        .await
        .expect("prepare claim");
    let _ = maker_client
        .complete_native_xmr_claim_v3(
            CompleteNativeXmrClaimV3Request::new(
                context(&run, Participant::Maker, "complete-claim"),
                runtime(Participant::Maker),
                terms,
                claim_transcript(),
                AggregateBip340Signature::from_bytes([41; 64]),
            )
            .expect("claim request"),
        )
        .await
        .expect("complete claim");
    let _ = taker_client
        .prepare_native_xmr_refund_v3(PrepareNativeXmrRefundV3Request::new(
            context(&run, Participant::Taker, "prepare-refund"),
            runtime(Participant::Taker),
            terms,
        ))
        .await
        .expect("prepare refund");
    let _ = taker_client
        .complete_native_xmr_refund_v3(
            CompleteNativeXmrRefundV3Request::new(
                context(&run, Participant::Taker, "complete-refund"),
                runtime(Participant::Taker),
                terms,
                refund_transcript(),
                AggregateBip340Signature::from_bytes([42; 64]),
            )
            .expect("refund request"),
        )
        .await
        .expect("complete refund");
    let _ = maker_client
        .prepare_native_xmr_punish_v3(PrepareNativeXmrPunishV3Request::new(
            context(&run, Participant::Maker, "punish"),
            runtime(Participant::Maker),
            terms,
        ))
        .await
        .expect("prepare punishment");
    let _ = taker_client
        .prepare_native_xmr_escrow_v3(PrepareNativeXmrEscrowV3Request::new(
            context(&run, Participant::Taker, "escrow"),
            runtime(Participant::Taker),
            terms,
        ))
        .await
        .expect("prepare escrow");
    let _ = taker_client
        .prepare_native_xmr_claim_authorization_v3(
            PrepareNativeXmrClaimAuthorizationV3Request::new(
                context(&run, Participant::Taker, "authorization"),
                runtime(Participant::Taker),
                terms,
                XmrClaimPartialV3::new([77; 32]).expect("claim partial"),
            ),
        )
        .await
        .expect("prepare authorization");
    let _ = maker_client
        .classify_finalized_native_xmr_effect_v3(ClassifyFinalizedNativeXmrEffectV3Request::new(
            context(&run, Participant::Maker, "classify"),
            runtime(Participant::Maker),
            terms,
            XmrNativeEffectV3::Claim,
            FinalizedNativeXmrTransactionTargetV3::exact(tx(80)),
            window(),
        ))
        .await
        .expect("classify effect");

    for method in METHODS {
        assert_eq!(
            maker.fixture.calls(method) + taker.fixture.calls(method),
            1,
            "{method}"
        );
    }
}

#[allow(clippy::too_many_lines)]
#[tokio::test]
async fn xmr_v3_locally_invalid_context_runtime_and_roles_make_zero_calls() {
    let maker = spawn_sidecar(Participant::Maker, Behavior::Happy).await;
    let taker = spawn_sidecar(Participant::Taker, Behavior::Happy).await;
    let run = run_id();
    let maker_client = client(
        &maker.endpoint,
        runtime(Participant::Maker),
        Duration::from_secs(1),
    );
    let taker_client = client(
        &taker.endpoint,
        runtime(Participant::Taker),
        Duration::from_secs(1),
    );
    let terms = terms(42);
    let wrong_run = RunId::new("another-xmr-run").expect("wrong run");

    let results = [
        maker_client
            .prepare_native_xmr_claim_v3(PrepareNativeXmrClaimV3Request::new(
                context(&wrong_run, Participant::Maker, "bad-claim"),
                runtime(Participant::Maker),
                terms,
            ))
            .await
            .map(|_| ()),
        maker_client
            .complete_native_xmr_claim_v3(
                CompleteNativeXmrClaimV3Request::new(
                    context(&wrong_run, Participant::Maker, "bad-complete-claim"),
                    runtime(Participant::Maker),
                    terms,
                    claim_transcript(),
                    AggregateBip340Signature::from_bytes([41; 64]),
                )
                .expect("claim request"),
            )
            .await
            .map(|_| ()),
        taker_client
            .prepare_native_xmr_refund_v3(PrepareNativeXmrRefundV3Request::new(
                context(&wrong_run, Participant::Taker, "bad-refund"),
                runtime(Participant::Taker),
                terms,
            ))
            .await
            .map(|_| ()),
        taker_client
            .complete_native_xmr_refund_v3(
                CompleteNativeXmrRefundV3Request::new(
                    context(&wrong_run, Participant::Taker, "bad-complete-refund"),
                    runtime(Participant::Taker),
                    terms,
                    refund_transcript(),
                    AggregateBip340Signature::from_bytes([42; 64]),
                )
                .expect("refund request"),
            )
            .await
            .map(|_| ()),
        maker_client
            .prepare_native_xmr_punish_v3(PrepareNativeXmrPunishV3Request::new(
                context(&wrong_run, Participant::Maker, "bad-punish"),
                runtime(Participant::Maker),
                terms,
            ))
            .await
            .map(|_| ()),
        taker_client
            .prepare_native_xmr_escrow_v3(PrepareNativeXmrEscrowV3Request::new(
                context(&wrong_run, Participant::Taker, "bad-escrow"),
                runtime(Participant::Taker),
                terms,
            ))
            .await
            .map(|_| ()),
        taker_client
            .prepare_native_xmr_claim_authorization_v3(
                PrepareNativeXmrClaimAuthorizationV3Request::new(
                    context(&wrong_run, Participant::Taker, "bad-authorization"),
                    runtime(Participant::Taker),
                    terms,
                    XmrClaimPartialV3::new([77; 32]).expect("claim partial"),
                ),
            )
            .await
            .map(|_| ()),
        maker_client
            .classify_finalized_native_xmr_effect_v3(
                ClassifyFinalizedNativeXmrEffectV3Request::new(
                    context(&run, Participant::Maker, "bad-classifier-runtime"),
                    runtime(Participant::Taker),
                    terms,
                    XmrNativeEffectV3::Claim,
                    FinalizedNativeXmrTransactionTargetV3::exact(tx(81)),
                    window(),
                ),
            )
            .await
            .map(|_| ()),
    ];
    assert!(results.into_iter().all(|result| matches!(
        result,
        Err(BridgeClientError::RequestContextMismatch { .. })
    )));
    for method in METHODS {
        assert_eq!(
            maker.fixture.calls(method) + taker.fixture.calls(method),
            0,
            "{method}"
        );
    }

    let wrong_role = maker_client
        .prepare_native_xmr_escrow_v3(PrepareNativeXmrEscrowV3Request::new(
            context(&run, Participant::Maker, "wrong-role"),
            runtime(Participant::Maker),
            terms,
        ))
        .await;
    assert!(matches!(
        wrong_role,
        Err(BridgeClientError::MalformedObservation {
            operation: BridgeOperation::PrepareNativeXmrEscrowV3
        })
    ));
    assert_eq!(maker.fixture.calls(METHOD_PREPARE_NATIVE_XMR_ESCROW_V3), 0);
}

#[tokio::test]
async fn xmr_v3_response_echo_drift_is_rejected_after_one_call() {
    let cases = [
        (
            Behavior::WrongContext,
            BridgeOperation::PrepareNativeXmrPunishV3,
        ),
        (
            Behavior::WrongTerms,
            BridgeOperation::PrepareNativeXmrPunishV3,
        ),
        (
            Behavior::WrongTarget,
            BridgeOperation::ClassifyFinalizedNativeXmrEffectV3,
        ),
        (
            Behavior::WrongEffect,
            BridgeOperation::ClassifyFinalizedNativeXmrEffectV3,
        ),
    ];
    for (index, (behavior, operation)) in cases.into_iter().enumerate() {
        let sidecar = spawn_sidecar(Participant::Maker, behavior).await;
        let run = run_id();
        let client = client(
            &sidecar.endpoint,
            runtime(Participant::Maker),
            Duration::from_secs(1),
        );
        let result = if matches!(operation, BridgeOperation::PrepareNativeXmrPunishV3) {
            client
                .prepare_native_xmr_punish_v3(PrepareNativeXmrPunishV3Request::new(
                    context(&run, Participant::Maker, &format!("drift-{index}")),
                    runtime(Participant::Maker),
                    terms(42),
                ))
                .await
                .map(|_| ())
        } else {
            client
                .classify_finalized_native_xmr_effect_v3(
                    ClassifyFinalizedNativeXmrEffectV3Request::new(
                        context(&run, Participant::Maker, &format!("drift-{index}")),
                        runtime(Participant::Maker),
                        terms(42),
                        XmrNativeEffectV3::Claim,
                        FinalizedNativeXmrTransactionTargetV3::exact(tx(82)),
                        window(),
                    ),
                )
                .await
                .map(|_| ())
        };
        assert!(result.is_err(), "{behavior:?}");
        let method = match operation {
            BridgeOperation::PrepareNativeXmrPunishV3 => METHOD_PREPARE_NATIVE_XMR_PUNISH_V3,
            BridgeOperation::ClassifyFinalizedNativeXmrEffectV3 => {
                METHOD_CLASSIFY_FINALIZED_NATIVE_XMR_EFFECT_V3
            }
            _ => unreachable!(),
        };
        assert_eq!(sidecar.fixture.calls(method), 1);
    }
}

#[tokio::test]
async fn xmr_v3_timeout_and_oversized_body_are_not_retried() {
    for (behavior, expected_timeout) in
        [(Behavior::Slow, true), (Behavior::OversizedResponse, false)]
    {
        let sidecar = spawn_sidecar(Participant::Maker, behavior).await;
        let run = run_id();
        let timeout = if expected_timeout {
            CLIENT_TIMEOUT
        } else {
            Duration::from_secs(2)
        };
        let result = client(&sidecar.endpoint, runtime(Participant::Maker), timeout)
            .prepare_native_xmr_claim_v3(PrepareNativeXmrClaimV3Request::new(
                context(
                    &run,
                    Participant::Maker,
                    if expected_timeout {
                        "timeout"
                    } else {
                        "oversized"
                    },
                ),
                runtime(Participant::Maker),
                terms(42),
            ))
            .await;
        if expected_timeout {
            assert!(matches!(
                result,
                Err(BridgeClientError::Timeout {
                    operation: BridgeOperation::PrepareNativeXmrClaimV3
                })
            ));
        } else {
            assert!(matches!(
                result,
                Err(BridgeClientError::Transport {
                    operation: BridgeOperation::PrepareNativeXmrClaimV3,
                } | BridgeClientError::InvalidResponse {
                    operation: BridgeOperation::PrepareNativeXmrClaimV3,
                })
            ));
        }
        assert_eq!(sidecar.fixture.calls(METHOD_PREPARE_NATIVE_XMR_CLAIM_V3), 1);
    }
}
#[tokio::test]
async fn xmr_release_client_submits_one_exact_authorization() {
    let sidecar = spawn_sidecar(Participant::Taker, Behavior::Happy).await;
    let client = release_client(
        &sidecar.endpoint,
        runtime(Participant::Taker),
        Duration::from_secs(1),
    )
    .expect("release client");
    let authorization = tx(36);
    let expected_id = authorization.transaction_id;
    let result = client
        .submit_native_xmr_claim_authorization_v3(SubmitNativeXmrClaimAuthorizationV3Request::new(
            context(&run_id(), Participant::Taker, "release-submit"),
            runtime(Participant::Taker),
            terms(42),
            authorization,
        ))
        .await
        .expect("submit exact authorization");

    assert_eq!(result.authorization_transaction_id, expected_id);
    assert_eq!(result.outcome, SubmissionOutcome::Accepted);
    assert_eq!(
        sidecar
            .fixture
            .calls(METHOD_SUBMIT_NATIVE_XMR_CLAIM_AUTHORIZATION_V3),
        1
    );
}

#[tokio::test]
async fn xmr_release_client_rejects_wrong_id_after_one_call_and_roles_before_transport() {
    let wrong_id_sidecar = spawn_sidecar(Participant::Taker, Behavior::WrongAuthorizationId).await;
    let client = release_client(
        &wrong_id_sidecar.endpoint,
        runtime(Participant::Taker),
        Duration::from_secs(1),
    )
    .expect("release client");
    let result = client
        .submit_native_xmr_claim_authorization_v3(SubmitNativeXmrClaimAuthorizationV3Request::new(
            context(&run_id(), Participant::Taker, "wrong-id"),
            runtime(Participant::Taker),
            terms(42),
            tx(36),
        ))
        .await;
    assert!(matches!(
        result,
        Err(BridgeClientError::SubmitTransactionIdMismatch)
    ));
    assert_eq!(
        wrong_id_sidecar
            .fixture
            .calls(METHOD_SUBMIT_NATIVE_XMR_CLAIM_AUTHORIZATION_V3),
        1
    );

    let role_sidecar = spawn_sidecar(Participant::Taker, Behavior::Happy).await;
    let client = release_client(
        &role_sidecar.endpoint,
        runtime(Participant::Taker),
        Duration::from_secs(1),
    )
    .expect("release client");
    let result = client
        .submit_native_xmr_claim_authorization_v3(SubmitNativeXmrClaimAuthorizationV3Request::new(
            context(&run_id(), Participant::Maker, "wrong-role"),
            runtime(Participant::Maker),
            terms(42),
            tx(36),
        ))
        .await;
    assert!(matches!(
        result,
        Err(BridgeClientError::RequestContextMismatch {
            operation: BridgeOperation::SubmitNativeXmrClaimAuthorizationV3
        })
    ));
    assert_eq!(
        role_sidecar
            .fixture
            .calls(METHOD_SUBMIT_NATIVE_XMR_CLAIM_AUTHORIZATION_V3),
        0
    );

    let result = release_client(
        &role_sidecar.endpoint,
        runtime(Participant::Maker),
        Duration::from_secs(1),
    );
    assert!(matches!(
        result,
        Err(BridgeClientError::InvalidConfiguration {
            reason: ConfigurationError::ReleaseClientRequiresTaker
        })
    ));
    assert_eq!(
        role_sidecar
            .fixture
            .calls(METHOD_SUBMIT_NATIVE_XMR_CLAIM_AUTHORIZATION_V3),
        0
    );
}

fn response_context(context: &MessageContext, behavior: Behavior) -> MessageContext {
    if matches!(behavior, Behavior::WrongContext) {
        MessageContext::new(
            context.run_id.clone(),
            context.request_id.clone(),
            match context.sidecar_role {
                Participant::Maker => Participant::Taker,
                Participant::Taker => Participant::Maker,
            },
        )
    } else {
        context.clone()
    }
}

fn response_terms(terms: &XmrNativeEscrowTermsV3, behavior: Behavior) -> XmrNativeEscrowTermsV3 {
    if matches!(behavior, Behavior::WrongTerms) {
        terms_with_hashes(43, claim_message_hash(), refund_message_hash())
    } else {
        *terms
    }
}

fn run_id() -> RunId {
    RunId::new(TEST_RUN).expect("run id")
}

fn context(run: &RunId, role: Participant, suffix: &str) -> MessageContext {
    MessageContext::new(
        run.clone(),
        RequestId::new(format!("xmr-v3-{suffix}")).expect("request id"),
        role,
    )
}

fn client(endpoint: &str, runtime: RuntimeDescriptor, timeout: Duration) -> BridgeClient {
    BridgeClient::connect(BridgeClientConfig::new(
        endpoint,
        SidecarCapability::new(CAPABILITY).expect("capability"),
        run_id(),
        runtime,
        timeout,
    ))
    .expect("client configuration")
}
fn release_client(
    endpoint: &str,
    runtime: RuntimeDescriptor,
    timeout: Duration,
) -> Result<XmrReleaseClient, BridgeClientError> {
    XmrReleaseClient::connect(BridgeClientConfig::new(
        endpoint,
        SidecarCapability::new(CAPABILITY).expect("capability"),
        run_id(),
        runtime,
        timeout,
    ))
}

fn runtime(role: Participant) -> RuntimeDescriptor {
    RuntimeDescriptor::new(
        role,
        RuntimeCompatibility::LeeV0_2_0,
        h(40),
        h(41),
        h(42),
        h(3),
        match role {
            Participant::Maker => h(8),
            Participant::Taker => h(7),
        },
    )
}

fn terms(amount: u128) -> XmrNativeEscrowTermsV3 {
    terms_with_hashes(amount, claim_message_hash(), refund_message_hash())
}

fn terms_with_hashes(
    amount: u128,
    claim_message_hash: Hex32,
    refund_message_hash: Hex32,
) -> XmrNativeEscrowTermsV3 {
    XmrNativeEscrowTermsV3::new(XmrNativeEscrowTermsV3Input {
        swap_id: h(1),
        activation_commitment: h(2),
        escrow_program_id: h(3),
        authenticated_transfer_program_id: h(4),
        metadata_account_id: h(5),
        custody_account_id: h(6),
        depositor: Participant::Taker,
        depositor_account_id: h(7),
        claimant: Participant::Maker,
        claimant_account_id: h(8),
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
        claim_message_hash,
        refund_message_hash,
        punish_message_hash: h(19),
    })
    .expect("valid XMR terms")
}

fn claim_transcript() -> PreparedWitnessedClaim {
    transcript("claim", vec![0xc1; 128])
}

fn refund_transcript() -> PreparedWitnessedClaim {
    transcript("refund", vec![0xd1; 128])
}

fn transcript(suffix: &str, bytes: Vec<u8>) -> PreparedWitnessedClaim {
    PreparedWitnessedClaim::new(
        RequestId::new(format!("xmr-v3-{suffix}-transcript")).expect("request id"),
        official_message_hash(&bytes),
        ExactMessageBytes::new(bytes).expect("message bytes"),
    )
}

fn claim_message_hash() -> Hex32 {
    official_message_hash(&[0xc1; 128])
}

fn refund_message_hash() -> Hex32 {
    official_message_hash(&[0xd1; 128])
}

fn official_message_hash(bytes: &[u8]) -> Hex32 {
    let mut hasher = Sha256::new();
    hasher.update(b"/LEE/v0.3/Message/Public/\x00\x00\x00\x00\x00\x00\x00");
    hasher.update(bytes);
    Hex32::from_bytes(hasher.finalize().into())
}

fn tx(byte: u8) -> PreparedTransaction {
    PreparedTransaction::new(
        TransactionId::from_bytes([byte; 32]),
        ExactTransactionBytes::new(vec![byte; 128]).expect("transaction bytes"),
    )
}

fn window() -> DiscoveryWindow {
    DiscoveryWindow::new(90, 21).expect("window")
}

const fn h(byte: u8) -> Hex32 {
    Hex32::from_bytes([byte; 32])
}

fn clock_account(
    account_id: Hex32,
    balance: u128,
    nonce: u128,
    account_sha256: Hex32,
) -> CurrentProfileClockAccountSnapshot {
    CurrentProfileClockAccountSnapshot::new(account_id, balance, nonce, h(4), account_sha256)
}
