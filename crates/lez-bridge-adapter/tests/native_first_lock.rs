use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

use async_trait::async_trait;
use lez_bridge_adapter::{
    LezBridgeAdapter, LezBridgeObservationTransport, LezBridgeTransport, ObserveNativeEscrowError,
    PrepareNativeFirstLockError,
};
use lez_bridge_protocol::{
    AccountIds, ChainPosition, ChainTip, DiscoveryWindow, EscrowMetadataFacts,
    EscrowObservationTarget, EscrowState, ExactTransactionBytes, FundingFoundFacts,
    FundingObservation, Hex32, InitializationFoundFacts, InitializationObservation, MessageContext,
    NativeCustodyFacts, NativeEscrowTerms, NativeEscrowTermsInput, NativeFundInstructionFacts,
    NativeInitializeInstructionFacts, ObserveEscrowRequest, ObserveEscrowResult,
    ObservedTransactionFacts, Participant as BridgeParticipant, PrepareNativeEscrowRequest,
    PrepareNativeEscrowResult, PreparedTransaction, RequestId, RunId, RuntimeCompatibility,
    RuntimeDescriptor, TransactionId,
};
use lez_swap_core::{Participant, SwapDirection, UnixSeconds};
use lez_zec_swap_sdk::{
    Bip199Contract, ExpectedBip199Output, FirstLockPlanV1, FirstLockStepV1, LezAssetV1,
    LezChainIdentityV1, LezEnvironmentV1, NegotiationTranscriptV1, TakerFirstLockObservationV1,
    ZEC_CONCRETE_AGREEMENT_SCHEMA_V2, ZcashTransparentDestinationV1, ZecAgreementBodyV1,
    ZecAgreementRecordV1, ZecAgreementV1, ZecLezTermsV1, ZecParticipantIdentityV1,
    ZecParticipantsV1, ZecProfileId, ZecProfileRecordV1, ZecRefundPlanV1, ZecSwapBinding,
    ZecSwapBindingRecordV1, ZecTransactionPolicyV1, derive_lez_metadata_account_v1,
    derive_lez_native_custody_account_v1, derive_lez_swap_id_v1,
    derive_nssa_v0_1_2_metadata_account_v1, derive_nssa_v0_1_2_native_custody_account_v1,
    derive_nssa_v0_1_2_token_account_v1,
};
use secp256k1::{Message, PublicKey, Secp256k1, SecretKey};
use sha2::{Digest as _, Sha256};
use thiserror::Error;
use zcash_protocol::{
    consensus::{BranchId, NetworkType},
    value::Zatoshis,
};
use zcash_transparent::address::TransparentAddress;

#[derive(Clone, Debug, Default)]
struct FakeTransport {
    requests: Arc<Mutex<Vec<PrepareNativeEscrowRequest>>>,
}

#[derive(Clone, Copy, Debug, Error)]
#[error("fake transport failure")]
struct FakeError;

#[derive(Clone, Debug)]
struct ObservationTransport {
    requests: Arc<Mutex<Vec<ObserveEscrowRequest>>>,
    response: Arc<Mutex<Option<ObserveEscrowResult>>>,
    attempts: Arc<AtomicUsize>,
}

impl ObservationTransport {
    fn new(response: ObserveEscrowResult) -> Self {
        Self {
            requests: Arc::default(),
            response: Arc::new(Mutex::new(Some(response))),
            attempts: Arc::default(),
        }
    }
}

#[async_trait]
impl LezBridgeObservationTransport for ObservationTransport {
    type Error = FakeError;

    async fn observe_escrow(
        &self,
        request: ObserveEscrowRequest,
    ) -> Result<ObserveEscrowResult, Self::Error> {
        self.attempts.fetch_add(1, Ordering::SeqCst);
        self.requests.lock().expect("request log").push(request);
        self.response
            .lock()
            .expect("response")
            .take()
            .ok_or(FakeError)
    }
}

#[tokio::test]
async fn owner_exact_observation_uses_the_caller_owned_ids() {
    let agreement = agreement();
    let context = observation_context(Participant::Taker, "observe-0001");
    let transport = ObservationTransport::new(canonical_observation(&agreement, context));
    let adapter = observation_adapter(transport.clone(), &agreement, Participant::Taker);

    let observed = adapter
        .observe_native_escrow(
            &agreement,
            RequestId::new("observe-0001").expect("request id"),
            EscrowObservationTarget::Exact {
                initialization_transaction_id: TransactionId::from_bytes([0x11; 32]),
                funding_transaction_id: TransactionId::from_bytes([0x22; 32]),
            },
        )
        .await
        .expect("canonical signed escrow");

    let TakerFirstLockObservationV1::CanonicalLez(canonical) = observed else {
        panic!("complete stable facts must produce canonical LEZ evidence");
    };
    assert_eq!(canonical.transaction_id(), &[0x22; 32]);
    assert_eq!(canonical.confirmations().get(), 2);
    assert_eq!(transport.attempts.load(Ordering::SeqCst), 1);
    let requests = transport.requests.lock().expect("request log");
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].context,
        observation_context(Participant::Taker, "observe-0001")
    );
    assert!(
        matches!(requests[0].target, EscrowObservationTarget::Exact {
        initialization_transaction_id,
        funding_transaction_id,
    } if initialization_transaction_id == TransactionId::from_bytes([0x11; 32])
        && funding_transaction_id == TransactionId::from_bytes([0x22; 32]))
    );
}

#[tokio::test]
async fn claimant_discovery_preserves_the_bounded_window_and_validates_the_same_escrow() {
    let agreement = agreement();
    let context = observation_context(Participant::Maker, "discover-0001");
    let transport = ObservationTransport::new(canonical_observation(&agreement, context));
    let adapter = observation_adapter(transport.clone(), &agreement, Participant::Maker);
    let window = DiscoveryWindow::new(4, 12).expect("bounded window");

    let observed = adapter
        .observe_native_escrow(
            &agreement,
            RequestId::new("discover-0001").expect("request id"),
            EscrowObservationTarget::DiscoverByTerms { window },
        )
        .await
        .expect("counterparty discovery");

    assert!(matches!(
        observed,
        TakerFirstLockObservationV1::CanonicalLez(_)
    ));
    assert_eq!(transport.attempts.load(Ordering::SeqCst), 1);
    assert!(matches!(
        transport.requests.lock().expect("request log")[0].target,
        EscrowObservationTarget::DiscoverByTerms { window: actual }
            if actual == window
    ));
}

