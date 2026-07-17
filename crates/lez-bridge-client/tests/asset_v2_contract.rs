use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::Duration,
};

use jsonrpsee::{RpcModule, types::ErrorObjectOwned};
use lez_bridge_client::{
    BridgeClient, BridgeClientConfig, BridgeClientError, BridgeOperation,
    METHOD_CLASSIFY_FINALIZED_WITNESSED_ASSET_CLAIM_V2,
    METHOD_CLASSIFY_FINALIZED_WITNESSED_ASSET_CUSTODY_CREATION_V2,
    METHOD_CLASSIFY_FINALIZED_WITNESSED_ASSET_FUNDING_V2,
    METHOD_CLASSIFY_FINALIZED_WITNESSED_ASSET_INITIALIZATION_V2,
    METHOD_COMPLETE_WITNESSED_ASSET_CLAIM_V2, METHOD_OBSERVE_FINALIZED_WITNESSED_ASSET_CLAIM_V2,
    METHOD_OBSERVE_WITNESSED_ASSET_ESCROW_V2, METHOD_OBSERVE_WITNESSED_ASSET_REFUND_V2,
    METHOD_PREPARE_WITNESSED_ASSET_CLAIM_V2, METHOD_PREPARE_WITNESSED_ASSET_ESCROW_V2,
    METHOD_PREPARE_WITNESSED_ASSET_REFUND_V2, RUN_ID_HEADER, SIDECAR_ROLE_HEADER,
    SidecarCapability,
};
use lez_bridge_protocol::{
    AccountIds, AggregateBip340Signature, ChainClock, ChainPosition, ChainTip,
    ClassifyFinalizedWitnessedAssetClaimV2Request, ClassifyFinalizedWitnessedAssetClaimV2Result,
    ClassifyFinalizedWitnessedAssetCustodyCreationV2Request,
    ClassifyFinalizedWitnessedAssetCustodyCreationV2Result,
    ClassifyFinalizedWitnessedAssetFundingV2Request,
    ClassifyFinalizedWitnessedAssetFundingV2Result,
    ClassifyFinalizedWitnessedAssetInitializationV2Request,
    ClassifyFinalizedWitnessedAssetInitializationV2Result, CompleteWitnessedAssetClaimV2Request,
    CompleteWitnessedAssetClaimV2Result, DiscoveryWindow, EscrowState, ExactMessageBytes,
    ExactTransactionBytes, FinalizedBlockIdentity, FinalizedWitnessedAssetClaimFactsV2,
    FinalizedWitnessedAssetInitializationFactsV2, FinalizedWitnessedAssetScanOutcomeV2,
    FinalizedWitnessedAssetTransactionTargetV2, FinalizedWitnessedAssetUnavailableReasonV2, Hex32,
    MessageContext, NativeCustodyFacts, NativeRefundObservationTarget,
    ObserveFinalizedWitnessedAssetClaimV2Request, ObserveFinalizedWitnessedAssetClaimV2Result,
    ObserveWitnessedAssetEscrowV2Request, ObserveWitnessedAssetEscrowV2Result,
    ObserveWitnessedAssetRefundV2Request, ObserveWitnessedAssetRefundV2Result,
    ObservedTransactionFacts, Participant, PrepareWitnessedAssetClaimV2Request,
    PrepareWitnessedAssetClaimV2Result, PrepareWitnessedAssetEscrowV2Request,
    PrepareWitnessedAssetEscrowV2Result, PrepareWitnessedAssetRefundV2Request,
    PrepareWitnessedAssetRefundV2Result, PreparedTransaction, PreparedWitnessedClaim, RequestId,
    RunId, RuntimeCompatibility, RuntimeDescriptor, TokenHoldingFactsV2, TransactionId,
    WitnessedAssetClaimInstructionFactsV2, WitnessedAssetCustodyFactsV2,
    WitnessedAssetEffectInstructionFactsV2, WitnessedAssetInitializationCustodyFactsV2,
    WitnessedAssetObservedPrepareEffectV2, WitnessedAssetPrepareStepV2,
    WitnessedAssetPreparedEffectV2, WitnessedAssetRefundFoundFactsV2,
    WitnessedAssetRefundInstructionFactsV2, WitnessedAssetRefundObservationV2,
    WitnessedEscrowMetadataFacts, WitnessedLezAssetTermsV2, WitnessedLezAssetV2,
    WitnessedNativeEscrowTerms, WitnessedNativeEscrowTermsInput, WitnessedTokenEscrowTermsV2,
    WitnessedTokenEscrowTermsV2Input,
};
use serde_json::json;
use sha2::{Digest as _, Sha256};
use tower::ServiceBuilder;
use tower_http::validate_request::ValidateRequestHeaderLayer;

