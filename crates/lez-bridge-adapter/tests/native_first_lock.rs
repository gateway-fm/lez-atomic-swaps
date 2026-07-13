use std::{
    collections::VecDeque,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

use async_trait::async_trait;
use lez_bridge_adapter::{
    BridgeRequestContextSource, CanonicalLezFundingSource, ContextOwningLezBridgePorts,
    ContextOwningLezPortError, FreshLezBridgeTransportFactory, LezBridgeAdapter,
    LezBridgeClaimTransport, LezBridgeFirstLockTransport, LezBridgeObservationTransport,
    LezBridgeRefundTransport, LezBridgeTransport, NativeFirstLockSubmitOutcome,
    NativeRefundAdapterError, NativeRevealingClaimAdapterError, ObserveNativeEscrowError,
    PrepareNativeFirstLockError, RevealingClaimSubmitOutcome,
};
use lez_bridge_protocol::{
    AccountIds, ChainClock, ChainPosition, ChainTip, DiscoveryWindow, EscrowMetadataFacts,
    EscrowObservationTarget, EscrowState, ExactTransactionBytes, FundingFoundFacts,
    FundingObservation, Hex32, InitializationFoundFacts, InitializationObservation, MessageContext,
    NativeAmount, NativeClaimInstructionFacts, NativeCustodyFacts, NativeEscrowAccountFacts,
    NativeEscrowAccountObservation, NativeEscrowTerms, NativeEscrowTermsInput,
    NativeFundInstructionFacts, NativeInitializeInstructionFacts, NativeRefundFoundFacts,
    NativeRefundInstructionFacts, NativeRefundObservation, NativeRefundObservationTarget,
    ObserveEscrowRequest, ObserveEscrowResult, ObserveNativeRefundRequest,
    ObserveNativeRefundResult, ObserveRevealingClaimRequest, ObserveRevealingClaimResult,
    ObservedTransactionFacts, Participant as BridgeParticipant, PrepareNativeEscrowRequest,
    PrepareNativeEscrowResult, PrepareNativeRefundRequest, PrepareNativeRefundResult,
    PrepareRevealingClaimRequest, PrepareRevealingClaimResult, PreparedTransaction, RequestId,
    RevealingClaimFoundFacts, RevealingClaimObservation, RevealingClaimObservationTarget,
    RevealingPreimage, RunId, RuntimeCompatibility, RuntimeDescriptor, SubmissionOutcome,
    SubmitTransactionRequest, SubmitTransactionResult, TransactionId,
};
use lez_swap_core::{Chain, LezUnixMilliseconds, Participant, SwapDirection, UnixSeconds};
use lez_swap_store::{
    BridgeOperationKey, BridgeRequestSpec, SqliteBridgeOperationJournal, SqliteZecRecoveryStore,
};
use lez_zec_swap_sdk::{
    AcceptedZecAgreementV1, Bip199Contract, ClaimPreimage, ClaimStepV1, ExpectedBip199Output,
    FirstLockConfirmedEvidenceV1, FirstLockObservation, FirstLockPlanV1, FirstLockStepV1,
    LezAssetV1, LezChainIdentityV1, LezClaimPort, LezEnvironmentV1, LezFirstLockPort,
    LezRefundPort, NegotiationChannel, NegotiationTranscriptV1, OfferDiscovery,
    PreparedClaimSubmissionV1, PreparedFirstLockSubmissionV1, PreparedRefundSubmissionV1,
    RefundEligibilityObservationV1, RefundError, RefundEvidenceV1, RefundFundingWaitReasonV1,
    RefundObservationV1, RefundStepV1, RefundSubmitOutcomeV1, RevealingClaimObservationV1,
    TakerFirstLockObservationV1, ZEC_CONCRETE_AGREEMENT_SCHEMA_V2, ZcashTransparentDestinationV1,
    ZecAgreementBodyV1, ZecAgreementRecordV1, ZecAgreementV1, ZecLezTermsV1, ZecPairSdk,
    ZecParticipantIdentityV1, ZecParticipantsV1, ZecProfileId, ZecProfileRecordV1, ZecRefundPlanV1,
    ZecSwapBinding, ZecSwapBindingRecordV1, ZecTransactionPolicyV1, derive_lez_metadata_account_v1,
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

static CONTEXT_DB_SEQUENCE: AtomicUsize = AtomicUsize::new(0);

#[derive(Clone, Debug)]
struct CloneTransportFactory<T> {
    transport: T,
    opens: Arc<AtomicUsize>,
}

impl<T> FreshLezBridgeTransportFactory for CloneTransportFactory<T>
where
    T: Clone + Send + Sync,
{
    type Transport = T;
    type Error = FakeError;

    fn fresh_transport(&self) -> Result<Self::Transport, Self::Error> {
        self.opens.fetch_add(1, Ordering::SeqCst);
        Ok(self.transport.clone())
    }
}

#[derive(Clone, Copy, Debug)]
struct SeedDiscovery;

#[async_trait]
impl OfferDiscovery for SeedDiscovery {
    type Error = FakeError;
    type Offer = ();
    type OfferRef = ();
    type Query = ();

    async fn publish(&self, _offer: Self::Offer) -> Result<Self::OfferRef, Self::Error> {
        Ok(())
    }

    async fn discover(&self, _query: &Self::Query) -> Result<Vec<Self::OfferRef>, Self::Error> {
        Ok(Vec::new())
    }
}

#[derive(Clone, Copy, Debug)]
struct SeedNegotiation;

#[async_trait]
impl NegotiationChannel for SeedNegotiation {
    type Error = FakeError;
    type LocalProposal = ();
    type OfferRef = ();

    async fn negotiate(
        &self,
        _local_participant: Participant,
        _offer: &Self::OfferRef,
        _proposal: Self::LocalProposal,
    ) -> Result<Vec<u8>, Self::Error> {
        Err(FakeError)
    }
}

#[derive(Clone, Debug)]
struct FixedCanonicalFunding(FirstLockConfirmedEvidenceV1);

#[async_trait]
impl CanonicalLezFundingSource for FixedCanonicalFunding {
    type Error = FakeError;

    async fn canonical_lez_funding(
        &self,
        _agreement: &ZecAgreementV1,
    ) -> Result<FirstLockConfirmedEvidenceV1, Self::Error> {
        Ok(self.0.clone())
    }
}

#[derive(Clone, Debug)]
struct ContextPrepareTransport {
    remaining_failures: Arc<AtomicUsize>,
    requests: Arc<Mutex<Vec<PrepareNativeEscrowRequest>>>,
}

#[async_trait]
impl LezBridgeTransport for ContextPrepareTransport {
    type Error = FakeError;

    async fn prepare_native_escrow(
        &self,
        request: PrepareNativeEscrowRequest,
    ) -> Result<PrepareNativeEscrowResult, Self::Error> {
        self.requests
            .lock()
            .expect("context prepare requests")
            .push(request.clone());
        if self
            .remaining_failures
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |remaining| {
                remaining.checked_sub(1)
            })
            .is_ok()
        {
            return Err(FakeError);
        }
        Ok(prepared_response(request.context))
    }
}

#[derive(Clone, Debug)]
struct ContextPrepareFactory {
    transport: ContextPrepareTransport,
    opens: Arc<AtomicUsize>,
    remaining_open_failures: Arc<AtomicUsize>,
}

impl FreshLezBridgeTransportFactory for ContextPrepareFactory {
    type Transport = ContextPrepareTransport;
    type Error = FakeError;

    fn fresh_transport(&self) -> Result<Self::Transport, Self::Error> {
        self.opens.fetch_add(1, Ordering::SeqCst);
        if self
            .remaining_open_failures
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |remaining| {
                remaining.checked_sub(1)
            })
            .is_ok()
        {
            return Err(FakeError);
        }
        Ok(self.transport.clone())
    }
}

#[derive(Debug)]
struct QueuedContexts {
    requests: Arc<Mutex<VecDeque<BridgeRequestSpec>>>,
    calls: Arc<AtomicUsize>,
}

impl BridgeRequestContextSource for QueuedContexts {
    type Error = FakeError;

    fn next_request(&self, _key: &BridgeOperationKey) -> Result<BridgeRequestSpec, Self::Error> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.requests
            .lock()
            .expect("context requests")
            .pop_front()
            .ok_or(FakeError)
    }
}

fn isolated_sqlite_path(prefix: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "{prefix}-{}-{}.sqlite",
        std::process::id(),
        CONTEXT_DB_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ))
}

fn remove_sqlite_files(path: &std::path::Path) {
    let _ = std::fs::remove_file(path);
    let _ = std::fs::remove_file(path.with_extension("sqlite-wal"));
    let _ = std::fs::remove_file(path.with_extension("sqlite-shm"));
}

async fn stage_native_first_lock_plan(
    store: &SqliteZecRecoveryStore,
    agreement: &ZecAgreementV1,
    plan: FirstLockPlanV1,
) {
    let wire = agreement.encode_wire().expect("bounded signed agreement");
    let accepted =
        AcceptedZecAgreementV1::accept_wire_at(&wire, UnixSeconds::new(10), Participant::Taker, 0)
            .expect("accepted signed agreement");
    let sdk = ZecPairSdk::new(
        Participant::Taker,
        SeedDiscovery,
        SeedNegotiation,
        (),
        (),
        store.clone(),
    );
    let active = sdk.activate(accepted).await.expect("persist agreement");
    active
        .stage_first_lock(plan)
        .await
        .expect("persist full LEZ first-lock plan");
}

fn runtime_for_participant(
    agreement: &ZecAgreementV1,
    participant: Participant,
) -> RuntimeDescriptor {
    let mut descriptor = runtime(agreement);
    descriptor.sidecar_role = match participant {
        Participant::Maker => BridgeParticipant::Maker,
        Participant::Taker => BridgeParticipant::Taker,
    };
    descriptor.signer_account_id = Hex32::from_bytes(*agreement.lez_account(participant));
    descriptor
}