#[tokio::test]
async fn discovery_requires_window_membership_and_full_coverage_for_absence() {
    let agreement = agreement();
    let outside_transport = ObservationTransport::new(canonical_observation(
        &agreement,
        observation_context(Participant::Maker, "discover-outside"),
    ));
    let outside = observation_adapter(outside_transport, &agreement, Participant::Maker)
        .observe_native_escrow(
            &agreement,
            RequestId::new("discover-outside").expect("request id"),
            EscrowObservationTarget::DiscoverByTerms {
                window: DiscoveryWindow::new(11, 2).expect("window"),
            },
        )
        .await;
    assert!(matches!(
        outside,
        Err(ObserveNativeEscrowError::InconsistentFacts)
    ));

    for (window, expected) in [
        (
            DiscoveryWindow::new(10, 4).expect("window ending above tip"),
            "unstable",
        ),
        (
            DiscoveryWindow::new(9, 4).expect("fully covered window"),
            "absent",
        ),
    ] {
        let context = observation_context(Participant::Maker, "discover-absence");
        let tip = ChainTip::new(Hex32::from_bytes([0x90; 32]), 12);
        let response = ObserveEscrowResult::new(
            context,
            tip,
            InitializationObservation::Absent,
            FundingObservation::Absent,
            tip,
        );
        let transport = ObservationTransport::new(response);
        let actual = observation_adapter(transport, &agreement, Participant::Maker)
            .observe_native_escrow(
                &agreement,
                RequestId::new("discover-absence").expect("request id"),
                EscrowObservationTarget::DiscoverByTerms { window },
            )
            .await
            .expect("conservative absence classification");
        match expected {
            "unstable" => assert!(matches!(actual, TakerFirstLockObservationV1::Unstable)),
            "absent" => assert!(matches!(actual, TakerFirstLockObservationV1::Absent)),
            _ => unreachable!("fixed status"),
        }
    }
}