const CAPABILITY: &str = "asset-v2-capability-0000000000000001";
const TEST_RUN: &str = "vvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvv";
const SLOW_DELAY: Duration = Duration::from_millis(300);
const CLIENT_TIMEOUT: Duration = Duration::from_millis(50);
const METHODS: [&str; 11] = [
    METHOD_PREPARE_WITNESSED_ASSET_ESCROW_V2,
    METHOD_OBSERVE_WITNESSED_ASSET_ESCROW_V2,
    METHOD_PREPARE_WITNESSED_ASSET_CLAIM_V2,
    METHOD_COMPLETE_WITNESSED_ASSET_CLAIM_V2,
    METHOD_OBSERVE_FINALIZED_WITNESSED_ASSET_CLAIM_V2,
    METHOD_PREPARE_WITNESSED_ASSET_REFUND_V2,
    METHOD_OBSERVE_WITNESSED_ASSET_REFUND_V2,
    METHOD_CLASSIFY_FINALIZED_WITNESSED_ASSET_INITIALIZATION_V2,
    METHOD_CLASSIFY_FINALIZED_WITNESSED_ASSET_CUSTODY_CREATION_V2,
    METHOD_CLASSIFY_FINALIZED_WITNESSED_ASSET_FUNDING_V2,
    METHOD_CLASSIFY_FINALIZED_WITNESSED_ASSET_CLAIM_V2,
];

