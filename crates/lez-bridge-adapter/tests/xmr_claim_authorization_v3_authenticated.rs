//! M4 authenticated Stage-B claim-authorization adapter contract.

use std::sync::{
    Arc, Mutex, OnceLock,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;

use jsonrpsee::RpcModule;
use lez_adaptor_signature::{AdaptorSessionContext, AdaptorSigner, SigningRole};
use lez_bridge_adapter::{
    LezBridgeAdapter, PreparedXmrClaimAuthorizationErrorV3, XmrLezBridgeBindingV3,
};
use lez_bridge_client::{
    BridgeClient, BridgeClientConfig, BridgeClientError, BridgeOperation,
    METHOD_PREPARE_NATIVE_XMR_CLAIM_AUTHORIZATION_V3, RUN_ID_HEADER, SIDECAR_ROLE_HEADER,
    SidecarCapability,
};
use lez_bridge_protocol::{
    ExactTransactionBytes, Hex32, MessageContext, Participant as BridgeParticipant,
    PrepareNativeXmrClaimAuthorizationV3Request, PrepareNativeXmrClaimAuthorizationV3Result,
    PreparedTransaction, RequestId, RunId, RuntimeCompatibility, RuntimeDescriptor, TransactionId,
};
use lez_swap_core::Participant;
use lez_xmr_swap_sdk::{
    CrossCurveDleqProofV1, CrossCurveScalar, MoneroAddressNetworkV1, MoneroPrivateViewKey,
    MoneroSharedAddressV1, XMR_ACTIVATION_SCHEMA_V1, XMR_AGREEMENT_SCHEMA_V1,
    XmrActivatedAgreementV1, XmrActivationBodyV1, XmrActivationRecordV1, XmrAgreementBodyV1,
    XmrAgreementRecordV1, XmrAgreementV1, XmrLezTermsV1, XmrMessagesV1, XmrMoneroTermsV1,
    XmrNamedProfileV1, XmrParticipantIdentityV1, XmrParticipantsV1, XmrRoleV1,
    XmrSessionTranscriptV1, XmrSwapDirectionV1, XmrWindowsV1,
};
use rand_chacha::{ChaCha20Rng, rand_core::SeedableRng as _};
use secp256k1::{Keypair, Message as SecpMessage, PublicKey, Secp256k1, SecretKey};
use serde_json::Value;
use sha2::{Digest as _, Sha256};
use tower::ServiceBuilder;
use tower_http::validate_request::ValidateRequestHeaderLayer;

const CAPABILITY: &str = "m4-xmr-claim-authorization-capability-0001";
const RUN: &str = "m4-xmr-claim-authorization-run";
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

fn stage_b(seed: u8) -> &'static StageBFixture {
    static PRIMARY: OnceLock<StageBFixture> = OnceLock::new();
    static OTHER: OnceLock<StageBFixture> = OnceLock::new();
    match seed {
        1 => PRIMARY.get_or_init(|| build_stage_b(1)),
        2 => OTHER.get_or_init(|| build_stage_b(2)),
        _ => panic!("unsupported fixture seed"),
    }
}