#[tokio::test]
async fn ambiguous_prepare_reopens_the_exact_context_with_a_fresh_transport() {
    let agreement = agreement();
    let path = std::env::temp_dir().join(format!(
        "lez-bridge-context-{}-{}.sqlite",
        std::process::id(),
        CONTEXT_DB_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    let requests = Arc::new(Mutex::new(Vec::new()));
    let opens = Arc::new(AtomicUsize::new(0));
    let factory = ContextPrepareFactory {
        transport: ContextPrepareTransport {
            remaining_failures: Arc::new(AtomicUsize::new(1)),
            requests: Arc::clone(&requests),
        },
        opens: Arc::clone(&opens),
        remaining_open_failures: Arc::new(AtomicUsize::new(0)),
    };
    let allocation_calls = Arc::new(AtomicUsize::new(0));
    let caller_request = BridgeRequestSpec::new(
        RequestId::new("context-owned-prepare-0001").expect("request id"),
        None,
    );
    let first_contexts = QueuedContexts {
        requests: Arc::new(Mutex::new(VecDeque::from([caller_request]))),
        calls: Arc::clone(&allocation_calls),
    };
    let first = ContextOwningLezBridgePorts::new(
        RunId::new("context-owned-run-0001").expect("run id"),
        runtime(&agreement),
        Participant::Taker,
        factory.clone(),
        first_contexts,
        (),
        (),
        SqliteBridgeOperationJournal::open(&path).expect("operation journal"),
    )
    .expect("role-local composition");

    assert!(first.prepare_native_first_lock(&agreement).await.is_err());
    assert_eq!(allocation_calls.load(Ordering::SeqCst), 1);
    assert_eq!(opens.load(Ordering::SeqCst), 1);
    drop(first);

    let resumed = ContextOwningLezBridgePorts::new(
        RunId::new("context-owned-run-0001").expect("run id"),
        runtime(&agreement),
        Participant::Taker,
        factory,
        QueuedContexts {
            requests: Arc::new(Mutex::new(VecDeque::new())),
            calls: Arc::clone(&allocation_calls),
        },
        (),
        (),
        SqliteBridgeOperationJournal::open(&path).expect("reopened operation journal"),
    )
    .expect("restarted role-local composition");
    let plan = resumed
        .prepare_native_first_lock(&agreement)
        .await
        .expect("exact context succeeds through a fresh client");
    assert!(matches!(plan, FirstLockPlanV1::Lez { .. }));
    assert_eq!(allocation_calls.load(Ordering::SeqCst), 1);
    assert_eq!(opens.load(Ordering::SeqCst), 2);
    let requests = requests.lock().expect("prepare request log");
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[0].context.request_id,
        requests[1].context.request_id
    );
    drop(requests);
    drop(resumed);
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(path.with_extension("sqlite-wal"));
    let _ = std::fs::remove_file(path.with_extension("sqlite-shm"));
}

#[tokio::test]
async fn factory_failure_after_reserve_reopens_exact_context_without_a_sidecar_call() {
    let agreement = agreement();
    let path = std::env::temp_dir().join(format!(
        "lez-bridge-pre-call-context-{}-{}.sqlite",
        std::process::id(),
        CONTEXT_DB_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    let requests = Arc::new(Mutex::new(Vec::new()));
    let opens = Arc::new(AtomicUsize::new(0));
    let factory = ContextPrepareFactory {
        transport: ContextPrepareTransport {
            remaining_failures: Arc::new(AtomicUsize::new(0)),
            requests: Arc::clone(&requests),
        },
        opens: Arc::clone(&opens),
        remaining_open_failures: Arc::new(AtomicUsize::new(1)),
    };
    let allocation_calls = Arc::new(AtomicUsize::new(0));
    let caller_request = BridgeRequestSpec::new(
        RequestId::new("context-owned-pre-call-prepare-0001").expect("request id"),
        None,
    );
    let caller_request_id = caller_request.request_id().clone();
    let first = ContextOwningLezBridgePorts::new(
        RunId::new("context-owned-pre-call-run-0001").expect("run id"),
        runtime(&agreement),
        Participant::Taker,
        factory.clone(),
        QueuedContexts {
            requests: Arc::new(Mutex::new(VecDeque::from([caller_request]))),
            calls: Arc::clone(&allocation_calls),
        },
        (),
        (),
        SqliteBridgeOperationJournal::open(&path).expect("operation journal"),
    )
    .expect("role-local composition");

    assert!(first.prepare_native_first_lock(&agreement).await.is_err());
    assert_eq!(allocation_calls.load(Ordering::SeqCst), 1);
    assert_eq!(opens.load(Ordering::SeqCst), 1);
    assert!(requests.lock().expect("prepare request log").is_empty());
    drop(first);

    let resumed = ContextOwningLezBridgePorts::new(
        RunId::new("context-owned-pre-call-run-0001").expect("run id"),
        runtime(&agreement),
        Participant::Taker,
        factory,
        QueuedContexts {
            requests: Arc::new(Mutex::new(VecDeque::new())),
            calls: Arc::clone(&allocation_calls),
        },
        (),
        (),
        SqliteBridgeOperationJournal::open(&path).expect("reopened operation journal"),
    )
    .expect("restarted role-local composition");
    let plan = resumed
        .prepare_native_first_lock(&agreement)
        .await
        .expect("durably reserved context succeeds after restart");

    assert!(matches!(plan, FirstLockPlanV1::Lez { .. }));
    assert_eq!(allocation_calls.load(Ordering::SeqCst), 1);
    assert_eq!(opens.load(Ordering::SeqCst), 2);
    let requests = requests.lock().expect("prepare request log");
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].context.request_id, caller_request_id);
    drop(requests);
    drop(resumed);
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(path.with_extension("sqlite-wal"));
    let _ = std::fs::remove_file(path.with_extension("sqlite-shm"));
}

#[derive(Clone, Debug)]
struct RefundTransport {
    prepare_requests: Arc<Mutex<Vec<PrepareNativeRefundRequest>>>,
    observe_requests: Arc<Mutex<Vec<ObserveNativeRefundRequest>>>,
    submit_requests: Arc<Mutex<Vec<SubmitTransactionRequest>>>,
    observations: Arc<Mutex<VecDeque<ObserveNativeRefundResult>>>,
    behavior: RefundBehavior,
}

#[derive(Clone, Copy, Debug, Default)]
enum RefundBehavior {
    #[default]
    Happy,
    FailPrepare,
    FailObserve,
    FailSubmit,
    WrongPrepareContext,
    WrongSubmitContext,
    WrongSubmitId,
    ZeroPreparedId,
}

impl RefundTransport {
    fn new(observations: impl IntoIterator<Item = ObserveNativeRefundResult>) -> Self {
        Self {
            prepare_requests: Arc::default(),
            observe_requests: Arc::default(),
            submit_requests: Arc::default(),
            observations: Arc::new(Mutex::new(observations.into_iter().collect())),
            behavior: RefundBehavior::Happy,
        }
    }

    fn with_behavior(mut self, behavior: RefundBehavior) -> Self {
        self.behavior = behavior;
        self
    }
}

#[async_trait]
impl LezBridgeRefundTransport for RefundTransport {
    type Error = FakeError;

    async fn prepare_native_refund(
        &self,
        request: PrepareNativeRefundRequest,
    ) -> Result<PrepareNativeRefundResult, Self::Error> {
        self.prepare_requests
            .lock()
            .expect("prepare request log")
            .push(request.clone());
        if matches!(self.behavior, RefundBehavior::FailPrepare) {
            return Err(FakeError);
        }
        let mut context = request.context;
        if matches!(self.behavior, RefundBehavior::WrongPrepareContext) {
            context.request_id =
                RequestId::new("wrong-refund-prepare-context").expect("request id");
        }
        let refund = if matches!(self.behavior, RefundBehavior::ZeroPreparedId) {
            PreparedTransaction::new(
                TransactionId::from_bytes([0; 32]),
                ExactTransactionBytes::new(vec![0xee, 0xff]).expect("refund bytes"),
            )
        } else {
            prepared_refund_transaction()
        };
        Ok(PrepareNativeRefundResult::new(context, refund))
    }

    async fn observe_native_refund(
        &self,
        request: ObserveNativeRefundRequest,
    ) -> Result<ObserveNativeRefundResult, Self::Error> {
        self.observe_requests
            .lock()
            .expect("observe request log")
            .push(request);
        if matches!(self.behavior, RefundBehavior::FailObserve) {
            return Err(FakeError);
        }
        self.observations
            .lock()
            .expect("observation queue")
            .pop_front()
            .ok_or(FakeError)
    }

    async fn submit_transaction(
        &self,
        request: SubmitTransactionRequest,
    ) -> Result<SubmitTransactionResult, Self::Error> {
        self.submit_requests
            .lock()
            .expect("submit request log")
            .push(request.clone());
        if matches!(self.behavior, RefundBehavior::FailSubmit) {
            return Err(FakeError);
        }
        let mut context = request.context;
        if matches!(self.behavior, RefundBehavior::WrongSubmitContext) {
            context.request_id = RequestId::new("wrong-refund-submit-context").expect("request id");
        }
        let transaction_id = if matches!(self.behavior, RefundBehavior::WrongSubmitId) {
            TransactionId::from_bytes([0x44; 32])
        } else {
            request.transaction.transaction_id
        };
        Ok(SubmitTransactionResult::new(
            context,
            transaction_id,
            SubmissionOutcome::Accepted,
        ))
    }
}

#[tokio::test]
async fn refund_unknown_reuses_the_exact_submit_context_after_sqlite_reopen() {
    let agreement = agreement();
    let prepared = prepared_refund_submission();
    let store_path = isolated_sqlite_path("lez-wrapper-refund-store");
    let journal_path = isolated_sqlite_path("lez-wrapper-refund-journal");
    let transport = RefundTransport::new([]).with_behavior(RefundBehavior::FailSubmit);
    let opens = Arc::new(AtomicUsize::new(0));
    let factory = CloneTransportFactory {
        transport: transport.clone(),
        opens: Arc::clone(&opens),
    };
    let allocation_calls = Arc::new(AtomicUsize::new(0));
    let store = SqliteZecRecoveryStore::open(&store_path, Participant::Taker)
        .expect("open production recovery store");
    let first = ContextOwningLezBridgePorts::new(
        RunId::new("native-run-0001").expect("run ID"),
        runtime_for_participant(&agreement, Participant::Taker),
        Participant::Taker,
        factory.clone(),
        QueuedContexts {
            requests: Arc::new(Mutex::new(VecDeque::from([BridgeRequestSpec::new(
                RequestId::new("wrapper-refund-submit-unknown").expect("request ID"),
                None,
            )]))),
            calls: Arc::clone(&allocation_calls),
        },
        store,
        (),
        SqliteBridgeOperationJournal::open(&journal_path).expect("operation journal"),
    )
    .expect("role-local refund wrapper");

    assert_eq!(
        first
            .submit_refund(&agreement, &prepared)
            .await
            .expect("transport ambiguity is an outcome"),
        RefundSubmitOutcomeV1::Unknown
    );
    assert_eq!(allocation_calls.load(Ordering::SeqCst), 1);
    drop(first);

    let reopened_store = SqliteZecRecoveryStore::open(&store_path, Participant::Taker)
        .expect("reopen production recovery store");
    let resumed = ContextOwningLezBridgePorts::new(
        RunId::new("native-run-0001").expect("run ID"),
        runtime_for_participant(&agreement, Participant::Taker),
        Participant::Taker,
        factory,
        QueuedContexts {
            requests: Arc::new(Mutex::new(VecDeque::new())),
            calls: Arc::clone(&allocation_calls),
        },
        reopened_store,
        (),
        SqliteBridgeOperationJournal::open(&journal_path).expect("reopened operation journal"),
    )
    .expect("restarted refund wrapper");
    assert_eq!(
        resumed
            .submit_refund(&agreement, &prepared)
            .await
            .expect("replayed ambiguity remains an outcome"),
        RefundSubmitOutcomeV1::Unknown
    );

    assert_eq!(allocation_calls.load(Ordering::SeqCst), 1);
    assert_eq!(opens.load(Ordering::SeqCst), 2);
    let submissions = transport.submit_requests.lock().expect("submit log");
    assert_eq!(submissions.len(), 2);
    assert_eq!(submissions[0].context, submissions[1].context);
    assert_eq!(
        submissions[0].context.request_id,
        RequestId::new("wrapper-refund-submit-unknown").expect("request ID")
    );
    assert_eq!(
        submissions[0].transaction.exact_bytes.as_slice(),
        prepared.exact_submission()
    );
    assert_eq!(
        submissions[1].transaction.exact_bytes.as_slice(),
        prepared.exact_submission()
    );
    drop(submissions);
    drop(resumed);
    remove_sqlite_files(&journal_path);
    remove_sqlite_files(&store_path);
}

#[derive(Clone, Debug)]
struct ClaimTransport {
    prepare_requests: Arc<Mutex<Vec<PrepareRevealingClaimRequest>>>,
    observe_requests: Arc<Mutex<Vec<ObserveRevealingClaimRequest>>>,
    submit_requests: Arc<Mutex<Vec<SubmitTransactionRequest>>>,
    observations: Arc<Mutex<VecDeque<ObserveRevealingClaimResult>>>,
    behavior: ClaimBehavior,
}

#[derive(Clone, Copy, Debug, Default)]
enum ClaimBehavior {
    #[default]
    Happy,
    FailPrepare,
    FailObserve,
    FailSubmit,
    WrongPrepareContext,
    WrongObserveContext,
    WrongSubmitContext,
    WrongSubmitId,
    ZeroPreparedId,
}

impl ClaimTransport {
    fn new(observations: impl IntoIterator<Item = ObserveRevealingClaimResult>) -> Self {
        Self {
            prepare_requests: Arc::default(),
            observe_requests: Arc::default(),
            submit_requests: Arc::default(),
            observations: Arc::new(Mutex::new(observations.into_iter().collect())),
            behavior: ClaimBehavior::Happy,
        }
    }

    fn with_behavior(mut self, behavior: ClaimBehavior) -> Self {
        self.behavior = behavior;
        self
    }
}

#[async_trait]
impl LezBridgeClaimTransport for ClaimTransport {
    type Error = FakeError;

    async fn prepare_revealing_claim(
        &self,
        request: PrepareRevealingClaimRequest,
    ) -> Result<PrepareRevealingClaimResult, Self::Error> {
        let mut context = request.context.clone();
        self.prepare_requests
            .lock()
            .expect("claim prepare log")
            .push(request);
        if matches!(self.behavior, ClaimBehavior::FailPrepare) {
            return Err(FakeError);
        }
        if matches!(self.behavior, ClaimBehavior::WrongPrepareContext) {
            context.request_id = RequestId::new("wrong-claim-prepare-context").expect("id");
        }
        let claim = if matches!(self.behavior, ClaimBehavior::ZeroPreparedId) {
            PreparedTransaction::new(
                TransactionId::from_bytes([0; 32]),
                ExactTransactionBytes::new(vec![0xca, 0xfe]).expect("claim bytes"),
            )
        } else {
            prepared_claim_transaction()
        };
        Ok(PrepareRevealingClaimResult::new(context, claim))
    }

    async fn observe_revealing_claim(
        &self,
        request: ObserveRevealingClaimRequest,
    ) -> Result<ObserveRevealingClaimResult, Self::Error> {
        self.observe_requests
            .lock()
            .expect("claim observe log")
            .push(request);
        if matches!(self.behavior, ClaimBehavior::FailObserve) {
            return Err(FakeError);
        }
        let mut response = self
            .observations
            .lock()
            .expect("claim observations")
            .pop_front()
            .ok_or(FakeError)?;
        if matches!(self.behavior, ClaimBehavior::WrongObserveContext) {
            response.context.request_id =
                RequestId::new("wrong-claim-observe-context").expect("id");
        }
        Ok(response)
    }

    async fn submit_transaction(
        &self,
        request: SubmitTransactionRequest,
    ) -> Result<SubmitTransactionResult, Self::Error> {
        self.submit_requests
            .lock()
            .expect("claim submit log")
            .push(request.clone());
        if matches!(self.behavior, ClaimBehavior::FailSubmit) {
            return Err(FakeError);
        }
        let mut context = request.context;
        if matches!(self.behavior, ClaimBehavior::WrongSubmitContext) {
            context.request_id = RequestId::new("wrong-claim-submit-context").expect("id");
        }
        let transaction_id = if matches!(self.behavior, ClaimBehavior::WrongSubmitId) {
            TransactionId::from_bytes([0x55; 32])
        } else {
            request.transaction.transaction_id
        };
        Ok(SubmitTransactionResult::new(
            context,
            transaction_id,
            SubmissionOutcome::Accepted,
        ))
    }
}

#[tokio::test]
async fn wrapper_rejects_mutated_canonical_funding_before_opening_the_sidecar() {
    let agreement = agreement();
    assert!(
        FirstLockConfirmedEvidenceV1::new(
            FirstLockStepV1::LezFund,
            [0x22; 32],
            "22".repeat(32),
            0,
        )
        .is_err(),
        "zero-depth evidence is rejected before it can reach a funding source or sidecar"
    );
    let mutations = [
        (
            "step",
            FirstLockConfirmedEvidenceV1::new(
                FirstLockStepV1::ZcashFund,
                [0x22; 32],
                "22".repeat(32),
                1,
            )
            .expect("independently valid wrong-chain evidence"),
        ),
        (
            "transaction-id",
            FirstLockConfirmedEvidenceV1::new(
                FirstLockStepV1::LezFund,
                [0x22; 32],
                "33".repeat(32),
                1,
            )
            .expect("independently valid mismatched transaction ID"),
        ),
    ];

    for (suffix, evidence) in mutations {
        let journal_path = isolated_sqlite_path(&format!("lez-wrapper-claim-{suffix}"));
        let opens = Arc::new(AtomicUsize::new(0));
        let context_calls = Arc::new(AtomicUsize::new(0));
        let transport = ClaimTransport::new([]);
        let ports = ContextOwningLezBridgePorts::new(
            RunId::new(format!("claim-mutation-{suffix}")).expect("run ID"),
            runtime_for_participant(&agreement, Participant::Maker),
            Participant::Maker,
            CloneTransportFactory {
                transport: transport.clone(),
                opens: Arc::clone(&opens),
            },
            QueuedContexts {
                requests: Arc::new(Mutex::new(VecDeque::new())),
                calls: Arc::clone(&context_calls),
            },
            (),
            FixedCanonicalFunding(evidence),
            SqliteBridgeOperationJournal::open(&journal_path).expect("operation journal"),
        )
        .expect("role-local claim wrapper");

        assert!(matches!(
            ports
                .prepare_revealing_claim(&agreement, &ClaimPreimage::new([0x91; 32]))
                .await,
            Err(ContextOwningLezPortError::InvalidCanonicalFunding)
        ));
        assert_eq!(opens.load(Ordering::SeqCst), 0);
        assert_eq!(context_calls.load(Ordering::SeqCst), 0);
        assert!(
            transport
                .prepare_requests
                .lock()
                .expect("claim prepare log")
                .is_empty()
        );
        drop(ports);
        remove_sqlite_files(&journal_path);
    }
}

#[tokio::test]
async fn signed_claimant_prepares_observes_and_submits_exact_revealing_claim_once() {
    let agreement = agreement();
    let transport = ClaimTransport::new([claim_found_observation(
        &agreement,
        claim_context(Participant::Maker, "claim-observe-0001"),
    )]);
    let adapter = claim_adapter(transport.clone(), &agreement, Participant::Maker);
    let preimage = ClaimPreimage::new([0x91; 32]);
    let funding_id = TransactionId::from_bytes([0x22; 32]);

    let prepared = adapter
        .prepare_native_revealing_claim(
            &agreement,
            RequestId::new("claim-prepare-0001").expect("request id"),
            funding_id,
            &preimage,
        )
        .await
        .expect("official agreement-bound claim");
    assert_eq!(prepared.step(), ClaimStepV1::RevealingLez);
    assert_eq!(prepared.expected_submission_id(), &[0x34; 32]);
    assert_eq!(prepared.exact_submission(), &[0xca, 0xfe]);

    let observed = adapter
        .observe_prepared_native_revealing_claim(
            &agreement,
            RequestId::new("claim-observe-0001").expect("request id"),
            &prepared,
        )
        .await
        .expect("canonical exact revealing claim");
    let RevealingClaimObservationV1::Confirmed(evidence) = observed else {
        panic!("canonical claim must produce evidence")
    };
    assert_eq!(evidence.observed_submission_id(), &[0x34; 32]);
    assert_eq!(evidence.preimage().expose_secret(), &[0x91; 32]);
    assert_eq!(evidence.confirmations(), 2);

    assert_eq!(
        adapter
            .submit_native_revealing_claim(
                &agreement,
                RequestId::new("claim-submit-0001").expect("request id"),
                &prepared,
            )
            .await
            .expect("one submit attempt"),
        RevealingClaimSubmitOutcome::Accepted,
    );

    let prepare_requests = transport.prepare_requests.lock().expect("claim log");
    assert_eq!(prepare_requests.len(), 1);
    assert_eq!(prepare_requests[0].funding_transaction_id, funding_id);
    assert_eq!(prepare_requests[0].preimage().expose_secret(), &[0x91; 32]);
    assert!(!format!("{:?}", prepare_requests[0]).contains("145, 145"));
    let observe_requests = transport.observe_requests.lock().expect("claim log");
    assert!(matches!(
        observe_requests[0].target,
        RevealingClaimObservationTarget::Exact { claim_transaction_id }
            if claim_transaction_id == TransactionId::from_bytes([0x34; 32])
    ));
    assert_eq!(
        transport.submit_requests.lock().expect("claim log").len(),
        1
    );
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn revealing_claim_roles_and_discovery_hold_for_both_signed_directions() {
    for (agreement, claimant, depositor, suffix) in [
        (
            agreement(),
            Participant::Maker,
            Participant::Taker,
            "forward",
        ),
        (
            agreement_for_direction(
                LezEnvironmentV1::DeterministicLocalV0_1_2Compatibility,
                false,
                SwapDirection::TakerSellsForeign,
            ),
            Participant::Taker,
            Participant::Maker,
            "reverse",
        ),
    ] {
        let owner_transport = ClaimTransport::new([]);
        let owner = claim_adapter(owner_transport.clone(), &agreement, claimant);
        owner
            .prepare_native_revealing_claim(
                &agreement,
                RequestId::new(format!("claim-owner-{suffix}")).expect("request id"),
                TransactionId::from_bytes([0x22; 32]),
                &ClaimPreimage::new([0x91; 32]),
            )
            .await
            .expect("signed claimant prepares");
        assert_eq!(
            owner_transport.prepare_requests.lock().expect("log").len(),
            1
        );
        assert!(matches!(
            owner
                .observe_counterparty_native_revealing_claim(
                    &agreement,
                    RequestId::new(format!("claim-owner-discovery-{suffix}")).expect("request id"),
                    DiscoveryWindow::new(10, 3).expect("window"),
                )
                .await,
            Err(NativeRevealingClaimAdapterError::DiscoveryRequiresDepositor)
        ));

        let prepared = prepared_claim_submission();
        let observer_context = claim_context(depositor, &format!("claim-discovery-{suffix}"));
        let observer_transport =
            ClaimTransport::new([claim_found_observation(&agreement, observer_context)]);
        let observer = claim_adapter(observer_transport.clone(), &agreement, depositor);
        assert!(matches!(
            observer
                .prepare_native_revealing_claim(
                    &agreement,
                    RequestId::new(format!("claim-nonowner-prepare-{suffix}")).expect("request id"),
                    TransactionId::from_bytes([0x22; 32]),
                    &ClaimPreimage::new([0x91; 32]),
                )
                .await,
            Err(NativeRevealingClaimAdapterError::WrongClaimant)
        ));
        assert!(matches!(
            observer
                .observe_prepared_native_revealing_claim(
                    &agreement,
                    RequestId::new(format!("claim-nonowner-exact-{suffix}")).expect("request id"),
                    &prepared,
                )
                .await,
            Err(NativeRevealingClaimAdapterError::ExactTargetRequiresClaimant)
        ));
        assert!(matches!(
            observer
                .submit_native_revealing_claim(
                    &agreement,
                    RequestId::new(format!("claim-nonowner-submit-{suffix}")).expect("request id"),
                    &prepared,
                )
                .await,
            Err(NativeRevealingClaimAdapterError::WrongClaimant)
        ));
        let window = DiscoveryWindow::new(10, 3).expect("window");
        assert!(matches!(
            observer
                .observe_counterparty_native_revealing_claim(
                    &agreement,
                    RequestId::new(format!("claim-discovery-{suffix}")).expect("request id"),
                    window,
                )
                .await,
            Ok(RevealingClaimObservationV1::Confirmed(_))
        ));
        let requests = observer_transport.observe_requests.lock().expect("log");
        assert_eq!(requests.len(), 1);
        assert!(matches!(
            requests[0].target,
            RevealingClaimObservationTarget::DiscoverByTerms { window: actual }
                if actual == window
        ));
        assert!(
            observer_transport
                .prepare_requests
                .lock()
                .expect("log")
                .is_empty()
        );
        assert!(
            observer_transport
                .submit_requests
                .lock()
                .expect("log")
                .is_empty()
        );
    }
}

#[tokio::test]
async fn revealing_claim_absence_is_stable_only_for_exact_or_fully_covered_discovery() {
    let agreement = agreement();
    let stable_tip = ChainTip::new(Hex32::from_bytes([0x90; 32]), 12);
    for (suffix, claim, before, after, expected) in [
        (
            "exact-absent",
            RevealingClaimObservation::Absent,
            stable_tip,
            stable_tip,
            "absent",
        ),
        (
            "exact-unknown",
            RevealingClaimObservation::UnknownOrPending,
            stable_tip,
            stable_tip,
            "unstable",
        ),
        (
            "exact-drift",
            RevealingClaimObservation::Absent,
            stable_tip,
            ChainTip::new(Hex32::from_bytes([0x91; 32]), 13),
            "unstable",
        ),
    ] {
        let request_id = format!("claim-{suffix}");
        let transport = ClaimTransport::new([ObserveRevealingClaimResult::new(
            claim_context(Participant::Maker, &request_id),
            before,
            claim,
            after,
        )]);
        let adapter = claim_adapter(transport, &agreement, Participant::Maker);
        let result = adapter
            .observe_prepared_native_revealing_claim(
                &agreement,
                RequestId::new(request_id).expect("request id"),
                &prepared_claim_submission(),
            )
            .await
            .expect("conservative exact absence");
        match expected {
            "absent" => assert!(matches!(result, RevealingClaimObservationV1::Absent)),
            "unstable" => assert!(matches!(result, RevealingClaimObservationV1::Unstable)),
            _ => unreachable!("fixed case"),
        }
    }

    for (suffix, window, expected) in [
        (
            "covered",
            DiscoveryWindow::new(10, 3).expect("window"),
            "absent",
        ),
        (
            "incomplete",
            DiscoveryWindow::new(11, 3).expect("window"),
            "unstable",
        ),
    ] {
        let request_id = format!("claim-discovery-{suffix}");
        let transport = ClaimTransport::new([ObserveRevealingClaimResult::new(
            claim_context(Participant::Taker, &request_id),
            stable_tip,
            RevealingClaimObservation::Absent,
            stable_tip,
        )]);
        let adapter = claim_adapter(transport, &agreement, Participant::Taker);
        let result = adapter
            .observe_counterparty_native_revealing_claim(
                &agreement,
                RequestId::new(request_id).expect("request id"),
                window,
            )
            .await
            .expect("bounded discovery absence");
        match expected {
            "absent" => assert!(matches!(result, RevealingClaimObservationV1::Absent)),
            "unstable" => assert!(matches!(result, RevealingClaimObservationV1::Unstable)),
            _ => unreachable!("fixed case"),
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum ClaimMutation {
    ResponseContext,
    TipHash,
    TipHeight,
    TransactionId,
    TransactionBytes,
    NonPublic,
    Signer,
    Program,
    Accounts,
    SwapId,
    Preimage,
    AboveTip,
    SameHeightWrongHash,
    MetadataOwner,
    MetadataAccount,
    MetadataTerms,
    MetadataStatus,
    CustodyAccount,
    CustodyOwner,
    CustodyBalance,
    DepthOverflow,
}

const ALL_CLAIM_MUTATIONS: [ClaimMutation; 21] = [
    ClaimMutation::ResponseContext,
    ClaimMutation::TipHash,
    ClaimMutation::TipHeight,
    ClaimMutation::TransactionId,
    ClaimMutation::TransactionBytes,
    ClaimMutation::NonPublic,
    ClaimMutation::Signer,
    ClaimMutation::Program,
    ClaimMutation::Accounts,
    ClaimMutation::SwapId,
    ClaimMutation::Preimage,
    ClaimMutation::AboveTip,
    ClaimMutation::SameHeightWrongHash,
    ClaimMutation::MetadataOwner,
    ClaimMutation::MetadataAccount,
    ClaimMutation::MetadataTerms,
    ClaimMutation::MetadataStatus,
    ClaimMutation::CustodyAccount,
    ClaimMutation::CustodyOwner,
    ClaimMutation::CustodyBalance,
    ClaimMutation::DepthOverflow,
];

#[tokio::test]
async fn revealing_claim_primitive_exact_account_tip_and_depth_mutations_fail_closed() {
    let agreement = agreement();
    for mutation in ALL_CLAIM_MUTATIONS {
        let mut response = claim_found_observation(
            &agreement,
            claim_context(Participant::Maker, "claim-mutated"),
        );
        mutate_claim_observation(&mut response, mutation);
        let transport = ClaimTransport::new([response]);
        let adapter = claim_adapter(transport.clone(), &agreement, Participant::Maker);
        let result = adapter
            .observe_prepared_native_revealing_claim(
                &agreement,
                RequestId::new("claim-mutated").expect("request id"),
                &prepared_claim_submission(),
            )
            .await;
        assert!(
            !matches!(result, Ok(RevealingClaimObservationV1::Confirmed(_))),
            "mutation {mutation:?} must never produce evidence",
        );
        assert_eq!(transport.observe_requests.lock().expect("log").len(), 1);
    }
}

#[tokio::test]
async fn discovered_revealing_claim_must_be_inside_the_caller_window() {
    let agreement = agreement();
    let mut response = claim_found_observation(
        &agreement,
        claim_context(Participant::Taker, "claim-outside-window"),
    );
    let RevealingClaimObservation::Found(found) = &mut response.claim else {
        panic!("fixture contains a claim")
    };
    found.transaction.position.height = 9;
    let transport = ClaimTransport::new([response]);
    let adapter = claim_adapter(transport, &agreement, Participant::Taker);
    assert!(matches!(
        adapter
            .observe_counterparty_native_revealing_claim(
                &agreement,
                RequestId::new("claim-outside-window").expect("request id"),
                DiscoveryWindow::new(10, 3).expect("window"),
            )
            .await,
        Err(NativeRevealingClaimAdapterError::InconsistentFacts)
    ));
}

#[tokio::test]
async fn revealing_claim_attempts_are_once_and_submit_uncertainty_is_unknown() {
    let agreement = agreement();
    let zero_funding_transport = ClaimTransport::new([]);
    let zero_funding = claim_adapter(
        zero_funding_transport.clone(),
        &agreement,
        Participant::Maker,
    );
    assert!(
        zero_funding
            .prepare_native_revealing_claim(
                &agreement,
                RequestId::new("claim-zero-funding").expect("request id"),
                TransactionId::from_bytes([0; 32]),
                &ClaimPreimage::new([0x91; 32]),
            )
            .await
            .is_err()
    );
    assert!(
        zero_funding_transport
            .prepare_requests
            .lock()
            .expect("log")
            .is_empty()
    );

    for (suffix, behavior) in [
        ("transport", ClaimBehavior::FailPrepare),
        ("context", ClaimBehavior::WrongPrepareContext),
        ("identity", ClaimBehavior::ZeroPreparedId),
    ] {
        let transport = ClaimTransport::new([]).with_behavior(behavior);
        let adapter = claim_adapter(transport.clone(), &agreement, Participant::Maker);
        assert!(
            adapter
                .prepare_native_revealing_claim(
                    &agreement,
                    RequestId::new(format!("claim-prepare-{suffix}")).expect("request id"),
                    TransactionId::from_bytes([0x22; 32]),
                    &ClaimPreimage::new([0x91; 32]),
                )
                .await
                .is_err()
        );
        assert_eq!(transport.prepare_requests.lock().expect("log").len(), 1);
    }

    for (suffix, behavior) in [
        ("transport", ClaimBehavior::FailObserve),
        ("context", ClaimBehavior::WrongObserveContext),
    ] {
        let transport = ClaimTransport::new([claim_found_observation(
            &agreement,
            claim_context(Participant::Maker, &format!("claim-observe-{suffix}")),
        )])
        .with_behavior(behavior);
        let adapter = claim_adapter(transport.clone(), &agreement, Participant::Maker);
        assert!(
            adapter
                .observe_prepared_native_revealing_claim(
                    &agreement,
                    RequestId::new(format!("claim-observe-{suffix}")).expect("request id"),
                    &prepared_claim_submission(),
                )
                .await
                .is_err()
        );
        assert_eq!(transport.observe_requests.lock().expect("log").len(), 1);
    }

    for (suffix, behavior) in [
        ("transport", ClaimBehavior::FailSubmit),
        ("context", ClaimBehavior::WrongSubmitContext),
        ("identity", ClaimBehavior::WrongSubmitId),
    ] {
        let transport = ClaimTransport::new([]).with_behavior(behavior);
        let adapter = claim_adapter(transport.clone(), &agreement, Participant::Maker);
        assert_eq!(
            adapter
                .submit_native_revealing_claim(
                    &agreement,
                    RequestId::new(format!("claim-submit-{suffix}")).expect("request id"),
                    &prepared_claim_submission(),
                )
                .await
                .expect("uncertain submit is typed"),
            RevealingClaimSubmitOutcome::Unknown,
        );
        assert_eq!(transport.submit_requests.lock().expect("log").len(), 1);
    }
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn revealing_claim_wrong_step_runtime_environment_and_asset_fail_before_transport() {
    let agreement = agreement();
    let wrong_step =
        PreparedClaimSubmissionV1::new(ClaimStepV1::FollowupZcash, [0x34; 32], vec![0xca, 0xfe])
            .expect("independently valid wrong step");
    let transport = ClaimTransport::new([]);
    let adapter = claim_adapter(transport.clone(), &agreement, Participant::Maker);
    assert!(matches!(
        adapter
            .submit_native_revealing_claim(
                &agreement,
                RequestId::new("claim-wrong-step").expect("request id"),
                &wrong_step,
            )
            .await,
        Err(NativeRevealingClaimAdapterError::WrongPreparedStep)
    ));
    assert!(transport.submit_requests.lock().expect("log").is_empty());

    for (mutation, expected) in [
        (RuntimeMutation::Channel, "chain"),
        (RuntimeMutation::Genesis, "chain"),
        (RuntimeMutation::Program, "program"),
        (RuntimeMutation::Signer, "signer"),
    ] {
        let transport = ClaimTransport::new([]);
        let mut descriptor = runtime(&agreement);
        descriptor.sidecar_role = BridgeParticipant::Maker;
        descriptor.signer_account_id =
            Hex32::from_bytes(*agreement.lez_account(Participant::Maker));
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
            Participant::Maker,
        )
        .expect("matching role");
        let error = adapter
            .prepare_native_revealing_claim(
                &agreement,
                RequestId::new(format!("claim-runtime-{expected}")).expect("request id"),
                TransactionId::from_bytes([0x22; 32]),
                &ClaimPreimage::new([0x91; 32]),
            )
            .await
            .expect_err("runtime drift fails closed");
        match expected {
            "chain" => assert!(matches!(
                error,
                NativeRevealingClaimAdapterError::ChainIdentityMismatch
            )),
            "program" => assert!(matches!(
                error,
                NativeRevealingClaimAdapterError::EscrowProgramMismatch
            )),
            "signer" => assert!(matches!(
                error,
                NativeRevealingClaimAdapterError::SignerAccountMismatch
            )),
            _ => unreachable!("fixed case"),
        }
        assert!(transport.prepare_requests.lock().expect("log").is_empty());
    }

    for (unsupported, expected) in [
        (
            agreement_for(LezEnvironmentV1::DeterministicLocalV0_2, false),
            "environment",
        ),
        (
            agreement_for(
                LezEnvironmentV1::DeterministicLocalV0_1_2Compatibility,
                true,
            ),
            "asset",
        ),
    ] {
        let claimant = unsupported.lez_claimant();
        let transport = ClaimTransport::new([]);
        let adapter = claim_adapter(transport.clone(), &unsupported, claimant);
        let error = adapter
            .prepare_native_revealing_claim(
                &unsupported,
                RequestId::new(format!("claim-unsupported-{expected}")).expect("request id"),
                TransactionId::from_bytes([0x22; 32]),
                &ClaimPreimage::new([0x91; 32]),
            )
            .await
            .expect_err("unsupported terms fail closed");
        match expected {
            "environment" => assert!(matches!(
                error,
                NativeRevealingClaimAdapterError::IncompatibleEnvironment
            )),
            "asset" => assert!(matches!(
                error,
                NativeRevealingClaimAdapterError::UnsupportedAsset
            )),
            _ => unreachable!("fixed case"),
        }
        assert!(transport.prepare_requests.lock().expect("log").is_empty());
    }
}

fn prepared_claim_submission() -> PreparedClaimSubmissionV1 {
    PreparedClaimSubmissionV1::new(ClaimStepV1::RevealingLez, [0x34; 32], vec![0xca, 0xfe])
        .expect("protected revealing claim")
}

fn prepared_claim_transaction() -> PreparedTransaction {
    PreparedTransaction::new(
        TransactionId::from_bytes([0x34; 32]),
        ExactTransactionBytes::new(vec![0xca, 0xfe]).expect("claim bytes"),
    )
}

fn claim_context(participant: Participant, request_id: &str) -> MessageContext {
    refund_context(participant, request_id)
}

fn claim_adapter(
    transport: ClaimTransport,
    agreement: &ZecAgreementV1,
    participant: Participant,
) -> LezBridgeAdapter<ClaimTransport> {
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
    .expect("matching claim actor")
}

fn claim_found_observation(
    agreement: &ZecAgreementV1,
    context: MessageContext,
) -> ObserveRevealingClaimResult {
    let terms = native_terms(agreement);
    let tip = ChainTip::new(Hex32::from_bytes([0x90; 32]), 12);
    let metadata = Hex32::from_bytes(*agreement.lez_terms().metadata_account());
    let custody = Hex32::from_bytes(*agreement.lez_terms().custody_account());
    let claimant = Hex32::from_bytes(*agreement.lez_account(agreement.lez_claimant()));
    let program = Hex32::from_bytes(program_bytes(agreement.lez_terms().escrow_program_id()));
    ObserveRevealingClaimResult::new(
        context,
        tip,
        RevealingClaimObservation::found(RevealingClaimFoundFacts::new(
            ObservedTransactionFacts::new(
                TransactionId::from_bytes([0x34; 32]),
                ExactTransactionBytes::new(vec![0xca, 0xfe]).expect("claim bytes"),
                ChainPosition::new(Hex32::from_bytes([0x82; 32]), 11, 0),
                AccountIds::new(vec![claimant]).expect("one claimant signer"),
                true,
            ),
            NativeClaimInstructionFacts::new(
                program,
                AccountIds::new(vec![metadata, custody, claimant]).expect("claim accounts"),
                Hex32::from_bytes(*agreement.onchain_swap_id()),
                RevealingPreimage::new([0x91; 32]),
            ),
            EscrowMetadataFacts::from_native_terms(
                metadata,
                program,
                custody,
                &terms,
                EscrowState::Claimed,
            ),
            NativeCustodyFacts::new(custody, terms.authenticated_transfer_program_id(), 0),
        )),
        tip,
    )
}

#[allow(clippy::too_many_lines)]
fn mutate_claim_observation(response: &mut ObserveRevealingClaimResult, mutation: ClaimMutation) {
    let RevealingClaimObservation::Found(found) = &mut response.claim else {
        panic!("canonical fixture contains a claim")
    };
    match mutation {
        ClaimMutation::ResponseContext => {
            response.context.request_id = RequestId::new("wrong-claim-context").expect("id");
        }
        ClaimMutation::TipHash => {
            response.tip_after.block_hash = Hex32::from_bytes([0x91; 32]);
        }
        ClaimMutation::TipHeight => response.tip_after.height += 1,
        ClaimMutation::TransactionId => {
            found.transaction.transaction_id = TransactionId::from_bytes([0x35; 32]);
        }
        ClaimMutation::TransactionBytes => {
            found.transaction.exact_bytes =
                ExactTransactionBytes::new(vec![0xde, 0xad]).expect("changed bytes");
        }
        ClaimMutation::NonPublic => found.transaction.is_public = false,
        ClaimMutation::Signer => {
            found.transaction.signer_account_ids =
                AccountIds::new(vec![Hex32::from_bytes([0x92; 32])]).expect("signer");
        }
        ClaimMutation::Program => {
            found.instruction.program_id = Hex32::from_bytes([0x93; 32]);
        }
        ClaimMutation::Accounts => {
            found.instruction.ordered_account_ids =
                AccountIds::new(vec![Hex32::from_bytes([0x94; 32])]).expect("accounts");
        }
        ClaimMutation::SwapId => {
            found.instruction.swap_id = Hex32::from_bytes([0x95; 32]);
        }
        ClaimMutation::Preimage => {
            found.instruction.preimage = RevealingPreimage::new([0x96; 32]);
        }
        ClaimMutation::AboveTip => found.transaction.position.height = 13,
        ClaimMutation::SameHeightWrongHash => found.transaction.position.height = 12,
        ClaimMutation::MetadataOwner => {
            found.metadata.owner_program_id = Hex32::from_bytes([0x97; 32]);
        }
        ClaimMutation::MetadataAccount => {
            found.metadata.account_id = Hex32::from_bytes([0x98; 32]);
        }
        ClaimMutation::MetadataTerms => {
            found.metadata.terms_hash = Hex32::from_bytes([0x99; 32]);
        }
        ClaimMutation::MetadataStatus => found.metadata.status = EscrowState::Funded,
        ClaimMutation::CustodyAccount => {
            found.custody.account_id = Hex32::from_bytes([0x9a; 32]);
        }
        ClaimMutation::CustodyOwner => {
            found.custody.owner_program_id = Hex32::from_bytes([0x9b; 32]);
        }
        ClaimMutation::CustodyBalance => found.custody.balance = NativeAmount::new(1),
        ClaimMutation::DepthOverflow => {
            response.tip_before.height = u64::MAX;
            response.tip_after.height = u64::MAX;
        }
    }
}

#[tokio::test]
async fn signed_owner_refund_state_prepare_exact_observe_and_submit_are_typed_once() {
    let agreement = agreement();
    let transport = RefundTransport::new([
        refund_state_observation(
            &agreement,
            refund_context(Participant::Taker, "refund-state-0001"),
        ),
        refund_found_observation(
            &agreement,
            refund_context(Participant::Taker, "refund-exact-0001"),
        ),
    ]);
    let adapter = refund_adapter(transport.clone(), &agreement, Participant::Taker);

    let eligibility = adapter
        .observe_native_refund_eligibility(
            &agreement,
            RequestId::new("refund-state-0001").expect("request id"),
        )
        .await
        .expect("canonical funded eligibility");
    assert_eq!(
        eligibility,
        RefundEligibilityObservationV1::canonical(
            lez_swap_core::ChainPosition::lez_timestamp_from_milliseconds_floor(
                LezUnixMilliseconds::new(200_000),
            ),
        )
    );

    let prepared = adapter
        .prepare_native_refund(
            &agreement,
            RequestId::new("refund-prepare-0001").expect("request id"),
        )
        .await
        .expect("agreement-bound refund preparation");
    assert_eq!(prepared.step(), RefundStepV1::Lez);
    assert_eq!(prepared.expected_submission_id(), &[0x33; 32]);
    assert_eq!(prepared.exact_submission(), &[0xee, 0xff]);

    let window = DiscoveryWindow::new(10, 3).expect("caller-owned window");
    let observed = adapter
        .observe_prepared_native_refund(
            &agreement,
            RequestId::new("refund-exact-0001").expect("request id"),
            &prepared,
            window,
        )
        .await
        .expect("exact canonical refund");
    let RefundObservationV1::Confirmed(evidence) = observed else {
        panic!("found exact refund must produce evidence");
    };
    assert_eq!(evidence.step(), RefundStepV1::Lez);
    assert_eq!(evidence.observed_submission_id(), &[0x33; 32]);
    assert_eq!(evidence.position().chain(), Chain::Lez);
    assert_eq!(evidence.position().value(), 200);
    assert_eq!(evidence.confirmations(), 2);

    assert_eq!(
        adapter
            .submit_native_refund(
                &agreement,
                RequestId::new("refund-submit-0001").expect("request id"),
                &prepared,
            )
            .await
            .expect("one exact submit attempt"),
        RefundSubmitOutcomeV1::Accepted
    );

    let observe_requests = transport.observe_requests.lock().expect("request log");
    assert!(matches!(
        observe_requests[0].target,
        NativeRefundObservationTarget::StateOnly
    ));
    assert!(matches!(
        observe_requests[1].target,
        NativeRefundObservationTarget::Exact {
            refund_transaction_id,
            window: actual,
        } if refund_transaction_id == TransactionId::from_bytes([0x33; 32]) && actual == window
    ));
    assert_eq!(transport.prepare_requests.lock().expect("log").len(), 1);
    assert_eq!(transport.submit_requests.lock().expect("log").len(), 1);
}

#[tokio::test]
async fn signed_refund_roles_hold_for_both_swap_directions() {
    for (agreement, owner, claimant, suffix) in [
        (
            agreement(),
            Participant::Taker,
            Participant::Maker,
            "forward",
        ),
        (
            agreement_for_direction(
                LezEnvironmentV1::DeterministicLocalV0_1_2Compatibility,
                false,
                SwapDirection::TakerSellsForeign,
            ),
            Participant::Maker,
            Participant::Taker,
            "reverse",
        ),
    ] {
        let request_id = format!("refund-owner-{suffix}");
        let transport = RefundTransport::new([]);
        let adapter = refund_adapter(transport.clone(), &agreement, owner);
        adapter
            .prepare_native_refund(&agreement, RequestId::new(request_id).expect("request id"))
            .await
            .expect("signed depositor owns refund preparation");
        assert_eq!(transport.prepare_requests.lock().expect("log").len(), 1);

        let nonowner_transport = RefundTransport::new([]);
        let nonowner = refund_adapter(nonowner_transport.clone(), &agreement, claimant);
        let prepared = prepared_refund_submission();
        assert!(matches!(
            nonowner
                .observe_native_refund_eligibility(
                    &agreement,
                    RequestId::new(format!("refund-state-nonowner-{suffix}")).expect("request id"),
                )
                .await,
            Err(NativeRefundAdapterError::WrongOwner)
        ));
        assert!(matches!(
            nonowner
                .prepare_native_refund(
                    &agreement,
                    RequestId::new(format!("refund-prepare-nonowner-{suffix}"))
                        .expect("request id"),
                )
                .await,
            Err(NativeRefundAdapterError::WrongOwner)
        ));
        assert!(matches!(
            nonowner
                .submit_native_refund(
                    &agreement,
                    RequestId::new(format!("refund-submit-nonowner-{suffix}")).expect("request id"),
                    &prepared,
                )
                .await,
            Err(NativeRefundAdapterError::WrongOwner)
        ));
        assert!(
            nonowner_transport
                .prepare_requests
                .lock()
                .expect("log")
                .is_empty()
        );
        assert!(
            nonowner_transport
                .observe_requests
                .lock()
                .expect("log")
                .is_empty()
        );
        assert!(
            nonowner_transport
                .submit_requests
                .lock()
                .expect("log")
                .is_empty()
        );
    }
}

#[tokio::test]
async fn exact_and_discovery_refund_paths_are_role_separated_and_window_bound() {
    let agreement = agreement();
    let window = DiscoveryWindow::new(10, 3).expect("window");
    let prepared = prepared_refund_submission();

    let owner_transport = RefundTransport::new([]);
    let owner = refund_adapter(owner_transport.clone(), &agreement, Participant::Taker);
    assert!(matches!(
        owner
            .observe_counterparty_native_refund(
                &agreement,
                RequestId::new("refund-owner-discovery").expect("request id"),
                window,
            )
            .await,
        Err(NativeRefundAdapterError::DiscoveryRequiresClaimant)
    ));
    assert!(
        owner_transport
            .observe_requests
            .lock()
            .expect("log")
            .is_empty()
    );

    let claimant_context = refund_context(Participant::Maker, "refund-claimant-discovery");
    let claimant_transport =
        RefundTransport::new([refund_found_observation(&agreement, claimant_context)]);
    let claimant = refund_adapter(claimant_transport.clone(), &agreement, Participant::Maker);
    assert!(matches!(
        claimant
            .observe_prepared_native_refund(
                &agreement,
                RequestId::new("refund-claimant-exact").expect("request id"),
                &prepared,
                window,
            )
            .await,
        Err(NativeRefundAdapterError::ExactTargetRequiresOwner)
    ));
    assert!(matches!(
        claimant
            .observe_counterparty_native_refund(
                &agreement,
                RequestId::new("refund-claimant-discovery").expect("request id"),
                window,
            )
            .await,
        Ok(RefundObservationV1::Confirmed(_))
    ));
    let requests = claimant_transport.observe_requests.lock().expect("log");
    assert_eq!(requests.len(), 1);
    assert!(matches!(
        requests[0].target,
        NativeRefundObservationTarget::DiscoverByTerms { window: actual } if actual == window
    ));
}

#[tokio::test]
async fn eligibility_distinguishes_absent_funded_and_spent_accounts() {
    let agreement = agreement();
    for (suffix, accounts, expected) in [
        (
            "absent",
            NativeEscrowAccountObservation::Absent,
            RefundEligibilityObservationV1::FundingUnavailable(RefundFundingWaitReasonV1::Absent),
        ),
        (
            "empty",
            refund_accounts(&agreement, EscrowState::Empty, 0),
            RefundEligibilityObservationV1::FundingUnavailable(RefundFundingWaitReasonV1::Absent),
        ),
        (
            "claimed",
            refund_accounts(&agreement, EscrowState::Claimed, 0),
            RefundEligibilityObservationV1::FundingUnavailable(RefundFundingWaitReasonV1::Spent),
        ),
        (
            "refunded",
            refund_accounts(&agreement, EscrowState::Refunded, 0),
            RefundEligibilityObservationV1::FundingUnavailable(RefundFundingWaitReasonV1::Spent),
        ),
    ] {
        let request_id = format!("refund-eligibility-{suffix}");
        let clock = refund_clock();
        let transport = RefundTransport::new([ObserveNativeRefundResult::new(
            refund_context(Participant::Taker, &request_id),
            clock,
            accounts,
            NativeRefundObservation::NotRequested,
            clock,
        )]);
        let adapter = refund_adapter(transport, &agreement, Participant::Taker);
        assert_eq!(
            adapter
                .observe_native_refund_eligibility(
                    &agreement,
                    RequestId::new(request_id).expect("request id"),
                )
                .await
                .expect("stable typed eligibility"),
            expected
        );
    }
}

#[tokio::test]
async fn eligibility_rejects_partial_facts_clock_drift_and_refund_lookup_claims() {
    let agreement = agreement();
    for (suffix, mut response, expected) in [
        (
            "partial",
            refund_state_observation(
                &agreement,
                refund_context(Participant::Taker, "refund-state-partial"),
            ),
            "facts",
        ),
        (
            "clock",
            refund_state_observation(
                &agreement,
                refund_context(Participant::Taker, "refund-state-clock"),
            ),
            "clock",
        ),
        (
            "lookup",
            refund_state_observation(
                &agreement,
                refund_context(Participant::Taker, "refund-state-lookup"),
            ),
            "facts",
        ),
    ] {
        match suffix {
            "partial" => {
                let NativeEscrowAccountObservation::Found(facts) = &mut response.accounts else {
                    panic!("fixture has full facts")
                };
                facts.custody.account_id = Hex32::from_bytes([0x77; 32]);
            }
            "clock" => response.clock_after.timestamp_ms += 1,
            "lookup" => response.refund = NativeRefundObservation::Absent,
            _ => unreachable!("fixed cases"),
        }
        let transport = RefundTransport::new([response]);
        let adapter = refund_adapter(transport, &agreement, Participant::Taker);
        let error = adapter
            .observe_native_refund_eligibility(
                &agreement,
                RequestId::new(format!("refund-state-{suffix}")).expect("request id"),
            )
            .await
            .expect_err("malformed state fails closed");
        match expected {
            "clock" => assert!(matches!(error, NativeRefundAdapterError::UnstableClock)),
            "facts" => assert!(matches!(error, NativeRefundAdapterError::InconsistentFacts)),
            _ => unreachable!("fixed cases"),
        }
    }
}

#[tokio::test]
async fn refund_absence_requires_a_stable_fully_covered_window() {
    let agreement = agreement();
    let prepared = prepared_refund_submission();
    let covered = DiscoveryWindow::new(10, 3).expect("covered window");
    let incomplete = DiscoveryWindow::new(11, 3).expect("incomplete window");
    for (suffix, window, accounts, refund, expected) in [
        (
            "covered",
            covered,
            refund_accounts(
                &agreement,
                EscrowState::Funded,
                agreement.lez_terms().amount(),
            ),
            NativeRefundObservation::Absent,
            RefundObservationV1::Absent,
        ),
        (
            "incomplete",
            incomplete,
            refund_accounts(
                &agreement,
                EscrowState::Funded,
                agreement.lez_terms().amount(),
            ),
            NativeRefundObservation::Absent,
            RefundObservationV1::Unstable,
        ),
        (
            "unknown",
            covered,
            refund_accounts(
                &agreement,
                EscrowState::Funded,
                agreement.lez_terms().amount(),
            ),
            NativeRefundObservation::UnknownOrPending,
            RefundObservationV1::Unstable,
        ),
        (
            "terminal",
            covered,
            refund_accounts(&agreement, EscrowState::Refunded, 0),
            NativeRefundObservation::Absent,
            RefundObservationV1::Unstable,
        ),
    ] {
        let request_id = format!("refund-absence-{suffix}");
        let clock = refund_clock();
        let transport = RefundTransport::new([ObserveNativeRefundResult::new(
            refund_context(Participant::Taker, &request_id),
            clock,
            accounts,
            refund,
            clock,
        )]);
        let adapter = refund_adapter(transport, &agreement, Participant::Taker);
        assert_eq!(
            adapter
                .observe_prepared_native_refund(
                    &agreement,
                    RequestId::new(request_id).expect("request id"),
                    &prepared,
                    window,
                )
                .await
                .expect("absence is conservatively typed"),
            expected
        );
    }
}

#[tokio::test]
async fn refund_transport_attempts_are_once_and_submit_uncertainty_is_never_rejection() {
    let agreement = agreement();
    for (suffix, behavior) in [
        ("transport", RefundBehavior::FailPrepare),
        ("context", RefundBehavior::WrongPrepareContext),
        ("identity", RefundBehavior::ZeroPreparedId),
    ] {
        let transport = RefundTransport::new([]).with_behavior(behavior);
        let adapter = refund_adapter(transport.clone(), &agreement, Participant::Taker);
        assert!(
            adapter
                .prepare_native_refund(
                    &agreement,
                    RequestId::new(format!("refund-prepare-{suffix}")).expect("request id"),
                )
                .await
                .is_err()
        );
        assert_eq!(transport.prepare_requests.lock().expect("log").len(), 1);
    }

    let observe_transport = RefundTransport::new([]).with_behavior(RefundBehavior::FailObserve);
    let observe_adapter = refund_adapter(observe_transport.clone(), &agreement, Participant::Taker);
    assert!(matches!(
        observe_adapter
            .observe_prepared_native_refund(
                &agreement,
                RequestId::new("refund-observe-transport").expect("request id"),
                &prepared_refund_submission(),
                DiscoveryWindow::new(10, 3).expect("window"),
            )
            .await,
        Err(NativeRefundAdapterError::Transport(FakeError))
    ));
    assert_eq!(
        observe_transport
            .observe_requests
            .lock()
            .expect("log")
            .len(),
        1
    );

    for (suffix, behavior) in [
        ("transport", RefundBehavior::FailSubmit),
        ("context", RefundBehavior::WrongSubmitContext),
        ("identity", RefundBehavior::WrongSubmitId),
    ] {
        let transport = RefundTransport::new([]).with_behavior(behavior);
        let adapter = refund_adapter(transport.clone(), &agreement, Participant::Taker);
        let outcome = adapter
            .submit_native_refund(
                &agreement,
                RequestId::new(format!("refund-submit-{suffix}")).expect("request id"),
                &prepared_refund_submission(),
            )
            .await
            .expect("unknown delivery is a typed outcome");
        assert_eq!(outcome, RefundSubmitOutcomeV1::Unknown);
        assert_ne!(outcome, RefundSubmitOutcomeV1::DefinitivelyRejected);
        assert_eq!(transport.submit_requests.lock().expect("log").len(), 1);
    }
}

#[tokio::test]
async fn wrong_refund_step_and_runtime_terms_fail_before_transport() {
    let agreement = agreement();
    let wrong_step =
        PreparedRefundSubmissionV1::new(RefundStepV1::Zcash, [0x33; 32], vec![0xee, 0xff])
            .expect("independently valid wrong step");
    let transport = RefundTransport::new([]);
    let adapter = refund_adapter(transport.clone(), &agreement, Participant::Taker);
    assert!(matches!(
        adapter
            .submit_native_refund(
                &agreement,
                RequestId::new("refund-wrong-step").expect("request id"),
                &wrong_step,
            )
            .await,
        Err(NativeRefundAdapterError::WrongPreparedStep)
    ));
    assert!(transport.submit_requests.lock().expect("log").is_empty());

    for (mutation, expected) in [
        (RuntimeMutation::Channel, "chain"),
        (RuntimeMutation::Genesis, "chain"),
        (RuntimeMutation::Program, "program"),
        (RuntimeMutation::Signer, "signer"),
    ] {
        let transport = RefundTransport::new([]);
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
            .observe_native_refund_eligibility(
                &agreement,
                RequestId::new(format!("refund-runtime-{expected}")).expect("request id"),
            )
            .await
            .expect_err("runtime drift fails closed");
        match expected {
            "chain" => assert!(matches!(
                error,
                NativeRefundAdapterError::ChainIdentityMismatch
            )),
            "program" => assert!(matches!(
                error,
                NativeRefundAdapterError::EscrowProgramMismatch
            )),
            "signer" => assert!(matches!(
                error,
                NativeRefundAdapterError::SignerAccountMismatch
            )),
            _ => unreachable!("fixed case"),
        }
        assert!(transport.observe_requests.lock().expect("log").is_empty());
    }

    for (unsupported, expected) in [
        (
            agreement_for(LezEnvironmentV1::DeterministicLocalV0_2, false),
            "environment",
        ),
        (
            agreement_for(
                LezEnvironmentV1::DeterministicLocalV0_1_2Compatibility,
                true,
            ),
            "asset",
        ),
    ] {
        let transport = RefundTransport::new([]);
        let adapter = refund_adapter(transport.clone(), &unsupported, Participant::Taker);
        let error = adapter
            .observe_native_refund_eligibility(
                &unsupported,
                RequestId::new(format!("refund-unsupported-{expected}")).expect("request id"),
            )
            .await
            .expect_err("unsupported signed terms fail closed");
        match expected {
            "environment" => assert!(matches!(
                error,
                NativeRefundAdapterError::IncompatibleEnvironment
            )),
            "asset" => assert!(matches!(error, NativeRefundAdapterError::UnsupportedAsset)),
            _ => unreachable!("fixed case"),
        }
        assert!(transport.observe_requests.lock().expect("log").is_empty());
    }
}

#[derive(Clone, Copy, Debug)]
enum RefundMutation {
    ResponseContext,
    ClockHash,
    ClockHeight,
    ClockTimestamp,
    MetadataTerms,
    MetadataStatus,
    CustodyAccount,
    CustodyOwner,
    CustodyBalance,
    RefundId,
    RefundBytes,
    NonPublic,
    Signer,
    Program,
    Accounts,
    SwapId,
    OutsideWindow,
    AboveTip,
    SameHeightWrongHash,
    BeforeDeadline,
    DepthOverflow,
}

const ALL_REFUND_MUTATIONS: [RefundMutation; 21] = [
    RefundMutation::ResponseContext,
    RefundMutation::ClockHash,
    RefundMutation::ClockHeight,
    RefundMutation::ClockTimestamp,
    RefundMutation::MetadataTerms,
    RefundMutation::MetadataStatus,
    RefundMutation::CustodyAccount,
    RefundMutation::CustodyOwner,
    RefundMutation::CustodyBalance,
    RefundMutation::RefundId,
    RefundMutation::RefundBytes,
    RefundMutation::NonPublic,
    RefundMutation::Signer,
    RefundMutation::Program,
    RefundMutation::Accounts,
    RefundMutation::SwapId,
    RefundMutation::OutsideWindow,
    RefundMutation::AboveTip,
    RefundMutation::SameHeightWrongHash,
    RefundMutation::BeforeDeadline,
    RefundMutation::DepthOverflow,
];

#[tokio::test]
async fn refund_primitive_identity_account_clock_window_and_depth_mutations_fail_closed() {
    let agreement = agreement();
    let window = DiscoveryWindow::new(10, 3).expect("window");
    let prepared = prepared_refund_submission();
    for mutation in ALL_REFUND_MUTATIONS {
        let mut response = refund_found_observation(
            &agreement,
            refund_context(Participant::Taker, "refund-mutated"),
        );
        mutate_refund_observation(&mut response, mutation);
        let transport = RefundTransport::new([response]);
        let adapter = refund_adapter(transport.clone(), &agreement, Participant::Taker);
        let result = adapter
            .observe_prepared_native_refund(
                &agreement,
                RequestId::new("refund-mutated").expect("request id"),
                &prepared,
                window,
            )
            .await;
        assert!(result.is_err(), "mutation {mutation:?} must fail closed");
        assert_eq!(transport.observe_requests.lock().expect("log").len(), 1);
    }
}

#[test]
fn signed_profile_rejects_insufficient_zero_confirmation_refund_evidence() {
    let agreement = agreement();
    assert!(matches!(
        RefundEvidenceV1::new(
            &agreement,
            RefundStepV1::Lez,
            [0x33; 32],
            "33".repeat(32),
            lez_swap_core::ChainPosition::lez_timestamp_from_milliseconds_floor(
                LezUnixMilliseconds::new(200_000),
            ),
            0,
        ),
        Err(RefundError::InsufficientConfirmations {
            step: RefundStepV1::Lez,
            required: 1,
            actual: 0,
        })
    ));
}

fn refund_adapter(
    transport: RefundTransport,
    agreement: &ZecAgreementV1,
    participant: Participant,
) -> LezBridgeAdapter<RefundTransport> {
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

fn refund_context(participant: Participant, request_id: &str) -> MessageContext {
    MessageContext::new(
        RunId::new("native-run-0001").expect("run id"),
        RequestId::new(request_id).expect("request id"),
        match participant {
            Participant::Maker => BridgeParticipant::Maker,
            Participant::Taker => BridgeParticipant::Taker,
        },
    )
}

fn prepared_refund_transaction() -> PreparedTransaction {
    PreparedTransaction::new(
        TransactionId::from_bytes([0x33; 32]),
        ExactTransactionBytes::new(vec![0xee, 0xff]).expect("refund bytes"),
    )
}

fn prepared_refund_submission() -> PreparedRefundSubmissionV1 {
    PreparedRefundSubmissionV1::new(RefundStepV1::Lez, [0x33; 32], vec![0xee, 0xff])
        .expect("durable refund")
}

fn refund_clock() -> ChainClock {
    ChainClock::new(Hex32::from_bytes([0x90; 32]), 12, 200_000)
}

fn refund_accounts(
    agreement: &ZecAgreementV1,
    status: EscrowState,
    balance: u128,
) -> NativeEscrowAccountObservation {
    let terms = native_terms(agreement);
    NativeEscrowAccountObservation::found(NativeEscrowAccountFacts::new(
        EscrowMetadataFacts::from_native_terms(
            Hex32::from_bytes(*agreement.lez_terms().metadata_account()),
            Hex32::from_bytes(program_bytes(agreement.lez_terms().escrow_program_id())),
            Hex32::from_bytes(*agreement.lez_terms().custody_account()),
            &terms,
            status,
        ),
        NativeCustodyFacts::new(
            Hex32::from_bytes(*agreement.lez_terms().custody_account()),
            terms.authenticated_transfer_program_id(),
            balance,
        ),
    ))
}

fn refund_state_observation(
    agreement: &ZecAgreementV1,
    context: MessageContext,
) -> ObserveNativeRefundResult {
    let clock = refund_clock();
    ObserveNativeRefundResult::new(
        context,
        clock,
        refund_accounts(
            agreement,
            EscrowState::Funded,
            agreement.lez_terms().amount(),
        ),
        NativeRefundObservation::NotRequested,
        clock,
    )
}

fn refund_found_observation(
    agreement: &ZecAgreementV1,
    context: MessageContext,
) -> ObserveNativeRefundResult {
    let clock = refund_clock();
    let metadata = Hex32::from_bytes(*agreement.lez_terms().metadata_account());
    let custody = Hex32::from_bytes(*agreement.lez_terms().custody_account());
    let depositor = Hex32::from_bytes(*agreement.lez_account(agreement.lez_depositor()));
    let program = Hex32::from_bytes(program_bytes(agreement.lez_terms().escrow_program_id()));
    ObserveNativeRefundResult::new(
        context,
        clock,
        refund_accounts(agreement, EscrowState::Refunded, 0),
        NativeRefundObservation::found(NativeRefundFoundFacts::new(
            ObservedTransactionFacts::new(
                TransactionId::from_bytes([0x33; 32]),
                ExactTransactionBytes::new(vec![0xee, 0xff]).expect("refund bytes"),
                ChainPosition::new(Hex32::from_bytes([0x82; 32]), 11, 0),
                AccountIds::new(Vec::new()).expect("empty official witness set"),
                true,
            ),
            NativeRefundInstructionFacts::new(
                program,
                AccountIds::new(vec![metadata, custody, depositor]).expect("refund accounts"),
                Hex32::from_bytes(*agreement.onchain_swap_id()),
            ),
        )),
        clock,
    )
}

#[allow(clippy::too_many_lines)]
fn mutate_refund_observation(response: &mut ObserveNativeRefundResult, mutation: RefundMutation) {
    let NativeEscrowAccountObservation::Found(accounts) = &mut response.accounts else {
        panic!("canonical refund has account facts")
    };
    let NativeRefundObservation::Found(refund) = &mut response.refund else {
        panic!("canonical refund has transaction facts")
    };
    match mutation {
        RefundMutation::ResponseContext => {
            response.context.request_id = RequestId::new("wrong-refund-context").expect("id");
        }
        RefundMutation::ClockHash => {
            response.clock_after.block_hash = Hex32::from_bytes([0x91; 32]);
        }
        RefundMutation::ClockHeight => response.clock_after.height += 1,
        RefundMutation::ClockTimestamp => response.clock_after.timestamp_ms += 1,
        RefundMutation::MetadataTerms => {
            accounts.metadata.terms_hash = Hex32::from_bytes([0x92; 32]);
        }
        RefundMutation::MetadataStatus => accounts.metadata.status = EscrowState::Funded,
        RefundMutation::CustodyAccount => {
            accounts.custody.account_id = Hex32::from_bytes([0x93; 32]);
        }
        RefundMutation::CustodyOwner => {
            accounts.custody.owner_program_id = Hex32::from_bytes([0x94; 32]);
        }
        RefundMutation::CustodyBalance => accounts.custody.balance = NativeAmount::new(1),
        RefundMutation::RefundId => {
            refund.transaction.transaction_id = TransactionId::from_bytes([0x95; 32]);
        }
        RefundMutation::RefundBytes => {
            refund.transaction.exact_bytes =
                ExactTransactionBytes::new(vec![0xde, 0xad]).expect("changed bytes");
        }
        RefundMutation::NonPublic => refund.transaction.is_public = false,
        RefundMutation::Signer => {
            refund.transaction.signer_account_ids =
                AccountIds::new(vec![Hex32::from_bytes([0x96; 32])]).expect("signer");
        }
        RefundMutation::Program => {
            refund.instruction.program_id = Hex32::from_bytes([0x97; 32]);
        }
        RefundMutation::Accounts => {
            refund.instruction.ordered_account_ids =
                AccountIds::new(vec![Hex32::from_bytes([0x98; 32])]).expect("accounts");
        }
        RefundMutation::SwapId => {
            refund.instruction.swap_id = Hex32::from_bytes([0x99; 32]);
        }
        RefundMutation::OutsideWindow => refund.transaction.position.height = 9,
        RefundMutation::AboveTip => refund.transaction.position.height = 13,
        RefundMutation::SameHeightWrongHash => refund.transaction.position.height = 12,
        RefundMutation::BeforeDeadline => {
            response.clock_before.timestamp_ms = 159_999;
            response.clock_after.timestamp_ms = 159_999;
        }
        RefundMutation::DepthOverflow => {
            response.clock_before.height = u64::MAX;
            response.clock_after.height = u64::MAX;
        }
    }
}

#[derive(Clone, Debug)]
struct ObservationTransport {
    requests: Arc<Mutex<Vec<ObserveEscrowRequest>>>,
    submit_requests: Arc<Mutex<Vec<SubmitTransactionRequest>>>,
    responses: Arc<Mutex<VecDeque<ObserveEscrowResult>>>,
    attempts: Arc<AtomicUsize>,
    fail_submit: bool,
}

impl ObservationTransport {
    fn new(response: ObserveEscrowResult) -> Self {
        Self::from_responses([response])
    }

    fn from_responses(responses: impl IntoIterator<Item = ObserveEscrowResult>) -> Self {
        Self {
            requests: Arc::default(),
            submit_requests: Arc::default(),
            responses: Arc::new(Mutex::new(responses.into_iter().collect())),
            attempts: Arc::default(),
            fail_submit: false,
        }
    }

    fn failing_submit(mut self) -> Self {
        self.fail_submit = true;
        self
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
        self.responses
            .lock()
            .expect("responses")
            .pop_front()
            .ok_or(FakeError)
    }
}

#[async_trait]
impl LezBridgeFirstLockTransport for ObservationTransport {
    type Error = FakeError;

    async fn observe_escrow(
        &self,
        request: ObserveEscrowRequest,
    ) -> Result<ObserveEscrowResult, Self::Error> {
        self.attempts.fetch_add(1, Ordering::SeqCst);
        self.requests.lock().expect("request log").push(request);
        self.responses
            .lock()
            .expect("responses")
            .pop_front()
            .ok_or(FakeError)
    }

    async fn submit_transaction(
        &self,
        request: SubmitTransactionRequest,
    ) -> Result<SubmitTransactionResult, Self::Error> {
        self.submit_requests
            .lock()
            .expect("submit request log")
            .push(request.clone());
        if self.fail_submit {
            return Err(FakeError);
        }
        Ok(SubmitTransactionResult::new(
            request.context,
            request.transaction.transaction_id,
            SubmissionOutcome::Accepted,
        ))
    }
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn sqlite_wrapper_never_submits_fund_before_canonical_initialization() {
    let agreement = agreement();
    let plan = native_first_lock_plan();
    let FirstLockPlanV1::Lez { initialize, fund } = &plan else {
        panic!("native LEZ plan")
    };
    let store_path = isolated_sqlite_path("lez-wrapper-first-lock-store");
    let journal_path = isolated_sqlite_path("lez-wrapper-first-lock-journal");
    let store = SqliteZecRecoveryStore::open(&store_path, Participant::Taker)
        .expect("open production recovery store");
    stage_native_first_lock_plan(&store, &agreement, plan.clone()).await;

    let mut before_initialization = canonical_observation(
        &agreement,
        observation_context(Participant::Taker, "wrapper-fund-before-init"),
    );
    before_initialization.initialization = InitializationObservation::UnknownOrPending;
    before_initialization.funding = FundingObservation::UnknownOrPending;
    let mut initialization_found = initialization_only_observation(
        &agreement,
        observation_context(Participant::Taker, "wrapper-init-found"),
    );
    initialization_found.funding = FundingObservation::UnknownOrPending;
    let mut funding_unknown = initialization_only_observation(
        &agreement,
        observation_context(Participant::Taker, "wrapper-fund-unknown"),
    );
    funding_unknown.funding = FundingObservation::UnknownOrPending;
    let transport = ObservationTransport::from_responses([
        before_initialization,
        initialization_found,
        funding_unknown,
    ]);
    let opens = Arc::new(AtomicUsize::new(0));
    let allocation_calls = Arc::new(AtomicUsize::new(0));
    let contexts = QueuedContexts {
        requests: Arc::new(Mutex::new(VecDeque::from([
            BridgeRequestSpec::new(
                RequestId::new("wrapper-fund-before-init").expect("request ID"),
                None,
            ),
            BridgeRequestSpec::new(
                RequestId::new("wrapper-init-found").expect("request ID"),
                None,
            ),
            BridgeRequestSpec::new(
                RequestId::new("wrapper-fund-unknown").expect("request ID"),
                None,
            ),
            BridgeRequestSpec::new(
                RequestId::new("wrapper-observe-next").expect("request ID"),
                None,
            ),
            BridgeRequestSpec::new(
                RequestId::new("wrapper-fund-submit").expect("request ID"),
                None,
            ),
        ]))),
        calls: Arc::clone(&allocation_calls),
    };
    let ports = ContextOwningLezBridgePorts::new(
        RunId::new("native-run-0001").expect("run ID"),
        runtime_for_participant(&agreement, Participant::Taker),
        Participant::Taker,
        CloneTransportFactory {
            transport: transport.clone(),
            opens: Arc::clone(&opens),
        },
        contexts,
        store.clone(),
        (),
        SqliteBridgeOperationJournal::open(&journal_path).expect("operation journal"),
    )
    .expect("role-local wrapper");

    assert_eq!(
        ports
            .observe_first_lock(&agreement, fund)
            .await
            .expect("fund-before-init observation"),
        FirstLockObservation::Unstable
    );
    assert!(
        transport
            .submit_requests
            .lock()
            .expect("submit log")
            .is_empty()
    );
    assert!(matches!(
        ports
            .observe_first_lock(&agreement, initialize)
            .await
            .expect("canonical initialization"),
        FirstLockObservation::Confirmed(evidence)
            if evidence.step() == FirstLockStepV1::LezInitialize
    ));
    assert_eq!(
        ports
            .observe_first_lock(&agreement, fund)
            .await
            .expect("production exact funding miss"),
        FirstLockObservation::Absent
    );
    ports
        .submit_first_lock(&agreement, fund)
        .await
        .expect("submit exact durable funding bytes");

    let observations = transport.requests.lock().expect("observation log");
    assert_eq!(observations.len(), 3);
    assert_eq!(
        observations[0].context.request_id,
        RequestId::new("wrapper-fund-before-init").expect("request ID")
    );
    assert_eq!(
        observations[1].context.request_id,
        RequestId::new("wrapper-init-found").expect("request ID")
    );
    assert_eq!(
        observations[2].context.request_id,
        RequestId::new("wrapper-fund-unknown").expect("request ID")
    );
    drop(observations);
    let submissions = transport.submit_requests.lock().expect("submit log");
    assert_eq!(submissions.len(), 1);
    assert_eq!(
        submissions[0].context.request_id,
        RequestId::new("wrapper-fund-submit").expect("request ID")
    );
    assert_eq!(
        submissions[0].transaction.transaction_id.as_bytes(),
        fund.expected_submission_id()
    );
    assert_eq!(
        submissions[0].transaction.exact_bytes.as_slice(),
        fund.exact_submission()
    );
    assert_eq!(allocation_calls.load(Ordering::SeqCst), 5);
    assert_eq!(opens.load(Ordering::SeqCst), 4);
    drop(submissions);
    drop(ports);
    drop(store);
    remove_sqlite_files(&journal_path);
    remove_sqlite_files(&store_path);
}

#[tokio::test]
async fn owner_step_observation_requires_the_complete_durable_lez_plan() {
    let agreement = agreement();
    let context = observation_context(Participant::Taker, "observe-step-0001");
    let transport = ObservationTransport::new(canonical_observation(&agreement, context));
    let adapter = observation_adapter(transport.clone(), &agreement, Participant::Taker);
    let initialize = PreparedFirstLockSubmissionV1::new(
        FirstLockStepV1::LezInitialize,
        [0x11; 32],
        vec![0xa1, 0xb1],
    )
    .expect("initialization");
    let fund =
        PreparedFirstLockSubmissionV1::new(FirstLockStepV1::LezFund, [0x22; 32], vec![0xa2, 0xb2])
            .expect("funding");
    let plan = FirstLockPlanV1::lez(initialize.clone(), fund).expect("complete LEZ plan");

    let observed = adapter
        .observe_native_first_lock_step(
            &agreement,
            RequestId::new("observe-step-0001").expect("request id"),
            &plan,
            &initialize,
        )
        .await
        .expect("exact initialization observation");

    let FirstLockObservation::Confirmed(evidence) = observed else {
        panic!("the canonical initialization must be confirmed")
    };
    assert_eq!(evidence.step(), FirstLockStepV1::LezInitialize);
    assert_eq!(evidence.expected_submission_id(), &[0x11; 32]);
    assert_eq!(transport.attempts.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn initialization_confirmation_precedes_a_stably_absent_funding_step() {
    let agreement = agreement();
    let plan = native_first_lock_plan();
    let FirstLockPlanV1::Lez { initialize, fund } = &plan else {
        panic!("native LEZ plan")
    };

    let init_context = observation_context(Participant::Taker, "observe-init-only");
    let init_response = initialization_only_observation(&agreement, init_context);
    let init_adapter = observation_adapter(
        ObservationTransport::new(init_response),
        &agreement,
        Participant::Taker,
    );
    assert!(matches!(
        init_adapter
            .observe_native_first_lock_step(
                &agreement,
                RequestId::new("observe-init-only").expect("request id"),
                &plan,
                initialize,
            )
            .await
            .expect("stable initialization"),
        FirstLockObservation::Confirmed(evidence)
            if evidence.step() == FirstLockStepV1::LezInitialize
    ));

    let fund_context = observation_context(Participant::Taker, "observe-fund-absent");
    let fund_response = initialization_only_observation(&agreement, fund_context);
    let fund_adapter = observation_adapter(
        ObservationTransport::new(fund_response),
        &agreement,
        Participant::Taker,
    );
    assert_eq!(
        fund_adapter
            .observe_native_first_lock_step(
                &agreement,
                RequestId::new("observe-fund-absent").expect("request id"),
                &plan,
                fund,
            )
            .await
            .expect("stable funding absence"),
        FirstLockObservation::Absent
    );
}

#[tokio::test]
async fn production_exact_unknown_shapes_progress_only_in_protocol_order() {
    let agreement = agreement();
    let plan = native_first_lock_plan();
    let FirstLockPlanV1::Lez { initialize, fund } = &plan else {
        panic!("native LEZ plan")
    };

    let mut no_steps = canonical_observation(
        &agreement,
        observation_context(Participant::Taker, "observe-no-steps"),
    );
    no_steps.initialization = InitializationObservation::UnknownOrPending;
    no_steps.funding = FundingObservation::UnknownOrPending;
    let no_steps_adapter = observation_adapter(
        ObservationTransport::new(no_steps.clone()),
        &agreement,
        Participant::Taker,
    );
    assert_eq!(
        no_steps_adapter
            .observe_native_first_lock_step(
                &agreement,
                RequestId::new("observe-no-steps").expect("request id"),
                &plan,
                initialize,
            )
            .await
            .expect("exact init miss is safe to replay"),
        FirstLockObservation::Absent
    );
    no_steps.context = observation_context(Participant::Taker, "observe-fund-before-init");
    let fund_before_init = observation_adapter(
        ObservationTransport::new(no_steps),
        &agreement,
        Participant::Taker,
    );
    assert_eq!(
        fund_before_init
            .observe_native_first_lock_step(
                &agreement,
                RequestId::new("observe-fund-before-init").expect("request id"),
                &plan,
                fund,
            )
            .await
            .expect("fund cannot advance before initialization"),
        FirstLockObservation::Unstable
    );

    let mut init_found = initialization_only_observation(
        &agreement,
        observation_context(Participant::Taker, "observe-fund-production-miss"),
    );
    init_found.funding = FundingObservation::UnknownOrPending;
    let fund_after_init = observation_adapter(
        ObservationTransport::new(init_found),
        &agreement,
        Participant::Taker,
    );
    assert_eq!(
        fund_after_init
            .observe_native_first_lock_step(
                &agreement,
                RequestId::new("observe-fund-production-miss").expect("request id"),
                &plan,
                fund,
            )
            .await
            .expect("validated init makes exact fund replay safe"),
        FirstLockObservation::Absent
    );
}

#[tokio::test]
async fn step_observation_rejects_a_substituted_sibling_before_transport() {
    let agreement = agreement();
    let plan = native_first_lock_plan();
    let substituted = PreparedFirstLockSubmissionV1::new(
        FirstLockStepV1::LezInitialize,
        [0x11; 32],
        vec![0xff, 0xb1],
    )
    .expect("well-formed but substituted initialization");
    let context = observation_context(Participant::Taker, "observe-substituted-step");
    let transport = ObservationTransport::new(canonical_observation(&agreement, context));
    let adapter = observation_adapter(transport.clone(), &agreement, Participant::Taker);

    let error = adapter
        .observe_native_first_lock_step(
            &agreement,
            RequestId::new("observe-substituted-step").expect("request id"),
            &plan,
            &substituted,
        )
        .await
        .expect_err("substituted sibling is not in the durable plan");

    assert!(matches!(
        error,
        ObserveNativeEscrowError::PreparedPlanMismatch
    ));
    assert_eq!(transport.attempts.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn funding_never_confirms_without_canonical_initialization() {
    let agreement = agreement();
    let plan = native_first_lock_plan();
    let FirstLockPlanV1::Lez { fund, .. } = &plan else {
        panic!("native LEZ plan")
    };
    let context = observation_context(Participant::Taker, "observe-missing-init");
    let mut response = canonical_observation(&agreement, context);
    response.initialization = InitializationObservation::Absent;
    let adapter = observation_adapter(
        ObservationTransport::new(response),
        &agreement,
        Participant::Taker,
    );

    assert!(matches!(
        adapter
            .observe_native_first_lock_step(
                &agreement,
                RequestId::new("observe-missing-init").expect("request id"),
                &plan,
                fund,
            )
            .await,
        Err(ObserveNativeEscrowError::InconsistentFacts)
    ));
}

#[tokio::test]
async fn first_lock_submit_selects_exact_step_and_keeps_ambiguity_unknown() {
    let agreement = agreement();
    let plan = native_first_lock_plan();
    let FirstLockPlanV1::Lez { initialize, fund } = &plan else {
        panic!("native LEZ plan")
    };
    let context = observation_context(Participant::Taker, "unused-observation");
    let transport = ObservationTransport::new(canonical_observation(&agreement, context));
    let adapter = observation_adapter(transport.clone(), &agreement, Participant::Taker);

    assert_eq!(
        adapter
            .submit_native_first_lock_step(
                &agreement,
                RequestId::new("submit-init-exact").expect("request id"),
                &plan,
                initialize,
            )
            .await
            .expect("initialization submit"),
        NativeFirstLockSubmitOutcome::Accepted
    );
    let request = transport
        .submit_requests
        .lock()
        .expect("submit request log")
        .last()
        .expect("one submit")
        .clone();
    assert_eq!(request.transaction.transaction_id.as_bytes(), &[0x11; 32]);
    assert_eq!(request.transaction.exact_bytes.as_slice(), &[0xa1, 0xb1]);

    let failing = ObservationTransport::new(canonical_observation(
        &agreement,
        observation_context(Participant::Taker, "unused-failing-observation"),
    ))
    .failing_submit();
    let failing_adapter = observation_adapter(failing, &agreement, Participant::Taker);
    assert_eq!(
        failing_adapter
            .submit_native_first_lock_step(
                &agreement,
                RequestId::new("submit-fund-unknown").expect("request id"),
                &plan,
                fund,
            )
            .await
            .expect("ambiguous submit is an outcome"),
        NativeFirstLockSubmitOutcome::Unknown
    );
}

#[tokio::test]
async fn reverse_taker_discovers_the_maker_funded_lez_lock() {
    let agreement = agreement_for_direction(
        LezEnvironmentV1::DeterministicLocalV0_1_2Compatibility,
        false,
        SwapDirection::TakerSellsForeign,
    );
    let context = observation_context(Participant::Taker, "observe-reverse-maker");
    let transport = ObservationTransport::new(canonical_observation(&agreement, context));
    let adapter = observation_adapter(transport, &agreement, Participant::Taker);

    let observed = adapter
        .observe_native_maker_lock(
            &agreement,
            RequestId::new("observe-reverse-maker").expect("request id"),
            DiscoveryWindow::new(9, 4).expect("covered discovery window"),
        )
        .await
        .expect("canonical reverse maker lock");

    assert!(matches!(
        observed,
        lez_zec_swap_sdk::MakerLockObservationV1::Confirmed(evidence)
            if evidence.step() == FirstLockStepV1::LezFund
                && evidence.expected_submission_id() == &[0x22; 32]
    ));
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
    let depositor = agreement.lez_depositor();
    let claimant = agreement.lez_claimant();
    NativeEscrowTerms::new(NativeEscrowTermsInput {
        swap_id: Hex32::from_bytes(*agreement.onchain_swap_id()),
        terms_hash: Hex32::from_bytes(*agreement.agreement_commitment()),
        secret_digest: Hex32::from_bytes(*agreement.secret_digest()),
        depositor: match depositor {
            Participant::Maker => BridgeParticipant::Maker,
            Participant::Taker => BridgeParticipant::Taker,
        },
        depositor_account_id: Hex32::from_bytes(*agreement.lez_account(depositor)),
        claimant: match claimant {
            Participant::Maker => BridgeParticipant::Maker,
            Participant::Taker => BridgeParticipant::Taker,
        },
        claimant_account_id: Hex32::from_bytes(*agreement.lez_account(claimant)),
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
    let depositor = Hex32::from_bytes(*agreement.lez_account(agreement.lez_depositor()));
    let claimant = Hex32::from_bytes(*agreement.lez_account(agreement.lez_claimant()));
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

fn native_first_lock_plan() -> FirstLockPlanV1 {
    FirstLockPlanV1::lez(
        PreparedFirstLockSubmissionV1::new(
            FirstLockStepV1::LezInitialize,
            [0x11; 32],
            vec![0xa1, 0xb1],
        )
        .expect("initialization"),
        PreparedFirstLockSubmissionV1::new(FirstLockStepV1::LezFund, [0x22; 32], vec![0xa2, 0xb2])
            .expect("funding"),
    )
    .expect("complete LEZ plan")
}

fn initialization_only_observation(
    agreement: &ZecAgreementV1,
    context: MessageContext,
) -> ObserveEscrowResult {
    let mut response = canonical_observation(agreement, context);
    let InitializationObservation::Found(initialization) = &mut response.initialization else {
        panic!("canonical fixture has initialization")
    };
    initialization.metadata = EscrowMetadataFacts::from_native_terms(
        Hex32::from_bytes(*agreement.lez_terms().metadata_account()),
        Hex32::from_bytes(program_bytes(agreement.lez_terms().escrow_program_id())),
        Hex32::from_bytes(*agreement.lez_terms().custody_account()),
        &native_terms(agreement),
        EscrowState::Empty,
    );
    response.funding = FundingObservation::Absent;
    response
}

fn agreement() -> ZecAgreementV1 {
    agreement_for(
        LezEnvironmentV1::DeterministicLocalV0_1_2Compatibility,
        false,
    )
}

#[allow(clippy::too_many_lines)]
fn agreement_for(environment: LezEnvironmentV1, token: bool) -> ZecAgreementV1 {
    agreement_for_direction(environment, token, SwapDirection::TakerSellsLez)
}

#[allow(clippy::too_many_lines)]
fn agreement_for_direction(
    environment: LezEnvironmentV1,
    token: bool,
    direction: SwapDirection,
) -> ZecAgreementV1 {
    let maker_secret = SecretKey::from_slice(&[1; 32]).expect("maker key");
    let taker_secret = SecretKey::from_slice(&[2; 32]).expect("taker key");
    let secp = Secp256k1::new();
    let maker_key = PublicKey::from_secret_key(&secp, &maker_secret).serialize();
    let taker_key = PublicKey::from_secret_key(&secp, &taker_secret).serialize();
    let (refund_hash, claimant_hash) = match direction {
        SwapDirection::TakerSellsLez => (pubkey_hash(&maker_key), pubkey_hash(&taker_key)),
        SwapDirection::TakerSellsForeign => (pubkey_hash(&taker_key), pubkey_hash(&maker_key)),
    };
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
    let id = match (environment, token, direction) {
        (
            LezEnvironmentV1::DeterministicLocalV0_1_2Compatibility,
            false,
            SwapDirection::TakerSellsForeign,
        ) => "lez-bridge-native-reverse-test",
        (LezEnvironmentV1::DeterministicLocalV0_1_2Compatibility, false, _) => {
            "lez-bridge-native-test"
        }
        (LezEnvironmentV1::DeterministicLocalV0_1_2Compatibility, true, _) => {
            "lez-bridge-token-test"
        }
        (LezEnvironmentV1::DeterministicLocalV0_2, false, _) => "lez-bridge-v02-test",
        (LezEnvironmentV1::PublicTestnetV0_2, _, _)
        | (LezEnvironmentV1::DeterministicLocalV0_2, true, _) => {
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
        direction,
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