#[tokio::test]
async fn target_ownership_is_rejected_without_transport() {
    let agreement = agreement();
    let maker_transport = ObservationTransport::new(canonical_observation(
        &agreement,
        observation_context(Participant::Maker, "owner-role-1"),
    ));
    let maker = observation_adapter(maker_transport.clone(), &agreement, Participant::Maker);
    let exact_error = maker
        .observe_native_escrow(
            &agreement,
            RequestId::new("owner-role-1").expect("request id"),
            exact_target(),
        )
        .await
        .expect_err("claimant cannot use owner-local exact IDs");
    assert!(matches!(
        exact_error,
        ObserveNativeEscrowError::ExactTargetRequiresDepositor
    ));
    assert_eq!(maker_transport.attempts.load(Ordering::SeqCst), 0);

    let taker_transport = ObservationTransport::new(canonical_observation(
        &agreement,
        observation_context(Participant::Taker, "owner-role-2"),
    ));
    let taker = observation_adapter(taker_transport.clone(), &agreement, Participant::Taker);
    let discovery_error = taker
        .observe_native_escrow(
            &agreement,
            RequestId::new("owner-role-2").expect("request id"),
            EscrowObservationTarget::DiscoverByTerms {
                window: DiscoveryWindow::new(1, 1).expect("window"),
            },
        )
        .await
        .expect_err("depositor cannot invent counterparty discovery");
    assert!(matches!(
        discovery_error,
        ObserveNativeEscrowError::DiscoveryRequiresClaimant
    ));
    assert_eq!(taker_transport.attempts.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn observation_runtime_mismatches_are_rejected_without_transport() {
    let agreement = agreement();
    for (mutation, expected) in [
        (RuntimeMutation::Channel, "chain"),
        (RuntimeMutation::Genesis, "chain"),
        (RuntimeMutation::Program, "program"),
        (RuntimeMutation::Signer, "signer"),
    ] {
        let context = observation_context(Participant::Taker, "runtime-observe");
        let transport = ObservationTransport::new(canonical_observation(&agreement, context));
        let mut descriptor = runtime(&agreement);
        match mutation {
            RuntimeMutation::Channel => descriptor.channel_id = Hex32::from_bytes([0x71; 32]),
            RuntimeMutation::Genesis => {
                descriptor.genesis_block_hash = Hex32::from_bytes([0x72; 32]);
            }
            RuntimeMutation::Program => {
                descriptor.escrow_program_id = Hex32::from_bytes([0x73; 32]);
            }
            RuntimeMutation::Signer => {
                descriptor.signer_account_id = Hex32::from_bytes([0x74; 32]);
            }
        }
        let adapter = LezBridgeAdapter::new(
            transport.clone(),
            RunId::new("native-run-0001").expect("run id"),
            descriptor,
            Participant::Taker,
        )
        .expect("matching role");
        let error = adapter
            .observe_native_escrow(
                &agreement,
                RequestId::new("runtime-observe").expect("request id"),
                exact_target(),
            )
            .await
            .expect_err("runtime mismatch");
        match expected {
            "chain" => assert!(matches!(
                error,
                ObserveNativeEscrowError::ChainIdentityMismatch
            )),
            "program" => assert!(matches!(
                error,
                ObserveNativeEscrowError::EscrowProgramMismatch
            )),
            "signer" => assert!(matches!(
                error,
                ObserveNativeEscrowError::SignerAccountMismatch
            )),
            _ => unreachable!("fixed expected category"),
        }
        assert_eq!(transport.attempts.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn unsupported_observation_agreements_are_rejected_without_transport() {
    for (agreement, expected, request_id) in [
        (
            agreement_for(LezEnvironmentV1::DeterministicLocalV0_2, false),
            "environment",
            "observe-v02",
        ),
        (
            agreement_for(
                LezEnvironmentV1::DeterministicLocalV0_1_2Compatibility,
                true,
            ),
            "asset",
            "observe-token",
        ),
    ] {
        let context = observation_context(Participant::Taker, request_id);
        let response = ObserveEscrowResult::new(
            context,
            ChainTip::new(Hex32::from_bytes([1; 32]), 1),
            InitializationObservation::Absent,
            FundingObservation::Absent,
            ChainTip::new(Hex32::from_bytes([1; 32]), 1),
        );
        let transport = ObservationTransport::new(response);
        let adapter = observation_adapter(transport.clone(), &agreement, Participant::Taker);
        let error = adapter
            .observe_native_escrow(
                &agreement,
                RequestId::new(request_id).expect("request id"),
                exact_target(),
            )
            .await
            .expect_err("unsupported agreement");
        match expected {
            "environment" => assert!(matches!(
                error,
                ObserveNativeEscrowError::IncompatibleEnvironment
            )),
            "asset" => assert!(matches!(error, ObserveNativeEscrowError::UnsupportedAsset)),
            _ => unreachable!("fixed expected category"),
        }
        assert_eq!(transport.attempts.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn absent_unknown_and_partial_states_never_create_evidence() {
    let agreement = agreement();
    for (initialization, funding, expected) in [
        (
            InitializationObservation::Absent,
            FundingObservation::Absent,
            "absent",
        ),
        (
            InitializationObservation::UnknownOrPending,
            FundingObservation::UnknownOrPending,
            "unstable",
        ),
        (
            canonical_observation(
                &agreement,
                observation_context(Participant::Taker, "partial-template"),
            )
            .initialization,
            FundingObservation::Absent,
            "unstable",
        ),
    ] {
        let context = observation_context(Participant::Taker, "status-observe");
        let tip = ChainTip::new(Hex32::from_bytes([0x90; 32]), 12);
        let transport = ObservationTransport::new(ObserveEscrowResult::new(
            context,
            tip,
            initialization,
            funding,
            tip,
        ));
        let adapter = observation_adapter(transport, &agreement, Participant::Taker);
        let actual = adapter
            .observe_native_escrow(
                &agreement,
                RequestId::new("status-observe").expect("request id"),
                exact_target(),
            )
            .await
            .expect("conservative status");
        match expected {
            "absent" => assert!(matches!(actual, TakerFirstLockObservationV1::Absent)),
            "unstable" => assert!(matches!(actual, TakerFirstLockObservationV1::Unstable)),
            _ => unreachable!("fixed status"),
        }
    }

    let mut inconsistent = canonical_observation(
        &agreement,
        observation_context(Participant::Taker, "partial-found"),
    );
    inconsistent.initialization = InitializationObservation::Absent;
    let transport = ObservationTransport::new(inconsistent);
    let adapter = observation_adapter(transport, &agreement, Participant::Taker);
    assert!(matches!(
        adapter
            .observe_native_escrow(
                &agreement,
                RequestId::new("partial-found").expect("request id"),
                exact_target(),
            )
            .await,
        Err(ObserveNativeEscrowError::InconsistentFacts)
    ));
}

#[tokio::test]
async fn response_context_tip_and_primitive_mutations_fail_closed() {
    let agreement = agreement();
    for mutation in ALL_OBSERVATION_MUTATIONS {
        let context = observation_context(Participant::Taker, "mutated-observe");
        let mut response = canonical_observation(&agreement, context);
        mutate_observation(&agreement, &mut response, mutation);
        let transport = ObservationTransport::new(response);
        let adapter = observation_adapter(transport, &agreement, Participant::Taker);
        let result = adapter
            .observe_native_escrow(
                &agreement,
                RequestId::new("mutated-observe").expect("request id"),
                exact_target(),
            )
            .await;
        assert!(result.is_err(), "mutation {mutation:?} must fail closed");
    }
}

#[derive(Clone, Debug, Default)]
struct FailingObservationTransport {
    attempts: Arc<AtomicUsize>,
}

#[async_trait]
impl LezBridgeObservationTransport for FailingObservationTransport {
    type Error = FakeError;

    async fn observe_escrow(
        &self,
        _request: ObserveEscrowRequest,
    ) -> Result<ObserveEscrowResult, Self::Error> {
        self.attempts.fetch_add(1, Ordering::SeqCst);
        Err(FakeError)
    }
}

#[tokio::test]
async fn unknown_observation_transport_outcome_is_not_retried() {
    let agreement = agreement();
    let transport = FailingObservationTransport::default();
    let adapter = LezBridgeAdapter::new(
        transport.clone(),
        RunId::new("native-run-0001").expect("run id"),
        runtime(&agreement),
        Participant::Taker,
    )
    .expect("matching actor sidecar");
    assert!(matches!(
        adapter
            .observe_native_escrow(
                &agreement,
                RequestId::new("observe-unknown").expect("request id"),
                exact_target(),
            )
            .await,
        Err(ObserveNativeEscrowError::Transport(FakeError))
    ));
    assert_eq!(transport.attempts.load(Ordering::SeqCst), 1);
}

#[async_trait]
impl LezBridgeTransport for FakeTransport {
    type Error = FakeError;

    async fn prepare_native_escrow(
        &self,
        request: PrepareNativeEscrowRequest,
    ) -> Result<PrepareNativeEscrowResult, Self::Error> {
        self.requests
            .lock()
            .expect("request log")
            .push(request.clone());
        Ok(prepared_response(request.context))
    }
}

#[tokio::test]
async fn signed_native_terms_prepare_an_exact_lez_first_lock_plan() {
    let agreement = agreement();
    let transport = FakeTransport::default();
    let adapter = adapter(transport.clone(), &agreement);

    let plan = adapter
        .prepare_native_first_lock(
            &agreement,
            RequestId::new("prepare-0001").expect("request id"),
        )
        .await
        .expect("signed terms prepare");

    let requests = transport.requests.lock().expect("request log");
    assert_eq!(
        requests.len(),
        1,
        "randomized preparation is attempted once"
    );
    let request = &requests[0];
    assert_eq!(request.context.run_id.as_str(), "native-run-0001");
    assert_eq!(request.context.request_id.as_str(), "prepare-0001");
    assert_eq!(request.context.sidecar_role, BridgeParticipant::Taker);
    assert_eq!(request.runtime, runtime(&agreement));
    assert_eq!(
        request.terms.swap_id().as_bytes(),
        agreement.onchain_swap_id()
    );
    assert_eq!(
        request.terms.terms_hash().as_bytes(),
        agreement.agreement_commitment()
    );
    assert_eq!(
        request.terms.secret_digest().as_bytes(),
        agreement.secret_digest()
    );
    assert_eq!(request.terms.depositor(), BridgeParticipant::Taker);
    assert_eq!(
        request.terms.depositor_account_id().as_bytes(),
        agreement.lez_account(Participant::Taker)
    );
    assert_eq!(request.terms.claimant(), BridgeParticipant::Maker);
    assert_eq!(
        request.terms.claimant_account_id().as_bytes(),
        agreement.lez_account(Participant::Maker)
    );
    assert_eq!(request.terms.amount().as_u128(), 42);
    assert_eq!(request.terms.refund_at_ms(), agreement.lez_refund_at_ms());
    assert_eq!(
        request.terms.authenticated_transfer_program_id().as_bytes(),
        &program_bytes(&[2; 8])
    );

    let FirstLockPlanV1::Lez { initialize, fund } = plan else {
        panic!("LEZ depositor must receive a LEZ first-lock plan");
    };
    assert_eq!(initialize.step(), FirstLockStepV1::LezInitialize);
    assert_eq!(initialize.expected_submission_id(), &[0x11; 32]);
    assert_eq!(initialize.exact_submission(), [0xaa, 0xbb]);
    assert_eq!(fund.step(), FirstLockStepV1::LezFund);
    assert_eq!(fund.expected_submission_id(), &[0x22; 32]);
    assert_eq!(fund.exact_submission(), [0xcc, 0xdd]);
}

#[tokio::test]
async fn non_depositor_is_rejected_before_randomized_preparation() {
    let agreement = agreement();
    let transport = FakeTransport::default();
    let adapter = LezBridgeAdapter::new(
        transport.clone(),
        RunId::new("native-run-0001").expect("run id"),
        RuntimeDescriptor::new(
            BridgeParticipant::Maker,
            RuntimeCompatibility::NssaV0_1_2,
            Hex32::from_bytes([6; 32]),
            Hex32::from_bytes(*agreement.lez_terms().chain().channel_id()),
            Hex32::from_bytes(*agreement.lez_terms().chain().genesis_block_hash()),
            Hex32::from_bytes(program_bytes(agreement.lez_terms().escrow_program_id())),
            Hex32::from_bytes(*agreement.lez_account(Participant::Maker)),
        ),
        Participant::Maker,
    )
    .expect("matching actor sidecar");

    let error = adapter
        .prepare_native_first_lock(
            &agreement,
            RequestId::new("prepare-0002").expect("request id"),
        )
        .await
        .expect_err("claimant cannot prepare depositor first lock");
    assert!(matches!(error, PrepareNativeFirstLockError::WrongDepositor));
    assert!(transport.requests.lock().expect("request log").is_empty());
}

#[tokio::test]
async fn runtime_identity_mismatches_are_rejected_before_preparation() {
    let agreement = agreement();

    let mut wrong_chain = runtime(&agreement);
    wrong_chain.channel_id = Hex32::from_bytes([0x91; 32]);
    assert_preparation_rejected(
        &agreement,
        wrong_chain,
        PrepareNativeFirstLockError::ChainIdentityMismatch,
        "wrong-chain",
    )
    .await;

    let mut wrong_program = runtime(&agreement);
    wrong_program.escrow_program_id = Hex32::from_bytes([0x92; 32]);
    assert_preparation_rejected(
        &agreement,
        wrong_program,
        PrepareNativeFirstLockError::EscrowProgramMismatch,
        "wrong-program",
    )
    .await;

    let mut wrong_signer = runtime(&agreement);
    wrong_signer.signer_account_id = Hex32::from_bytes([0x93; 32]);
    assert_preparation_rejected(
        &agreement,
        wrong_signer,
        PrepareNativeFirstLockError::SignerAccountMismatch,
        "wrong-signer",
    )
    .await;
}

#[tokio::test]
async fn incompatible_environment_and_token_are_rejected_without_transport() {
    for (agreement, expected, request_id) in [
        (
            agreement_for(LezEnvironmentV1::DeterministicLocalV0_2, false),
            "environment",
            "bad-environment",
        ),
        (
            agreement_for(
                LezEnvironmentV1::DeterministicLocalV0_1_2Compatibility,
                true,
            ),
            "asset",
            "bad-token-asset",
        ),
    ] {
        let transport = FakeTransport::default();
        let adapter = adapter(transport.clone(), &agreement);
        let error = adapter
            .prepare_native_first_lock(&agreement, RequestId::new(request_id).expect("request id"))
            .await
            .expect_err("unsupported signed terms fail closed");
        match expected {
            "environment" => assert!(matches!(
                error,
                PrepareNativeFirstLockError::IncompatibleEnvironment
            )),
            "asset" => assert!(matches!(
                error,
                PrepareNativeFirstLockError::UnsupportedAsset
            )),
            _ => unreachable!("fixed case"),
        }
        assert!(transport.requests.lock().expect("request log").is_empty());
    }
}

#[derive(Clone, Debug, Default)]
struct FailingTransport {
    attempts: Arc<AtomicUsize>,
}

#[async_trait]
impl LezBridgeTransport for FailingTransport {
    type Error = FakeError;

    async fn prepare_native_escrow(
        &self,
        _request: PrepareNativeEscrowRequest,
    ) -> Result<PrepareNativeEscrowResult, Self::Error> {
        self.attempts.fetch_add(1, Ordering::SeqCst);
        Err(FakeError)
    }
}

#[tokio::test]
async fn unknown_transport_outcome_is_not_retried() {
    let agreement = agreement();
    let transport = FailingTransport::default();
    let adapter = LezBridgeAdapter::new(
        transport.clone(),
        RunId::new("native-run-0001").expect("run id"),
        runtime(&agreement),
        Participant::Taker,
    )
    .expect("matching actor sidecar");

    let error = adapter
        .prepare_native_first_lock(
            &agreement,
            RequestId::new("unknown-outcome").expect("request id"),
        )
        .await
        .expect_err("transport delivery is unknown");
    assert!(matches!(
        error,
        PrepareNativeFirstLockError::Transport(FakeError)
    ));
    assert_eq!(transport.attempts.load(Ordering::SeqCst), 1);
}

#[derive(Clone, Copy, Debug)]
struct WrongContextTransport;

#[async_trait]
impl LezBridgeTransport for WrongContextTransport {
    type Error = FakeError;

    async fn prepare_native_escrow(
        &self,
        request: PrepareNativeEscrowRequest,
    ) -> Result<PrepareNativeEscrowResult, Self::Error> {
        let mut context = request.context;
        context.request_id = RequestId::new("wrong-response").expect("request id");
        Ok(prepared_response(context))
    }
}

#[tokio::test]
async fn prepared_bytes_with_a_different_context_are_rejected() {
    let agreement = agreement();
    let adapter = LezBridgeAdapter::new(
        WrongContextTransport,
        RunId::new("native-run-0001").expect("run id"),
        runtime(&agreement),
        Participant::Taker,
    )
    .expect("matching actor sidecar");

    let error = adapter
        .prepare_native_first_lock(
            &agreement,
            RequestId::new("expected-response").expect("request id"),
        )
        .await
        .expect_err("response context is exact");
    assert!(matches!(
        error,
        PrepareNativeFirstLockError::ResponseContextMismatch
    ));
}

async fn assert_preparation_rejected(
    agreement: &ZecAgreementV1,
    runtime: RuntimeDescriptor,
    expected: PrepareNativeFirstLockError<FakeError>,
    request_id: &str,
) {
    let transport = FakeTransport::default();
    let adapter = LezBridgeAdapter::new(
        transport.clone(),
        RunId::new("native-run-0001").expect("run id"),
        runtime,
        Participant::Taker,
    )
    .expect("matching actor sidecar");
    let actual = adapter
        .prepare_native_first_lock(agreement, RequestId::new(request_id).expect("request id"))
        .await
        .expect_err("runtime mismatch fails closed");
    assert_eq!(
        std::mem::discriminant(&actual),
        std::mem::discriminant(&expected)
    );
    assert!(transport.requests.lock().expect("request log").is_empty());
}

fn adapter(
    transport: FakeTransport,
    agreement: &ZecAgreementV1,
) -> LezBridgeAdapter<FakeTransport> {
    LezBridgeAdapter::new(
        transport,
        RunId::new("native-run-0001").expect("run id"),
        runtime(agreement),
        Participant::Taker,
    )
    .expect("matching actor sidecar")
}

fn runtime(agreement: &ZecAgreementV1) -> RuntimeDescriptor {
    RuntimeDescriptor::new(
        BridgeParticipant::Taker,
        RuntimeCompatibility::NssaV0_1_2,
        Hex32::from_bytes([6; 32]),
        Hex32::from_bytes(*agreement.lez_terms().chain().channel_id()),
        Hex32::from_bytes(*agreement.lez_terms().chain().genesis_block_hash()),
        Hex32::from_bytes(program_bytes(agreement.lez_terms().escrow_program_id())),
        Hex32::from_bytes(*agreement.lez_account(Participant::Taker)),
    )
}

fn observation_adapter(
    transport: ObservationTransport,
    agreement: &ZecAgreementV1,
    participant: Participant,
) -> LezBridgeAdapter<ObservationTransport> {
    let mut descriptor = runtime(agreement);
    descriptor.sidecar_role = match participant {
        Participant::Maker => BridgeParticipant::Maker,
        Participant::Taker => BridgeParticipant::Taker,
    };
    descriptor.signer_account_id = Hex32::from_bytes(*agreement.lez_account(participant));
    LezBridgeAdapter::new(
        transport,
        RunId::new("native-run-0001").expect("run id"),
        descriptor,
        participant,
    )
    .expect("matching actor sidecar")
}

fn observation_context(participant: Participant, request_id: &str) -> MessageContext {
    MessageContext::new(
        RunId::new("native-run-0001").expect("run id"),
        RequestId::new(request_id).expect("request id"),
        match participant {
            Participant::Maker => BridgeParticipant::Maker,
            Participant::Taker => BridgeParticipant::Taker,
        },
    )
}

const fn exact_target() -> EscrowObservationTarget {
    EscrowObservationTarget::Exact {
        initialization_transaction_id: TransactionId::from_bytes([0x11; 32]),
        funding_transaction_id: TransactionId::from_bytes([0x22; 32]),
    }
}

#[derive(Clone, Copy, Debug)]
enum RuntimeMutation {
    Channel,
    Genesis,
    Program,
    Signer,
}

#[derive(Clone, Copy, Debug)]
enum ObservationMutation {
    ResponseContext,
    TipHash,
    TipHeight,
    InitializationId,
    FundingId,
    DuplicateId,
    DuplicateBytes,
    InitializationNonPublic,
    FundingNonPublic,
    InitializationSigner,
    FundingSigner,
    InitializationProgram,
    InitializationAccounts,
    InitializationTerms,
    FundingProgram,
    FundingAccounts,
    FundingSwapId,
    MetadataAccount,
    MetadataOwner,
    MetadataVersion,
    MetadataSwapId,
    MetadataTermsHash,
    MetadataSecretDigest,
    MetadataDepositor,
    MetadataDepositorAsset,
    MetadataClaimant,
    MetadataClaimantAsset,
    MetadataCustody,
    MetadataAssetProgram,
    MetadataCustodyProgram,
    MetadataDefinition,
    MetadataAmount,
    MetadataRefundAt,
    MetadataStatus,
    InitializationMetadataDiffers,
    CustodyAccount,
    CustodyOwner,
    CustodyBalance,
    PositionOrder,
    FundingAboveTip,
    SameHeightDifferentBlock,
}

const ALL_OBSERVATION_MUTATIONS: [ObservationMutation; 41] = [
    ObservationMutation::ResponseContext,
    ObservationMutation::TipHash,
    ObservationMutation::TipHeight,
    ObservationMutation::InitializationId,
    ObservationMutation::FundingId,
    ObservationMutation::DuplicateId,
    ObservationMutation::DuplicateBytes,
    ObservationMutation::InitializationNonPublic,
    ObservationMutation::FundingNonPublic,
    ObservationMutation::InitializationSigner,
    ObservationMutation::FundingSigner,
    ObservationMutation::InitializationProgram,
    ObservationMutation::InitializationAccounts,
    ObservationMutation::InitializationTerms,
    ObservationMutation::FundingProgram,
    ObservationMutation::FundingAccounts,
    ObservationMutation::FundingSwapId,
    ObservationMutation::MetadataAccount,
    ObservationMutation::MetadataOwner,
    ObservationMutation::MetadataVersion,
    ObservationMutation::MetadataSwapId,
    ObservationMutation::MetadataTermsHash,
    ObservationMutation::MetadataSecretDigest,
    ObservationMutation::MetadataDepositor,
    ObservationMutation::MetadataDepositorAsset,
    ObservationMutation::MetadataClaimant,
    ObservationMutation::MetadataClaimantAsset,
    ObservationMutation::MetadataCustody,
    ObservationMutation::MetadataAssetProgram,
    ObservationMutation::MetadataCustodyProgram,
    ObservationMutation::MetadataDefinition,
    ObservationMutation::MetadataAmount,
    ObservationMutation::MetadataRefundAt,
    ObservationMutation::MetadataStatus,
    ObservationMutation::InitializationMetadataDiffers,
    ObservationMutation::CustodyAccount,
    ObservationMutation::CustodyOwner,
    ObservationMutation::CustodyBalance,
    ObservationMutation::PositionOrder,
    ObservationMutation::FundingAboveTip,
    ObservationMutation::SameHeightDifferentBlock,
];

#[allow(clippy::too_many_lines)]
fn mutate_observation(
    agreement: &ZecAgreementV1,
    response: &mut ObserveEscrowResult,
    mutation: ObservationMutation,
) {
    let InitializationObservation::Found(initialization) = &mut response.initialization else {
        panic!("canonical fixture has initialization")
    };
    let FundingObservation::Found(funding) = &mut response.funding else {
        panic!("canonical fixture has funding")
    };
    match mutation {
        ObservationMutation::ResponseContext => {
            response.context.request_id = RequestId::new("wrong-context").expect("request id");
        }
        ObservationMutation::TipHash => {
            response.tip_after.block_hash = Hex32::from_bytes([0x91; 32]);
        }
        ObservationMutation::TipHeight => response.tip_after.height += 1,
        ObservationMutation::InitializationId => {
            initialization.transaction.transaction_id = TransactionId::from_bytes([0x31; 32]);
        }
        ObservationMutation::FundingId => {
            funding.transaction.transaction_id = TransactionId::from_bytes([0x32; 32]);
        }
        ObservationMutation::DuplicateId => {
            funding.transaction.transaction_id = initialization.transaction.transaction_id;
        }
        ObservationMutation::DuplicateBytes => {
            funding.transaction.exact_bytes = initialization.transaction.exact_bytes.clone();
        }
        ObservationMutation::InitializationNonPublic => {
            initialization.transaction.is_public = false;
        }
        ObservationMutation::FundingNonPublic => funding.transaction.is_public = false,
        ObservationMutation::InitializationSigner => {
            initialization.transaction.signer_account_ids =
                AccountIds::new(vec![Hex32::from_bytes([0x41; 32])]).expect("signer");
        }
        ObservationMutation::FundingSigner => {
            funding.transaction.signer_account_ids =
                AccountIds::new(vec![Hex32::from_bytes([0x42; 32])]).expect("signer");
        }
        ObservationMutation::InitializationProgram => {
            initialization.instruction.program_id = Hex32::from_bytes([0x43; 32]);
        }
        ObservationMutation::InitializationAccounts => {
            initialization.instruction.ordered_account_ids =
                AccountIds::new(vec![Hex32::from_bytes(
                    *agreement.lez_terms().custody_account(),
                )])
                .expect("accounts");
        }
        ObservationMutation::InitializationTerms => {
            initialization.instruction.terms = changed_native_terms(agreement);
        }
        ObservationMutation::FundingProgram => {
            funding.instruction.program_id = Hex32::from_bytes([0x44; 32]);
        }
        ObservationMutation::FundingAccounts => {
            funding.instruction.ordered_account_ids = AccountIds::new(vec![Hex32::from_bytes(
                *agreement.lez_terms().metadata_account(),
            )])
            .expect("accounts");
        }
        ObservationMutation::FundingSwapId => {
            funding.instruction.swap_id = Hex32::from_bytes([0x45; 32]);
        }
        ObservationMutation::MetadataAccount => {
            funding.metadata.account_id = Hex32::from_bytes([0x46; 32]);
        }
        ObservationMutation::MetadataOwner => {
            funding.metadata.owner_program_id = Hex32::from_bytes([0x47; 32]);
        }
        ObservationMutation::MetadataVersion => funding.metadata.version += 1,
        ObservationMutation::MetadataSwapId => {
            funding.metadata.swap_id = Hex32::from_bytes([0x48; 32]);
        }
        ObservationMutation::MetadataTermsHash => {
            funding.metadata.terms_hash = Hex32::from_bytes([0x49; 32]);
        }
        ObservationMutation::MetadataSecretDigest => {
            funding.metadata.secret_digest = Hex32::from_bytes([0x4a; 32]);
        }
        ObservationMutation::MetadataDepositor => {
            funding.metadata.depositor_account_id = Hex32::from_bytes([0x4b; 32]);
        }
        ObservationMutation::MetadataDepositorAsset => {
            funding.metadata.depositor_asset_account_id = Hex32::from_bytes([0x4c; 32]);
        }
        ObservationMutation::MetadataClaimant => {
            funding.metadata.claimant_account_id = Hex32::from_bytes([0x4d; 32]);
        }
        ObservationMutation::MetadataClaimantAsset => {
            funding.metadata.claimant_asset_account_id = Hex32::from_bytes([0x4e; 32]);
        }
        ObservationMutation::MetadataCustody => {
            funding.metadata.custody_account_id = Hex32::from_bytes([0x4f; 32]);
        }
        ObservationMutation::MetadataAssetProgram => {
            funding.metadata.asset_program_id = Hex32::from_bytes([0x50; 32]);
        }
        ObservationMutation::MetadataCustodyProgram => {
            funding.metadata.custody_program_id = Hex32::from_bytes([0x51; 32]);
        }
        ObservationMutation::MetadataDefinition => {
            funding.metadata.asset_definition = Hex32::from_bytes([0x52; 32]);
        }
        ObservationMutation::MetadataAmount => {
            funding.metadata.amount = lez_bridge_protocol::NativeAmount::new(43);
        }
        ObservationMutation::MetadataRefundAt => funding.metadata.refund_at_ms += 1,
        ObservationMutation::MetadataStatus => funding.metadata.status = EscrowState::Claimed,
        ObservationMutation::InitializationMetadataDiffers => {
            initialization.metadata.status = EscrowState::Empty;
        }
        ObservationMutation::CustodyAccount => {
            funding.custody.account_id = Hex32::from_bytes([0x53; 32]);
        }
        ObservationMutation::CustodyOwner => {
            funding.custody.owner_program_id = Hex32::from_bytes([0x54; 32]);
        }
        ObservationMutation::CustodyBalance => {
            funding.custody.balance = lez_bridge_protocol::NativeAmount::new(41);
        }
        ObservationMutation::PositionOrder => {
            initialization.transaction.position.height = funding.transaction.position.height;
            initialization.transaction.position.transaction_index =
                funding.transaction.position.transaction_index;
        }
        ObservationMutation::FundingAboveTip => {
            funding.transaction.position.height = response.tip_after.height + 1;
        }
        ObservationMutation::SameHeightDifferentBlock => {
            initialization.transaction.position.height = funding.transaction.position.height;
            initialization.transaction.position.transaction_index = 0;
            funding.transaction.position.transaction_index = 1;
        }
    }
}

fn changed_native_terms(agreement: &ZecAgreementV1) -> NativeEscrowTerms {
    let terms = native_terms(agreement);
    NativeEscrowTerms::new(NativeEscrowTermsInput {
        swap_id: terms.swap_id(),
        terms_hash: terms.terms_hash(),
        secret_digest: terms.secret_digest(),
        depositor: terms.depositor(),
        depositor_account_id: terms.depositor_account_id(),
        claimant: terms.claimant(),
        claimant_account_id: terms.claimant_account_id(),
        amount: terms.amount().as_u128() + 1,
        refund_at_ms: terms.refund_at_ms(),
        authenticated_transfer_program_id: terms.authenticated_transfer_program_id(),
    })
    .expect("independently valid changed terms")
}

fn native_terms(agreement: &ZecAgreementV1) -> NativeEscrowTerms {
    let LezAssetV1::Native {
        authenticated_transfer_program_id,
    } = agreement.lez_terms().asset()
    else {
        panic!("native fixture")
    };
    NativeEscrowTerms::new(NativeEscrowTermsInput {
        swap_id: Hex32::from_bytes(*agreement.onchain_swap_id()),
        terms_hash: Hex32::from_bytes(*agreement.agreement_commitment()),
        secret_digest: Hex32::from_bytes(*agreement.secret_digest()),
        depositor: BridgeParticipant::Taker,
        depositor_account_id: Hex32::from_bytes(*agreement.lez_account(Participant::Taker)),
        claimant: BridgeParticipant::Maker,
        claimant_account_id: Hex32::from_bytes(*agreement.lez_account(Participant::Maker)),
        amount: agreement.lez_terms().amount(),
        refund_at_ms: agreement.lez_refund_at_ms(),
        authenticated_transfer_program_id: Hex32::from_bytes(program_bytes(
            authenticated_transfer_program_id,
        )),
    })
    .expect("valid native terms")
}

fn canonical_observation(
    agreement: &ZecAgreementV1,
    context: MessageContext,
) -> ObserveEscrowResult {
    let terms = native_terms(agreement);
    let escrow_program =
        Hex32::from_bytes(program_bytes(agreement.lez_terms().escrow_program_id()));
    let metadata_account = Hex32::from_bytes(*agreement.lez_terms().metadata_account());
    let custody_account = Hex32::from_bytes(*agreement.lez_terms().custody_account());
    let depositor = Hex32::from_bytes(*agreement.lez_account(Participant::Taker));
    let claimant = Hex32::from_bytes(*agreement.lez_account(Participant::Maker));
    let signers = AccountIds::new(vec![depositor]).expect("signer list");
    let metadata = EscrowMetadataFacts::from_native_terms(
        metadata_account,
        escrow_program,
        custody_account,
        &terms,
        EscrowState::Funded,
    );
    let initialization = InitializationFoundFacts::new(
        ObservedTransactionFacts::new(
            TransactionId::from_bytes([0x11; 32]),
            ExactTransactionBytes::new(vec![0xa1, 0xb1]).expect("init bytes"),
            ChainPosition::new(Hex32::from_bytes([0x81; 32]), 10, 1),
            signers.clone(),
            true,
        ),
        NativeInitializeInstructionFacts::new(
            escrow_program,
            AccountIds::new(vec![metadata_account, custody_account, depositor, claimant])
                .expect("init accounts"),
            terms.clone(),
        ),
        metadata.clone(),
    );
    let funding = FundingFoundFacts::new(
        ObservedTransactionFacts::new(
            TransactionId::from_bytes([0x22; 32]),
            ExactTransactionBytes::new(vec![0xa2, 0xb2]).expect("fund bytes"),
            ChainPosition::new(Hex32::from_bytes([0x82; 32]), 11, 0),
            signers,
            true,
        ),
        NativeFundInstructionFacts::new(
            escrow_program,
            AccountIds::new(vec![metadata_account, custody_account, depositor])
                .expect("fund accounts"),
            terms.swap_id(),
        ),
        metadata,
        NativeCustodyFacts::new(
            custody_account,
            terms.authenticated_transfer_program_id(),
            terms.amount().as_u128(),
        ),
    );
    let tip = ChainTip::new(Hex32::from_bytes([0x90; 32]), 12);
    ObserveEscrowResult::new(
        context,
        tip,
        InitializationObservation::found(initialization),
        FundingObservation::found(funding),
        tip,
    )
}

fn agreement() -> ZecAgreementV1 {
    agreement_for(
        LezEnvironmentV1::DeterministicLocalV0_1_2Compatibility,
        false,
    )
}

#[allow(clippy::too_many_lines)]
fn agreement_for(environment: LezEnvironmentV1, token: bool) -> ZecAgreementV1 {
    let maker_secret = SecretKey::from_slice(&[1; 32]).expect("maker key");
    let taker_secret = SecretKey::from_slice(&[2; 32]).expect("taker key");
    let secp = Secp256k1::new();
    let maker_key = PublicKey::from_secret_key(&secp, &maker_secret).serialize();
    let taker_key = PublicKey::from_secret_key(&secp, &taker_secret).serialize();
    let refund_hash = pubkey_hash(&maker_key);
    let claimant_hash = pubkey_hash(&taker_key);
    let secret_digest: [u8; 32] = Sha256::digest([0x91; 32]).into();
    let contract = Bip199Contract::new(120, refund_hash, secret_digest, claimant_hash);
    let binding = ZecSwapBinding::new(
        ZecProfileId::DeterministicLocalV1,
        ExpectedBip199Output::new(
            NetworkType::Regtest,
            BranchId::Nu6_2,
            Zatoshis::from_u64(100_000_000).expect("principal"),
            contract,
        ),
    )
    .expect("profile binding");
    let id = match (environment, token) {
        (LezEnvironmentV1::DeterministicLocalV0_1_2Compatibility, false) => {
            "lez-bridge-native-test"
        }
        (LezEnvironmentV1::DeterministicLocalV0_1_2Compatibility, true) => "lez-bridge-token-test",
        (LezEnvironmentV1::DeterministicLocalV0_2, false) => "lez-bridge-v02-test",
        (LezEnvironmentV1::PublicTestnetV0_2, _)
        | (LezEnvironmentV1::DeterministicLocalV0_2, true) => {
            unreachable!("test fixtures cover supported deterministic combinations")
        }
    };
    let escrow_program = [1; 8];
    let onchain_id = derive_lez_swap_id_v1(id.as_bytes());
    let metadata = if environment == LezEnvironmentV1::DeterministicLocalV0_1_2Compatibility {
        derive_nssa_v0_1_2_metadata_account_v1(&escrow_program, &onchain_id)
    } else {
        derive_lez_metadata_account_v1(&escrow_program, &onchain_id)
    };
    let (asset, custody) = if token {
        let definition_account = [9; 32];
        let token_program_id = [3; 8];
        let ata_program_id = [4; 8];
        (
            LezAssetV1::FungibleToken {
                definition_account,
                token_program_id,
                ata_program_id,
                depositor_ata: derive_nssa_v0_1_2_token_account_v1(
                    &ata_program_id,
                    &[4; 32],
                    &definition_account,
                ),
                claimant_ata: derive_nssa_v0_1_2_token_account_v1(
                    &ata_program_id,
                    &[3; 32],
                    &definition_account,
                ),
            },
            derive_nssa_v0_1_2_token_account_v1(&ata_program_id, &metadata, &definition_account),
        )
    } else {
        (
            LezAssetV1::Native {
                authenticated_transfer_program_id: [2; 8],
            },
            if environment == LezEnvironmentV1::DeterministicLocalV0_1_2Compatibility {
                derive_nssa_v0_1_2_native_custody_account_v1(&escrow_program, &onchain_id)
            } else {
                derive_lez_native_custody_account_v1(&escrow_program, &onchain_id)
            },
        )
    };
    let body = ZecAgreementBodyV1::new(
        id,
        SwapDirection::TakerSellsLez,
        ZecProfileRecordV1::from(ZecProfileId::DeterministicLocalV1),
        ZecParticipantsV1::new(
            ZecParticipantIdentityV1::new([3; 32], maker_key),
            ZecParticipantIdentityV1::new([4; 32], taker_key),
        ),
        secret_digest,
        ZecLezTermsV1::new(
            LezChainIdentityV1::new(environment, [8; 32], [7; 32]),
            escrow_program,
            asset,
            42,
            metadata,
            custody,
        ),
        ZecSwapBindingRecordV1::from_binding(&binding),
        ZecTransactionPolicyV1::new(
            [12; 32],
            ZcashTransparentDestinationV1::p2pkh(refund_hash),
            10_000,
            1_000,
            ZcashTransparentDestinationV1::p2pkh(claimant_hash),
            10_000,
            ZcashTransparentDestinationV1::p2pkh(refund_hash),
            10_000,
            40,
        ),
        ZecRefundPlanV1::new(100, 116, 160_000, 200),
        NegotiationTranscriptV1::new([5; 32], [6; 32], 1_000),
    );
    let commitment = body.commitment();
    let record = ZecAgreementRecordV1::from_parts(
        ZEC_CONCRETE_AGREEMENT_SCHEMA_V2,
        body,
        commitment,
        secp.sign_ecdsa(&Message::from_digest(commitment), &maker_secret)
            .serialize_compact(),
        secp.sign_ecdsa(&Message::from_digest(commitment), &taker_secret)
            .serialize_compact(),
    );
    ZecAgreementV1::from_wire_at(
        &record.encode_wire().expect("bounded agreement"),
        UnixSeconds::new(10),
    )
    .expect("valid agreement")
}

fn prepared_response(context: lez_bridge_protocol::MessageContext) -> PrepareNativeEscrowResult {
    PrepareNativeEscrowResult::new(
        context,
        PreparedTransaction::new(
            TransactionId::from_bytes([0x11; 32]),
            ExactTransactionBytes::new(vec![0xaa, 0xbb]).expect("initialize bytes"),
        ),
        PreparedTransaction::new(
            TransactionId::from_bytes([0x22; 32]),
            ExactTransactionBytes::new(vec![0xcc, 0xdd]).expect("fund bytes"),
        ),
    )
}

fn pubkey_hash(bytes: &[u8; 33]) -> [u8; 20] {
    match TransparentAddress::from_pubkey(&PublicKey::from_slice(bytes).expect("public key")) {
        TransparentAddress::PublicKeyHash(hash) => hash,
        TransparentAddress::ScriptHash(_) => unreachable!("public keys produce P2PKH"),
    }
}

fn program_bytes(words: &[u32; 8]) -> [u8; 32] {
    let mut bytes = [0_u8; 32];
    for (chunk, word) in bytes.chunks_exact_mut(4).zip(words) {
        chunk.copy_from_slice(&word.to_le_bytes());
    }
    bytes
}