#[allow(
    clippy::too_many_lines,
    reason = "one canonical Stage-B fixture keeps agreement, adaptor rounds, signatures, and runtime joined"
)]
fn build_stage_b(seed: u8) -> StageBFixture {
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
        [18_u8.wrapping_add(seed); 32],
        participants,
        XmrMoneroTermsV1::new(
            MoneroAddressNetworkV1::Regtest,
            [31; 32],
            1_000_000_000_000,
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
            [44_u8.wrapping_add(seed); 32],
            [46_u8.wrapping_add(seed); 32],
            [22; 32],
            [21; 32],
            claim_key,
            XmrLezTermsV1::authority_account_for_key(claim_key),
            refund_key,
            XmrLezTermsV1::authority_account_for_key(refund_key),
            proof.maker_transcript_commitment,
            proof.taker_transcript_commitment,
            500 + u128::from(seed),
        ),
        XmrMessagesV1::new([51; 32], [52; 32], [53; 32]),
        XmrWindowsV1::new(10_000, 20_000, 30_000),
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

#[derive(Clone, Copy, Debug)]
enum Behavior {
    Happy,
    WrongContext,
    WrongTerms,
    MalformedTransaction,
}

#[derive(Clone, Debug)]
struct ServerFixture {
    behavior: Behavior,
    calls: Arc<AtomicUsize>,
    requests: Arc<Mutex<Vec<PrepareNativeXmrClaimAuthorizationV3Request>>>,
}

struct MockSidecar {
    endpoint: String,
    fixture: ServerFixture,
    _handle: jsonrpsee::server::ServerHandle,
}

async fn spawn_sidecar(role: BridgeParticipant, behavior: Behavior) -> MockSidecar {
    let fixture = ServerFixture {
        behavior,
        calls: Arc::new(AtomicUsize::new(0)),
        requests: Arc::new(Mutex::new(Vec::new())),
    };
    let authorization = format!("Bearer {CAPABILITY}");
    let middleware = ServiceBuilder::new()
        .layer(
            ValidateRequestHeaderLayer::has_header_value("authorization", &authorization)
                .expect("authorization header"),
        )
        .layer(
            ValidateRequestHeaderLayer::has_header_value(RUN_ID_HEADER, RUN).expect("run header"),
        )
        .layer(
            ValidateRequestHeaderLayer::has_header_value(
                SIDECAR_ROLE_HEADER,
                match role {
                    BridgeParticipant::Maker => "maker",
                    BridgeParticipant::Taker => "taker",
                },
            )
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
                fixture
                    .requests
                    .lock()
                    .expect("request recorder")
                    .push(request.clone());
                let context = if matches!(fixture.behavior, Behavior::WrongContext) {
                    MessageContext::new(
                        request.context.run_id.clone(),
                        RequestId::new("m4-wrong-response-context").expect("request ID"),
                        request.context.sidecar_role,
                    )
                } else {
                    request.context
                };
                let terms = if matches!(fixture.behavior, Behavior::WrongTerms) {
                    stage_b(2).binding.terms()
                } else {
                    request.terms
                };
                let result = PrepareNativeXmrClaimAuthorizationV3Result::new(
                    context,
                    terms,
                    authorization_transaction(),
                );
                let mut value = serde_json::to_value(result).expect("response JSON");
                if matches!(fixture.behavior, Behavior::MalformedTransaction) {
                    value["authorization"]["exact_bytes"] = Value::String(String::new());
                }
                Ok::<_, jsonrpsee::types::ErrorObjectOwned>(value)
            },
        )
        .expect("register claim authorization");
    let handle = server.start(module);
    MockSidecar {
        endpoint: format!("http://{address}"),
        fixture,
        _handle: handle,
    }
}

#[tokio::test]
async fn valid_stage_b_partial_routes_once_and_mints_exact_linear_capability() {
    let stage = stage_b(1);
    let sidecar = spawn_sidecar(BridgeParticipant::Taker, Behavior::Happy).await;
    let client = client(&sidecar.endpoint, run_id(), stage.runtime.clone());
    let adapter =
        LezBridgeAdapter::new(client, run_id(), stage.runtime.clone(), Participant::Taker)
            .expect("Taker adapter");

    let evidence = adapter
        .prepare_xmr_claim_authorization_v3(
            &stage.agreement,
            &stage.activation,
            &stage.binding,
            RequestId::new("m4-valid-authorization").expect("request ID"),
            stage.taker_claim_partial,
        )
        .await
        .expect("authorized preparation");

    assert_eq!(sidecar.fixture.calls.load(Ordering::SeqCst), 1);
    let requests = sidecar.fixture.requests.lock().expect("request recorder");
    let [request] = requests.as_slice() else {
        panic!("exactly one request")
    };
    assert_eq!(request.context.run_id, run_id());
    assert_eq!(request.context.sidecar_role, BridgeParticipant::Taker);
    assert_eq!(request.runtime, stage.runtime);
    assert_eq!(request.terms, stage.binding.terms());
    assert_eq!(
        request.claim_partial.expose_secret(),
        &stage.taker_claim_partial
    );
    assert_eq!(evidence.context(), &request.context);
    assert_eq!(evidence.preparer(), Participant::Taker);
    assert_eq!(evidence.runtime(), &stage.runtime);
    assert_eq!(evidence.terms(), stage.binding.terms());
    assert_eq!(evidence.authorization(), &authorization_transaction());
}