#[derive(Clone, Copy, Debug, Default)]
enum Behavior {
    #[default]
    Happy,
    SlowPrepare,
    WrongContext,
    WrongTerms,
    WrongTarget,
    MovingEscrowTip,
    StateOnlyRefundMismatch,
    ExactRefundWrongId,
    NonPublicRefund,
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

// Keep the complete external v2 surface visibly registered in one mock sidecar.
#[allow(clippy::too_many_lines)]
fn register_methods(module: &mut RpcModule<Fixture>) {
    module
        .register_async_method(
            METHOD_PREPARE_WITNESSED_ASSET_ESCROW_V2,
            |params, fixture, _| async move {
                let request: PrepareWitnessedAssetEscrowV2Request = params.one()?;
                fixture.record(METHOD_PREPARE_WITNESSED_ASSET_ESCROW_V2);
                if matches!(fixture.behavior, Behavior::SlowPrepare) {
                    tokio::time::sleep(SLOW_DELAY).await;
                }
                let context = response_context(&request.context, fixture.behavior);
                let terms = response_terms(&request.terms, fixture.behavior);
                let result = PrepareWitnessedAssetEscrowV2Result::new(
                    context,
                    terms.clone(),
                    prepared_effects(&terms),
                )
                .expect("valid prepared effects");
                Ok::<_, ErrorObjectOwned>(result)
            },
        )
        .expect("prepare asset escrow method");
    module
        .register_method(
            METHOD_OBSERVE_WITNESSED_ASSET_ESCROW_V2,
            |params, fixture, _| {
                let request: ObserveWitnessedAssetEscrowV2Request = params.one()?;
                fixture.record(METHOD_OBSERVE_WITNESSED_ASSET_ESCROW_V2);
                let (metadata, custody) = asset_state(&request.terms, EscrowState::Funded, true);
                let effects = observed_prepare_effects(&request.terms, metadata.account_id);
                let mut value = serde_json::to_value(
                    ObserveWitnessedAssetEscrowV2Result::new(
                        request.context,
                        request.terms,
                        tip(92),
                        effects,
                        metadata,
                        custody,
                        tip(92),
                    )
                    .expect("valid asset observation"),
                )
                .expect("serializable observation");
                if matches!(fixture.behavior, Behavior::MovingEscrowTip) {
                    value["tip_after"]["block_hash"] = json!(h(199));
                }
                Ok::<_, ErrorObjectOwned>(value)
            },
        )
        .expect("observe asset escrow method");
    module
        .register_method(
            METHOD_PREPARE_WITNESSED_ASSET_CLAIM_V2,
            |params, fixture, _| {
                let request: PrepareWitnessedAssetClaimV2Request = params.one()?;
                fixture.record(METHOD_PREPARE_WITNESSED_ASSET_CLAIM_V2);
                Ok::<_, ErrorObjectOwned>(PrepareWitnessedAssetClaimV2Result::new(
                    request.context,
                    request.terms,
                    claim_transcript(),
                ))
            },
        )
        .expect("prepare asset claim method");
    module
        .register_method(
            METHOD_COMPLETE_WITNESSED_ASSET_CLAIM_V2,
            |params, fixture, _| {
                let request: CompleteWitnessedAssetClaimV2Request = params.one()?;
                fixture.record(METHOD_COMPLETE_WITNESSED_ASSET_CLAIM_V2);
                Ok::<_, ErrorObjectOwned>(CompleteWitnessedAssetClaimV2Result::new(
                    request.context,
                    request.terms,
                    tx(124),
                ))
            },
        )
        .expect("complete asset claim method");
    module
        .register_method(
            METHOD_OBSERVE_FINALIZED_WITNESSED_ASSET_CLAIM_V2,
            |params, fixture, _| {
                let request: ObserveFinalizedWitnessedAssetClaimV2Request = params.one()?;
                fixture.record(METHOD_OBSERVE_FINALIZED_WITNESSED_ASSET_CLAIM_V2);
                let facts = finalized_claim_facts(&request.terms, request.claim);
                Ok::<_, ErrorObjectOwned>(
                    ObserveFinalizedWitnessedAssetClaimV2Result::new(
                        request.context,
                        request.terms,
                        tip(92),
                        facts,
                    )
                    .expect("valid finalized claim"),
                )
            },
        )
        .expect("observe finalized asset claim method");
    module
        .register_method(
            METHOD_PREPARE_WITNESSED_ASSET_REFUND_V2,
            |params, fixture, _| {
                let request: PrepareWitnessedAssetRefundV2Request = params.one()?;
                fixture.record(METHOD_PREPARE_WITNESSED_ASSET_REFUND_V2);
                Ok::<_, ErrorObjectOwned>(PrepareWitnessedAssetRefundV2Result::new(
                    request.context,
                    request.terms,
                    tx(129),
                ))
            },
        )
        .expect("prepare asset refund method");
    module
        .register_method(
            METHOD_OBSERVE_WITNESSED_ASSET_REFUND_V2,
            |params, fixture, _| {
                let request: ObserveWitnessedAssetRefundV2Request = params.one()?;
                fixture.record(METHOD_OBSERVE_WITNESSED_ASSET_REFUND_V2);
                let found = matches!(
                    fixture.behavior,
                    Behavior::ExactRefundWrongId | Behavior::NonPublicRefund
                );
                let (metadata, custody) = asset_state(
                    &request.terms,
                    if found {
                        EscrowState::Refunded
                    } else {
                        EscrowState::Funded
                    },
                    !found,
                );
                let refund = match fixture.behavior {
                    Behavior::StateOnlyRefundMismatch => {
                        WitnessedAssetRefundObservationV2::UnknownOrPending
                    }
                    Behavior::ExactRefundWrongId => WitnessedAssetRefundObservationV2::found(
                        refund_facts(&request.terms, &metadata, 130),
                    ),
                    Behavior::NonPublicRefund => WitnessedAssetRefundObservationV2::found(
                        refund_facts(&request.terms, &metadata, 129),
                    ),
                    _ => WitnessedAssetRefundObservationV2::NotRequested,
                };
                let mut value = serde_json::to_value(
                    ObserveWitnessedAssetRefundV2Result::new(
                        request.context,
                        request.terms,
                        clock(92),
                        metadata,
                        custody,
                        refund,
                        clock(92),
                    )
                    .expect("valid refund state"),
                )
                .expect("serializable refund observation");
                if matches!(fixture.behavior, Behavior::NonPublicRefund) {
                    value["refund"]["facts"]["transaction"]["is_public"] = json!(false);
                }
                Ok::<_, ErrorObjectOwned>(value)
            },
        )
        .expect("observe asset refund method");
    register_classifier_methods(module);
}

fn register_classifier_methods(module: &mut RpcModule<Fixture>) {
    module
        .register_method(
            METHOD_CLASSIFY_FINALIZED_WITNESSED_ASSET_INITIALIZATION_V2,
            |params, fixture, _| {
                let request: ClassifyFinalizedWitnessedAssetInitializationV2Request =
                    params.one()?;
                fixture.record(METHOD_CLASSIFY_FINALIZED_WITNESSED_ASSET_INITIALIZATION_V2);
                let facts = finalized_native_initialization_facts();
                let mut value = serde_json::to_value(
                    ClassifyFinalizedWitnessedAssetInitializationV2Result::found(
                        request.context,
                        request.terms,
                        request.target,
                        finalized_clock(),
                        request.window,
                        facts,
                    )
                    .expect("valid found initialization"),
                )
                .expect("serializable result");
                if matches!(fixture.behavior, Behavior::WrongContext) {
                    value["context"]["sidecar_role"] = json!("taker");
                }
                Ok::<_, ErrorObjectOwned>(value)
            },
        )
        .expect("classify asset initialization method");
    module
        .register_method(
            METHOD_CLASSIFY_FINALIZED_WITNESSED_ASSET_CUSTODY_CREATION_V2,
            |params, fixture, _| {
                let request: ClassifyFinalizedWitnessedAssetCustodyCreationV2Request =
                    params.one()?;
                fixture.record(METHOD_CLASSIFY_FINALIZED_WITNESSED_ASSET_CUSTODY_CREATION_V2);
                Ok::<_, ErrorObjectOwned>(
                    ClassifyFinalizedWitnessedAssetCustodyCreationV2Result::absent(
                        request.context,
                        request.terms,
                        request.target,
                        finalized_clock(),
                        request.window,
                    )
                    .expect("valid absent custody creation"),
                )
            },
        )
        .expect("classify custody creation method");
    module
        .register_method(
            METHOD_CLASSIFY_FINALIZED_WITNESSED_ASSET_FUNDING_V2,
            |params, fixture, _| {
                let request: ClassifyFinalizedWitnessedAssetFundingV2Request = params.one()?;
                fixture.record(METHOD_CLASSIFY_FINALIZED_WITNESSED_ASSET_FUNDING_V2);
                let target = if matches!(fixture.behavior, Behavior::WrongTarget) {
                    FinalizedWitnessedAssetTransactionTargetV2::DiscoverByTerms {}
                } else {
                    request.target
                };
                Ok::<_, ErrorObjectOwned>(
                    ClassifyFinalizedWitnessedAssetFundingV2Result::uncertain(
                        request.context,
                        request.terms,
                        target,
                        finalized_clock(),
                        request.window,
                    )
                    .expect("valid uncertain funding"),
                )
            },
        )
        .expect("classify asset funding method");
    module
        .register_method(
            METHOD_CLASSIFY_FINALIZED_WITNESSED_ASSET_CLAIM_V2,
            |params, fixture, _| {
                let request: ClassifyFinalizedWitnessedAssetClaimV2Request = params.one()?;
                fixture.record(METHOD_CLASSIFY_FINALIZED_WITNESSED_ASSET_CLAIM_V2);
                Ok::<_, ErrorObjectOwned>(
                    ClassifyFinalizedWitnessedAssetClaimV2Result::unavailable(
                        request.context,
                        request.terms,
                        request.claim,
                        request.target,
                        FinalizedWitnessedAssetUnavailableReasonV2::HistoryUnavailable,
                    ),
                )
            },
        )
        .expect("classify asset claim method");
}

#[tokio::test]
// One contiguous flow makes the exactly-once assertion cover every v2 method together.
#[allow(clippy::too_many_lines)]
async fn all_v2_methods_are_single_call_and_preserve_asset_specific_wire_contracts() {
    let maker_sidecar = spawn_sidecar(Participant::Maker, Behavior::Happy).await;
    let taker_sidecar = spawn_sidecar(Participant::Taker, Behavior::Happy).await;
    let run = run_id();
    let maker_runtime = runtime(Participant::Maker);
    let taker_runtime = runtime(Participant::Taker);
    let maker_client = client(
        &maker_sidecar.endpoint,
        &run,
        maker_runtime.clone(),
        Duration::from_secs(1),
    );
    let taker_client = client(
        &taker_sidecar.endpoint,
        &run,
        taker_runtime.clone(),
        Duration::from_secs(1),
    );
    let native = native_asset();
    let token_82 = token_asset(h(82));
    let token_83 = token_asset(h(83));
    let window = window();

    let prepared = maker_client
        .prepare_witnessed_asset_escrow_v2(PrepareWitnessedAssetEscrowV2Request::new(
            context(&run, Participant::Maker, "prepare-escrow"),
            maker_runtime.clone(),
            token_82.clone(),
        ))
        .await
        .expect("token escrow prepares");
    assert_eq!(
        prepared
            .effects
            .iter()
            .map(|effect| effect.step)
            .collect::<Vec<_>>(),
        vec![
            WitnessedAssetPrepareStepV2::InitializeWitnessed,
            WitnessedAssetPrepareStepV2::CreateCustodyAta,
            WitnessedAssetPrepareStepV2::Fund,
        ]
    );
    let _ = maker_client
        .observe_witnessed_asset_escrow_v2(
            ObserveWitnessedAssetEscrowV2Request::new(
                context(&run, Participant::Maker, "observe-escrow"),
                maker_runtime.clone(),
                token_82.clone(),
                prepared.effects,
                window,
            )
            .expect("ordered token observation request"),
        )
        .await
        .expect("token escrow observes");
    let _ = taker_client
        .prepare_witnessed_asset_claim_v2(PrepareWitnessedAssetClaimV2Request::new(
            context(&run, Participant::Taker, "prepare-claim"),
            taker_runtime.clone(),
            token_83.clone(),
            TransactionId::from_bytes([107; 32]),
        ))
        .await
        .expect("token claim prepares");
    let claim = claim_transcript();
    let _ = taker_client
        .complete_witnessed_asset_claim_v2(CompleteWitnessedAssetClaimV2Request::new(
            context(&run, Participant::Taker, "complete-claim"),
            taker_runtime.clone(),
            native.clone(),
            claim.clone(),
            AggregateBip340Signature::from_bytes([123; 64]),
        ))
        .await
        .expect("native claim completes");
    let _ = taker_client
        .observe_finalized_witnessed_asset_claim_v2(
            ObserveFinalizedWitnessedAssetClaimV2Request::new(
                context(&run, Participant::Taker, "observe-claim"),
                taker_runtime.clone(),
                token_83.clone(),
                claim.clone(),
                TransactionId::from_bytes([124; 32]),
                window,
            ),
        )
        .await
        .expect("token claim observes");
    let _ = taker_client
        .prepare_witnessed_asset_refund_v2(PrepareWitnessedAssetRefundV2Request::new(
            context(&run, Participant::Taker, "prepare-refund"),
            taker_runtime.clone(),
            native.clone(),
        ))
        .await
        .expect("native refund prepares");
    let _ = maker_client
        .observe_witnessed_asset_refund_v2(ObserveWitnessedAssetRefundV2Request::new(
            context(&run, Participant::Maker, "observe-refund"),
            maker_runtime.clone(),
            token_82.clone(),
            NativeRefundObservationTarget::StateOnly,
        ))
        .await
        .expect("token refund state observes");

    let initialization = maker_client
        .classify_finalized_witnessed_asset_initialization_v2(
            ClassifyFinalizedWitnessedAssetInitializationV2Request::new(
                context(&run, Participant::Maker, "classify-init"),
                maker_runtime.clone(),
                native,
                tx(132),
                window,
            ),
        )
        .await
        .expect("exact native initialization classifies");
    assert!(matches!(
        initialization.outcome,
        FinalizedWitnessedAssetScanOutcomeV2::Found { .. }
    ));
    let custody = maker_client
        .classify_finalized_witnessed_asset_custody_creation_v2(
            ClassifyFinalizedWitnessedAssetCustodyCreationV2Request::discover_by_terms(
                context(&run, Participant::Maker, "classify-custody"),
                maker_runtime.clone(),
                token_82,
                window,
            )
            .expect("token custody discovery"),
        )
        .await
        .expect("custody absence classifies");
    assert!(matches!(
        custody.outcome,
        FinalizedWitnessedAssetScanOutcomeV2::Absent { .. }
    ));
    let funding = maker_client
        .classify_finalized_witnessed_asset_funding_v2(
            ClassifyFinalizedWitnessedAssetFundingV2Request::new(
                context(&run, Participant::Maker, "classify-funding"),
                maker_runtime.clone(),
                token_83.clone(),
                tx(107),
                window,
            ),
        )
        .await
        .expect("exact token funding classifies");
    assert!(matches!(
        funding.outcome,
        FinalizedWitnessedAssetScanOutcomeV2::Uncertain { .. }
    ));
    let claim_presence = maker_client
        .classify_finalized_witnessed_asset_claim_v2(
            ClassifyFinalizedWitnessedAssetClaimV2Request::discover_by_terms(
                context(&run, Participant::Maker, "classify-claim"),
                maker_runtime,
                token_83,
                claim,
                window,
            ),
        )
        .await
        .expect("token claim discovery classifies");
    assert!(matches!(
        claim_presence.outcome,
        FinalizedWitnessedAssetScanOutcomeV2::Unavailable { .. }
    ));

    for method in METHODS {
        assert_eq!(
            maker_sidecar.fixture.calls(method) + taker_sidecar.fixture.calls(method),
            1,
            "{method} must be called once"
        );
    }
}

#[tokio::test]
async fn v2_prepare_timeout_is_not_retried() {
    let sidecar = spawn_sidecar(Participant::Maker, Behavior::SlowPrepare).await;
    let run = run_id();
    let runtime = runtime(Participant::Maker);
    let result = client(&sidecar.endpoint, &run, runtime.clone(), CLIENT_TIMEOUT)
        .prepare_witnessed_asset_escrow_v2(PrepareWitnessedAssetEscrowV2Request::new(
            context(&run, Participant::Maker, "timeout"),
            runtime,
            token_asset(h(82)),
        ))
        .await;
    assert!(matches!(
        result,
        Err(BridgeClientError::Timeout {
            operation: BridgeOperation::PrepareWitnessedAssetEscrowV2
        })
    ));
    assert_eq!(
        sidecar
            .fixture
            .calls(METHOD_PREPARE_WITNESSED_ASSET_ESCROW_V2),
        1
    );
}

#[tokio::test]
async fn v2_wrong_context_terms_and_target_are_rejected_after_one_call() {
    let run = run_id();
    let runtime = runtime(Participant::Maker);
    for (behavior, expected) in [
        (
            Behavior::WrongContext,
            BridgeOperation::ClassifyFinalizedWitnessedAssetInitializationV2,
        ),
        (
            Behavior::WrongTerms,
            BridgeOperation::PrepareWitnessedAssetEscrowV2,
        ),
        (
            Behavior::WrongTarget,
            BridgeOperation::ClassifyFinalizedWitnessedAssetFundingV2,
        ),
    ] {
        let sidecar = spawn_sidecar(Participant::Maker, behavior).await;
        let client = client(
            &sidecar.endpoint,
            &run,
            runtime.clone(),
            Duration::from_secs(1),
        );
        let error = match behavior {
            Behavior::WrongContext => client
                .classify_finalized_witnessed_asset_initialization_v2(
                    ClassifyFinalizedWitnessedAssetInitializationV2Request::new(
                        context(&run, Participant::Maker, "wrong-context"),
                        runtime.clone(),
                        native_asset(),
                        tx(132),
                        window(),
                    ),
                )
                .await
                .unwrap_err(),
            Behavior::WrongTerms => client
                .prepare_witnessed_asset_escrow_v2(PrepareWitnessedAssetEscrowV2Request::new(
                    context(&run, Participant::Maker, "wrong-terms"),
                    runtime.clone(),
                    token_asset(h(82)),
                ))
                .await
                .unwrap_err(),
            Behavior::WrongTarget => client
                .classify_finalized_witnessed_asset_funding_v2(
                    ClassifyFinalizedWitnessedAssetFundingV2Request::new(
                        context(&run, Participant::Maker, "wrong-target"),
                        runtime.clone(),
                        token_asset(h(83)),
                        tx(107),
                        window(),
                    ),
                )
                .await
                .unwrap_err(),
            _ => unreachable!(),
        };
        assert!(
            matches!(
                error,
                BridgeClientError::ResponseContextMismatch { operation }
                    | BridgeClientError::MalformedObservation { operation }
                    if operation == expected
            ),
            "unexpected error: {error:?}"
        );
        assert_eq!(
            sidecar.fixture.calls(match behavior {
                Behavior::WrongContext => {
                    METHOD_CLASSIFY_FINALIZED_WITNESSED_ASSET_INITIALIZATION_V2
                }
                Behavior::WrongTerms => METHOD_PREPARE_WITNESSED_ASSET_ESCROW_V2,
                Behavior::WrongTarget => METHOD_CLASSIFY_FINALIZED_WITNESSED_ASSET_FUNDING_V2,
                _ => unreachable!(),
            }),
            1
        );
    }
}

#[tokio::test]
async fn v2_observations_reject_unstable_or_target_mismatched_evidence_once() {
    let run = run_id();
    let runtime = runtime(Participant::Maker);
    for behavior in [
        Behavior::MovingEscrowTip,
        Behavior::StateOnlyRefundMismatch,
        Behavior::ExactRefundWrongId,
        Behavior::NonPublicRefund,
    ] {
        let sidecar = spawn_sidecar(Participant::Maker, behavior).await;
        let client = client(
            &sidecar.endpoint,
            &run,
            runtime.clone(),
            Duration::from_secs(1),
        );
        let operation = match behavior {
            Behavior::MovingEscrowTip => {
                let request = ObserveWitnessedAssetEscrowV2Request::new(
                    context(&run, Participant::Maker, "moving-escrow-tip"),
                    runtime.clone(),
                    token_asset(h(82)),
                    prepared_effects(&token_asset(h(82))),
                    window(),
                )
                .expect("ordered observation request");
                assert!(matches!(
                    client.observe_witnessed_asset_escrow_v2(request).await,
                    Err(BridgeClientError::MalformedObservation {
                        operation: BridgeOperation::ObserveWitnessedAssetEscrowV2
                    })
                ));
                METHOD_OBSERVE_WITNESSED_ASSET_ESCROW_V2
            }
            Behavior::StateOnlyRefundMismatch
            | Behavior::ExactRefundWrongId
            | Behavior::NonPublicRefund => {
                let target = if matches!(behavior, Behavior::StateOnlyRefundMismatch) {
                    NativeRefundObservationTarget::StateOnly
                } else {
                    NativeRefundObservationTarget::Exact {
                        refund_transaction_id: TransactionId::from_bytes([129; 32]),
                        window: window(),
                    }
                };
                let request = ObserveWitnessedAssetRefundV2Request::new(
                    context(&run, Participant::Maker, "invalid-refund-evidence"),
                    runtime.clone(),
                    token_asset(h(82)),
                    target,
                );
                assert!(matches!(
                    client.observe_witnessed_asset_refund_v2(request).await,
                    Err(BridgeClientError::MalformedObservation {
                        operation: BridgeOperation::ObserveWitnessedAssetRefundV2
                    })
                ));
                METHOD_OBSERVE_WITNESSED_ASSET_REFUND_V2
            }
            _ => unreachable!(),
        };
        assert_eq!(sidecar.fixture.calls(operation), 1);
    }
}

#[tokio::test]
async fn v2_refund_rejects_an_outsider_signer_before_transport() {
    let sidecar = spawn_sidecar(Participant::Maker, Behavior::Happy).await;
    let run = run_id();
    let mut outsider_runtime = runtime(Participant::Maker);
    outsider_runtime.signer_account_id = h(200);
    let result = client(
        &sidecar.endpoint,
        &run,
        outsider_runtime.clone(),
        Duration::from_secs(1),
    )
    .prepare_witnessed_asset_refund_v2(PrepareWitnessedAssetRefundV2Request::new(
        context(&run, Participant::Maker, "outsider-refund"),
        outsider_runtime,
        token_asset(h(82)),
    ))
    .await;
    assert!(matches!(
        result,
        Err(BridgeClientError::MalformedObservation {
            operation: BridgeOperation::PrepareWitnessedAssetRefundV2
        })
    ));
    assert_eq!(
        sidecar
            .fixture
            .calls(METHOD_PREPARE_WITNESSED_ASSET_REFUND_V2),
        0
    );
}

fn response_context(context: &MessageContext, behavior: Behavior) -> MessageContext {
    if matches!(behavior, Behavior::WrongContext) {
        MessageContext::new(
            context.run_id.clone(),
            context.request_id.clone(),
            Participant::Taker,
        )
    } else {
        context.clone()
    }
}

fn response_terms(
    terms: &WitnessedLezAssetTermsV2,
    behavior: Behavior,
) -> WitnessedLezAssetTermsV2 {
    if matches!(behavior, Behavior::WrongTerms) {
        token_asset(h(83))
    } else {
        terms.clone()
    }
}

fn run_id() -> RunId {
    RunId::new(TEST_RUN).expect("run id")
}

fn context(run: &RunId, role: Participant, suffix: &str) -> MessageContext {
    MessageContext::new(
        run.clone(),
        RequestId::new(format!("asset-v2-{suffix}")).expect("request id"),
        role,
    )
}

fn client(
    endpoint: &str,
    run: &RunId,
    runtime: RuntimeDescriptor,
    timeout: Duration,
) -> BridgeClient {
    BridgeClient::connect(BridgeClientConfig::new(
        endpoint,
        SidecarCapability::new(CAPABILITY).expect("capability"),
        run.clone(),
        runtime,
        timeout,
    ))
    .expect("client configuration")
}

fn runtime(role: Participant) -> RuntimeDescriptor {
    RuntimeDescriptor::new(
        role,
        RuntimeCompatibility::LeeV0_2_0,
        h(101),
        h(102),
        h(104),
        h(103),
        match role {
            Participant::Maker => h(73),
            Participant::Taker => h(75),
        },
    )
}

fn native_asset() -> WitnessedLezAssetTermsV2 {
    WitnessedLezAssetTermsV2::native(
        WitnessedNativeEscrowTerms::new(WitnessedNativeEscrowTermsInput {
            swap_id: h(40),
            terms_hash: h(41),
            depositor: Participant::Maker,
            depositor_account_id: h(73),
            claimant: Participant::Taker,
            claimant_account_id: h(75),
            aggregate_authority_account_id: h(78),
            aggregate_x_only_public_key: h(79),
            amount: 75,
            refund_at_ms: 1_850_000_000_123,
            authenticated_transfer_program_id: h(46),
        })
        .expect("native terms"),
    )
}

fn token_asset(definition: Hex32) -> WitnessedLezAssetTermsV2 {
    WitnessedLezAssetTermsV2::custom_token(
        WitnessedTokenEscrowTermsV2::new(WitnessedTokenEscrowTermsV2Input {
            swap_id: h(80),
            terms_hash: h(81),
            depositor: Participant::Maker,
            depositor_owner_account_id: h(73),
            depositor_ata_account_id: h(74),
            claimant: Participant::Taker,
            claimant_owner_account_id: h(75),
            claimant_ata_account_id: h(76),
            custody_ata_account_id: h(77),
            token_program_id: h(70),
            ata_program_id: h(71),
            token_definition_account_id: definition,
            aggregate_authority_account_id: h(78),
            aggregate_x_only_public_key: h(79),
            amount: 125,
            refund_at_ms: 1_850_000_000_456,
        })
        .expect("token terms"),
    )
}

fn prepared_effects(terms: &WitnessedLezAssetTermsV2) -> Vec<WitnessedAssetPreparedEffectV2> {
    let mut effects = vec![WitnessedAssetPreparedEffectV2::new(
        WitnessedAssetPrepareStepV2::InitializeWitnessed,
        tx(105),
    )];
    if matches!(terms.asset(), WitnessedLezAssetV2::CustomToken(_)) {
        effects.push(WitnessedAssetPreparedEffectV2::new(
            WitnessedAssetPrepareStepV2::CreateCustodyAta,
            tx(106),
        ));
    }
    effects.push(WitnessedAssetPreparedEffectV2::new(
        WitnessedAssetPrepareStepV2::Fund,
        tx(107),
    ));
    effects
}

fn observed_prepare_effects(
    terms: &WitnessedLezAssetTermsV2,
    metadata: Hex32,
) -> Vec<WitnessedAssetObservedPrepareEffectV2> {
    match terms.asset() {
        WitnessedLezAssetV2::Native(terms) => vec![
            observed_effect(
                WitnessedAssetPrepareStepV2::InitializeWitnessed,
                105,
                vec![
                    metadata,
                    h(146),
                    terms.depositor_account_id(),
                    terms.claimant_account_id(),
                    terms.aggregate_authority_account_id(),
                ],
            ),
            observed_effect(
                WitnessedAssetPrepareStepV2::Fund,
                107,
                vec![metadata, h(146), terms.depositor_account_id()],
            ),
        ],
        WitnessedLezAssetV2::CustomToken(terms) => vec![
            observed_effect(
                WitnessedAssetPrepareStepV2::InitializeWitnessed,
                105,
                vec![
                    metadata,
                    terms.depositor_owner_account_id(),
                    terms.claimant_owner_account_id(),
                    terms.token_definition_account_id(),
                    terms.aggregate_authority_account_id(),
                ],
            ),
            observed_effect(
                WitnessedAssetPrepareStepV2::CreateCustodyAta,
                106,
                vec![
                    metadata,
                    terms.token_definition_account_id(),
                    terms.custody_ata_account_id(),
                ],
            ),
            observed_effect(
                WitnessedAssetPrepareStepV2::Fund,
                107,
                vec![
                    metadata,
                    terms.depositor_owner_account_id(),
                    terms.depositor_ata_account_id(),
                    terms.custody_ata_account_id(),
                ],
            ),
        ],
    }
}

fn observed_effect(
    step: WitnessedAssetPrepareStepV2,
    byte: u8,
    accounts: Vec<Hex32>,
) -> WitnessedAssetObservedPrepareEffectV2 {
    let (block_hash, height) = match step {
        WitnessedAssetPrepareStepV2::InitializeWitnessed => (h(140), 90),
        WitnessedAssetPrepareStepV2::CreateCustodyAta => (h(141), 91),
        WitnessedAssetPrepareStepV2::Fund => (h(92), 92),
    };
    WitnessedAssetObservedPrepareEffectV2::new(
        step,
        observed_tx(byte, block_hash, height, h(73)),
        h(103),
        AccountIds::new(accounts).expect("account order"),
    )
}

fn asset_state(
    terms: &WitnessedLezAssetTermsV2,
    status: EscrowState,
    funded: bool,
) -> (WitnessedEscrowMetadataFacts, WitnessedAssetCustodyFactsV2) {
    match terms.asset() {
        WitnessedLezAssetV2::Native(terms) => {
            let metadata = WitnessedEscrowMetadataFacts::from_witnessed_native_terms(
                h(141),
                h(103),
                h(146),
                terms,
                status,
            );
            let balance = if funded { terms.amount().as_u128() } else { 0 };
            (
                metadata,
                WitnessedAssetCustodyFactsV2::Native(NativeCustodyFacts::new(
                    h(146),
                    terms.authenticated_transfer_program_id(),
                    balance,
                )),
            )
        }
        WitnessedLezAssetV2::CustomToken(terms) => {
            let metadata = WitnessedEscrowMetadataFacts::from_witnessed_token_terms(
                h(141),
                h(103),
                terms,
                status,
            );
            let balance = if funded { terms.amount().as_u128() } else { 0 };
            (
                metadata,
                WitnessedAssetCustodyFactsV2::CustomToken(TokenHoldingFactsV2::new(
                    terms.custody_ata_account_id(),
                    terms.token_program_id(),
                    terms.token_definition_account_id(),
                    balance,
                )),
            )
        }
    }
}

fn refund_facts(
    terms: &WitnessedLezAssetTermsV2,
    metadata: &WitnessedEscrowMetadataFacts,
    transaction_byte: u8,
) -> WitnessedAssetRefundFoundFactsV2 {
    let (swap_id, accounts, signer) = match terms.asset() {
        WitnessedLezAssetV2::Native(terms) => (
            terms.swap_id(),
            vec![
                metadata.account_id,
                metadata.custody_account_id,
                terms.depositor_account_id(),
            ],
            terms.depositor_account_id(),
        ),
        WitnessedLezAssetV2::CustomToken(terms) => (
            terms.swap_id(),
            vec![
                metadata.account_id,
                terms.custody_ata_account_id(),
                terms.depositor_ata_account_id(),
            ],
            terms.depositor_owner_account_id(),
        ),
    };
    WitnessedAssetRefundFoundFactsV2::new(
        observed_tx(transaction_byte, h(143), 91, signer),
        WitnessedAssetRefundInstructionFactsV2::new(
            h(103),
            AccountIds::new(accounts).expect("refund accounts"),
            swap_id,
        ),
    )
}

fn finalized_claim_facts(
    terms: &WitnessedLezAssetTermsV2,
    claim: PreparedWitnessedClaim,
) -> FinalizedWitnessedAssetClaimFactsV2 {
    let (metadata, custody) = asset_state(terms, EscrowState::Claimed, false);
    let (swap_id, accounts, signer) = match terms.asset() {
        WitnessedLezAssetV2::Native(terms) => (
            terms.swap_id(),
            vec![
                metadata.account_id,
                metadata.custody_account_id,
                terms.claimant_account_id(),
                terms.aggregate_authority_account_id(),
            ],
            terms.aggregate_authority_account_id(),
        ),
        WitnessedLezAssetV2::CustomToken(terms) => (
            terms.swap_id(),
            vec![
                metadata.account_id,
                terms.custody_ata_account_id(),
                terms.claimant_owner_account_id(),
                terms.claimant_ata_account_id(),
                terms.aggregate_authority_account_id(),
            ],
            terms.aggregate_authority_account_id(),
        ),
    };
    FinalizedWitnessedAssetClaimFactsV2::new(
        observed_tx(124, h(143), 91, signer),
        WitnessedAssetClaimInstructionFactsV2::new(
            h(103),
            AccountIds::new(accounts).expect("claim accounts"),
            swap_id,
            claim,
        ),
        AggregateBip340Signature::from_bytes([123; 64]),
        FinalizedBlockIdentity::new(91, h(143), 1_850_000_001_750),
        metadata,
        custody,
    )
}

fn finalized_native_initialization_facts() -> FinalizedWitnessedAssetInitializationFactsV2 {
    let WitnessedLezAssetV2::Native(terms) = native_asset().asset().clone() else {
        unreachable!()
    };
    let metadata = WitnessedEscrowMetadataFacts::from_witnessed_native_terms(
        h(145),
        h(103),
        h(146),
        &terms,
        EscrowState::Empty,
    );
    FinalizedWitnessedAssetInitializationFactsV2::new(
        observed_tx(132, h(147), 90, terms.depositor_account_id()),
        WitnessedAssetEffectInstructionFactsV2::new(
            WitnessedAssetPrepareStepV2::InitializeWitnessed,
            h(103),
            AccountIds::new(vec![
                metadata.account_id,
                metadata.custody_account_id,
                terms.depositor_account_id(),
                terms.claimant_account_id(),
                terms.aggregate_authority_account_id(),
            ])
            .expect("initialization accounts"),
            terms.swap_id(),
        ),
        FinalizedBlockIdentity::new(90, h(147), 1_850_000_001_700),
        metadata,
        WitnessedAssetInitializationCustodyFactsV2::native(NativeCustodyFacts::new(
            h(146),
            terms.authenticated_transfer_program_id(),
            0,
        )),
    )
}

fn claim_transcript() -> PreparedWitnessedClaim {
    let bytes = vec![121; 128];
    let mut hasher = Sha256::new();
    hasher.update(b"/LEE/v0.3/Message/Public/\x00\x00\x00\x00\x00\x00\x00");
    hasher.update(&bytes);
    PreparedWitnessedClaim::new(
        RequestId::new("asset-v2-claim-transcript").expect("request id"),
        Hex32::from_bytes(hasher.finalize().into()),
        ExactMessageBytes::new(bytes).expect("message bytes"),
    )
}

fn tx(byte: u8) -> PreparedTransaction {
    PreparedTransaction::new(
        TransactionId::from_bytes([byte; 32]),
        ExactTransactionBytes::new(vec![byte; 128]).expect("transaction bytes"),
    )
}

fn observed_tx(
    byte: u8,
    block_hash: Hex32,
    height: u64,
    signer: Hex32,
) -> ObservedTransactionFacts {
    ObservedTransactionFacts::new(
        TransactionId::from_bytes([byte; 32]),
        ExactTransactionBytes::new(vec![byte; 128]).expect("observed bytes"),
        ChainPosition::new(block_hash, height, 0),
        AccountIds::new(vec![signer]).expect("signer"),
        true,
    )
}

fn window() -> DiscoveryWindow {
    DiscoveryWindow::new(90, 3).expect("window")
}

fn finalized_clock() -> ChainClock {
    ChainClock::new(h(150), 92, 1_850_000_001_900)
}

fn tip(height: u8) -> ChainTip {
    ChainTip::new(h(height), u64::from(height))
}

fn clock(height: u8) -> ChainClock {
    ChainClock::new(h(height), u64::from(height), 1_850_000_001_900)
}

fn h(byte: u8) -> Hex32 {
    Hex32::from_bytes([byte; 32])
}