#[tokio::test]
#[allow(
    clippy::too_many_lines,
    reason = "all pure pre-wire authority failures share one authenticated call counter"
)]
async fn wrong_partial_stage_b_binding_run_role_and_runtime_make_zero_calls() {
    let stage = stage_b(1);
    let other = stage_b(2);
    let taker_sidecar = spawn_sidecar(BridgeParticipant::Taker, Behavior::Happy).await;

    let mut wrong_partial = stage.taker_claim_partial;
    wrong_partial[0] ^= 1;
    let adapter = taker_adapter(
        &taker_sidecar.endpoint,
        run_id(),
        stage.runtime.clone(),
        run_id(),
        stage.runtime.clone(),
    );
    assert!(matches!(
        adapter
            .prepare_xmr_claim_authorization_v3(
                &stage.agreement,
                &stage.activation,
                &stage.binding,
                request_id("wrong-partial"),
                wrong_partial,
            )
            .await,
        Err(PreparedXmrClaimAuthorizationErrorV3::PublishedPartial(_))
    ));

    let adapter = taker_adapter(
        &taker_sidecar.endpoint,
        run_id(),
        stage.runtime.clone(),
        run_id(),
        stage.runtime.clone(),
    );
    assert!(matches!(
        adapter
            .prepare_xmr_claim_authorization_v3(
                &other.agreement,
                &stage.activation,
                &stage.binding,
                request_id("wrong-stage-b"),
                stage.taker_claim_partial,
            )
            .await,
        Err(PreparedXmrClaimAuthorizationErrorV3::StageB(_))
    ));

    let adapter = taker_adapter(
        &taker_sidecar.endpoint,
        run_id(),
        stage.runtime.clone(),
        run_id(),
        stage.runtime.clone(),
    );
    assert!(matches!(
        adapter
            .prepare_xmr_claim_authorization_v3(
                &stage.agreement,
                &stage.activation,
                &other.binding,
                request_id("wrong-binding"),
                stage.taker_claim_partial,
            )
            .await,
        Err(PreparedXmrClaimAuthorizationErrorV3::BindingMismatch)
    ));

    let wrong_run = RunId::new("m4-wrong-adapter-run").expect("wrong run");
    let adapter = taker_adapter(
        &taker_sidecar.endpoint,
        run_id(),
        stage.runtime.clone(),
        wrong_run,
        stage.runtime.clone(),
    );
    assert!(matches!(
        adapter
            .prepare_xmr_claim_authorization_v3(
                &stage.agreement,
                &stage.activation,
                &stage.binding,
                request_id("wrong-run"),
                stage.taker_claim_partial,
            )
            .await,
        Err(PreparedXmrClaimAuthorizationErrorV3::Bridge(
            BridgeClientError::RequestContextMismatch { .. }
        ))
    ));

    let mut client_runtime = stage.runtime.clone();
    client_runtime.chain_id = h(99);
    let adapter = taker_adapter(
        &taker_sidecar.endpoint,
        run_id(),
        client_runtime,
        run_id(),
        stage.runtime.clone(),
    );
    assert!(matches!(
        adapter
            .prepare_xmr_claim_authorization_v3(
                &stage.agreement,
                &stage.activation,
                &stage.binding,
                request_id("wrong-client-runtime"),
                stage.taker_claim_partial,
            )
            .await,
        Err(PreparedXmrClaimAuthorizationErrorV3::Bridge(
            BridgeClientError::RequestContextMismatch { .. }
        ))
    ));

    let mut unsigned_runtime = stage.runtime.clone();
    unsigned_runtime.channel_id = h(98);
    let adapter = taker_adapter(
        &taker_sidecar.endpoint,
        run_id(),
        unsigned_runtime.clone(),
        run_id(),
        unsigned_runtime,
    );
    assert!(matches!(
        adapter
            .prepare_xmr_claim_authorization_v3(
                &stage.agreement,
                &stage.activation,
                &stage.binding,
                request_id("wrong-stage-runtime"),
                stage.taker_claim_partial,
            )
            .await,
        Err(PreparedXmrClaimAuthorizationErrorV3::RuntimeBinding(_))
    ));
    assert_eq!(taker_sidecar.fixture.calls.load(Ordering::SeqCst), 0);

    let maker_sidecar = spawn_sidecar(BridgeParticipant::Maker, Behavior::Happy).await;
    let mut maker_runtime = stage.runtime.clone();
    maker_runtime.sidecar_role = BridgeParticipant::Maker;
    maker_runtime.signer_account_id = Hex32::from_bytes([21; 32]);
    let maker_client = client(&maker_sidecar.endpoint, run_id(), maker_runtime.clone());
    let maker_adapter =
        LezBridgeAdapter::new(maker_client, run_id(), maker_runtime, Participant::Maker)
            .expect("Maker adapter");
    assert!(matches!(
        maker_adapter
            .prepare_xmr_claim_authorization_v3(
                &stage.agreement,
                &stage.activation,
                &stage.binding,
                request_id("wrong-role"),
                stage.taker_claim_partial,
            )
            .await,
        Err(PreparedXmrClaimAuthorizationErrorV3::WrongPreparer)
    ));
    assert_eq!(maker_sidecar.fixture.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn malformed_context_terms_and_transaction_fail_after_one_authenticated_call() {
    let stage = stage_b(1);
    let _ = stage_b(2);
    for (index, behavior) in [
        Behavior::WrongContext,
        Behavior::WrongTerms,
        Behavior::MalformedTransaction,
    ]
    .into_iter()
    .enumerate()
    {
        let sidecar = spawn_sidecar(BridgeParticipant::Taker, behavior).await;
        let adapter = taker_adapter(
            &sidecar.endpoint,
            run_id(),
            stage.runtime.clone(),
            run_id(),
            stage.runtime.clone(),
        );
        let error = adapter
            .prepare_xmr_claim_authorization_v3(
                &stage.agreement,
                &stage.activation,
                &stage.binding,
                request_id(&format!("malformed-{index}")),
                stage.taker_claim_partial,
            )
            .await
            .expect_err("malformed response must fail closed");
        match (behavior, error) {
            (
                Behavior::WrongContext,
                PreparedXmrClaimAuthorizationErrorV3::Bridge(
                    BridgeClientError::ResponseContextMismatch {
                        operation: BridgeOperation::PrepareNativeXmrClaimAuthorizationV3,
                    },
                ),
            )
            | (
                Behavior::WrongTerms,
                PreparedXmrClaimAuthorizationErrorV3::Bridge(
                    BridgeClientError::MalformedObservation {
                        operation: BridgeOperation::PrepareNativeXmrClaimAuthorizationV3,
                    },
                ),
            )
            | (
                Behavior::MalformedTransaction,
                PreparedXmrClaimAuthorizationErrorV3::Bridge(BridgeClientError::InvalidResponse {
                    operation: BridgeOperation::PrepareNativeXmrClaimAuthorizationV3,
                }),
            ) => {}
            (_, other) => panic!("unexpected {behavior:?} error: {other:?}"),
        }
        assert_eq!(sidecar.fixture.calls.load(Ordering::SeqCst), 1);
    }
}

fn taker_adapter(
    endpoint: &str,
    client_run: RunId,
    client_runtime: RuntimeDescriptor,
    adapter_run: RunId,
    adapter_runtime: RuntimeDescriptor,
) -> LezBridgeAdapter<BridgeClient> {
    LezBridgeAdapter::new(
        client(endpoint, client_run, client_runtime),
        adapter_run,
        adapter_runtime,
        Participant::Taker,
    )
    .expect("Taker adapter")
}

fn client(endpoint: &str, run: RunId, runtime: RuntimeDescriptor) -> BridgeClient {
    BridgeClient::connect(BridgeClientConfig::new(
        endpoint,
        SidecarCapability::new(CAPABILITY).expect("capability"),
        run,
        runtime,
        Duration::from_secs(2),
    ))
    .expect("bridge client")
}

fn authorization_transaction() -> PreparedTransaction {
    PreparedTransaction::new(
        TransactionId::from_bytes([70; 32]),
        ExactTransactionBytes::new(vec![70; 128]).expect("transaction bytes"),
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

fn request_id(suffix: &str) -> RequestId {
    RequestId::new(format!("m4-xmr-claim-authorization-{suffix}")).expect("request ID")
}

const fn h(byte: u8) -> Hex32 {
    Hex32::from_bytes([byte; 32])
}
