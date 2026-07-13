//! Crash-safe SDK-port composition for one role-local official LEZ sidecar.

use std::{
    error::Error,
    fmt,
    sync::{Mutex, MutexGuard},
};

use async_trait::async_trait;
use lez_bridge_protocol::{
    EscrowObservationTarget, Hex32, RunId, RuntimeDescriptor, TransactionId,
};
use lez_swap_core::Participant;
use lez_swap_store::{
    BridgeObservationOutcome, BridgeOperationKey, BridgeOperationKind, BridgeRequestSpec,
    DurableBridgeRequestContext, SqliteBridgeOperationJournal, StoreError,
};
use lez_zec_swap_sdk::{
    CanonicalLezEscrowObservationV1, ClaimPreimage, FirstLockConfirmedEvidenceV1,
    FirstLockObservation, FirstLockPlanV1, FirstLockStepV1, LezClaimPort, LezFirstLockPort,
    LezMakerLockObservationPort, LezRefundPort, LezTakerFirstLockObservationPort,
    MakerLockObservationV1, PreparedClaimSubmissionV1, PreparedFirstLockSubmissionV1,
    PreparedRefundSubmissionV1, RecoveryStore, RefundEligibilityObservationV1, RefundObservationV1,
    RefundSubmitOutcomeV1, RevealingClaimObservationV1, TakerFirstLockObservationV1,
    ZecAgreementV1,
};
use thiserror::Error;

use crate::{
    LezBridgeAdapter, LezBridgeClaimTransport, LezBridgeConfigurationError,
    LezBridgeFirstLockTransport, LezBridgeObservationTransport, LezBridgeRefundTransport,
    LezBridgeTransport, NativeFirstLockSubmitOutcome, NativeRefundAdapterError,
    NativeRevealingClaimAdapterError, ObserveNativeEscrowError, PrepareNativeFirstLockError,
    RevealingClaimSubmitOutcome, bridge_participant,
};

type BoxError = Box<dyn Error + Send + Sync + 'static>;

/// Supplies one caller-owned request ID and optional bounded discovery window.
///
/// Implementations normally draw these values from an actor-owned secure request
/// allocator. The adapter never derives, randomizes, or widens either value.
pub trait BridgeRequestContextSource: Send + Sync {
    /// Structured allocator error.
    type Error: Error + Send + Sync + 'static;

    /// Returns the next unused request specification for one logical operation.
    ///
    /// # Errors
    ///
    /// Returns the allocator's structured error when no fresh request is available.
    fn next_request(&self, key: &BridgeOperationKey) -> Result<BridgeRequestSpec, Self::Error>;
}

/// Creates a fresh one-use bridge transport for every sidecar attempt.
///
/// A fresh process/client is mandatory when an ambiguous operation resumes an
/// exact durable request ID because `BridgeClient` rejects in-process ID reuse.
pub trait FreshLezBridgeTransportFactory: Send + Sync {
    /// Fresh transport type.
    type Transport: Send + Sync;
    /// Structured factory or credential-loading error.
    type Error: Error + Send + Sync + 'static;

    /// Opens one fresh authenticated role-local transport.
    ///
    /// # Errors
    ///
    /// Returns the factory's structured error when the transport cannot be opened.
    fn fresh_transport(&self) -> Result<Self::Transport, Self::Error>;
}

/// Loads complete canonical LEZ funding evidence retained by the actor.
///
/// Claim preparation needs the already validated funding transaction identity,
/// but the SDK claim port intentionally accepts no primitive ID argument. This
/// is a trusted actor/store boundary: implementations must reconstruct and
/// revalidate the persisted agreement-bound remote-lock transition before
/// returning evidence. A caller-supplied primitive transaction ID is not valid.
/// The reference actor slice must mutation-test that concrete implementation.
#[async_trait]
pub trait CanonicalLezFundingSource: Send + Sync {
    /// Structured durable-source error.
    type Error: Error + Send + Sync + 'static;

    /// Returns canonical final-step evidence for this exact accepted agreement.
    async fn canonical_lez_funding(
        &self,
        agreement: &ZecAgreementV1,
    ) -> Result<FirstLockConfirmedEvidenceV1, Self::Error>;
}

/// Failure at the context-owning production SDK boundary.
#[derive(Debug, Error)]
pub enum ContextOwningLezPortError {
    /// The context allocator could not provide a caller-owned request.
    #[error("caller-owned LEZ bridge request context is unavailable")]
    Context(#[source] BoxError),
    /// The durable operation journal failed or rejected context drift.
    #[error("durable LEZ bridge operation journal failed")]
    Journal(#[source] StoreError),
    /// The role-local SDK recovery store failed.
    #[error("role-local LEZ recovery store failed")]
    Recovery(#[source] BoxError),
    /// A fresh authenticated transport could not be created.
    #[error("fresh role-local LEZ bridge transport is unavailable")]
    Factory(#[source] BoxError),
    /// The fresh transport could not be bound to the configured role/runtime.
    #[error("fresh LEZ bridge adapter configuration is invalid")]
    Configuration(#[source] LezBridgeConfigurationError),
    /// An explicit agreement-validating bridge operation failed.
    #[error("LEZ bridge adapter rejected the operation")]
    Adapter(#[source] BoxError),
    /// No complete durable LEZ initialize/fund plan exists for this actor.
    #[error("complete durable LEZ initialize/fund plan is missing")]
    MissingLezPlan,
    /// The durable plan belongs to another swap, agreement, or actor.
    #[error("durable LEZ initialize/fund plan context mismatch")]
    LezPlanContextMismatch,
    /// A discovery operation did not contain its caller-owned bounded window.
    #[error("LEZ bridge discovery context has no bounded window")]
    MissingDiscoveryWindow,
    /// An unstable or typed-error poll attempted to silently replace its scan window.
    #[error("LEZ bridge retry changed the active discovery window")]
    DiscoveryWindowChanged,
    /// Canonical funding evidence was missing, non-final-step, under-depth, or inconsistent.
    #[error("canonical LEZ funding context is invalid")]
    InvalidCanonicalFunding,
    /// Submission was attempted once but delivery remains ambiguous.
    #[error("LEZ bridge submission outcome is unknown; observe before retry")]
    UnknownSubmission,
    /// Another thread panicked while holding the role-local journal.
    #[error("durable LEZ bridge journal lock is poisoned")]
    JournalLockPoisoned,
}

/// Role-local composition implementing the SDK's context-free LEZ ports safely.
pub struct ContextOwningLezBridgePorts<Factory, Contexts, Store, Funding> {
    run_id: RunId,
    runtime: RuntimeDescriptor,
    local_participant: Participant,
    factory: Factory,
    contexts: Contexts,
    store: Store,
    funding: Funding,
    journal: Mutex<SqliteBridgeOperationJournal>,
}

impl<Factory, Contexts, Store, Funding> fmt::Debug
    for ContextOwningLezBridgePorts<Factory, Contexts, Store, Funding>
{
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ContextOwningLezBridgePorts")
            .field("run_id", &self.run_id)
            .field("runtime", &self.runtime)
            .field("local_participant", &self.local_participant)
            .field("factory", &"<redacted>")
            .field("contexts", &"<redacted>")
            .field("store", &"<redacted>")
            .field("funding", &"<redacted>")
            .field("journal", &"<redacted>")
            .finish()
    }
}

impl<Factory, Contexts, Store, Funding>
    ContextOwningLezBridgePorts<Factory, Contexts, Store, Funding>
{
    /// Binds one independent actor, store, context journal, and fresh-client factory.
    ///
    /// # Errors
    ///
    /// Rejects a sidecar runtime whose signing role differs from the local actor.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        run_id: RunId,
        runtime: RuntimeDescriptor,
        local_participant: Participant,
        factory: Factory,
        contexts: Contexts,
        store: Store,
        funding: Funding,
        journal: SqliteBridgeOperationJournal,
    ) -> Result<Self, LezBridgeConfigurationError> {
        if runtime.sidecar_role != bridge_participant(local_participant) {
            return Err(LezBridgeConfigurationError::SidecarRoleMismatch);
        }
        Ok(Self {
            run_id,
            runtime,
            local_participant,
            factory,
            contexts,
            store,
            funding,
            journal: Mutex::new(journal),
        })
    }

    fn journal(
        &self,
    ) -> Result<MutexGuard<'_, SqliteBridgeOperationJournal>, ContextOwningLezPortError> {
        self.journal
            .lock()
            .map_err(|_| ContextOwningLezPortError::JournalLockPoisoned)
    }

    fn key(
        &self,
        agreement: &ZecAgreementV1,
        operation: BridgeOperationKind,
    ) -> BridgeOperationKey {
        BridgeOperationKey::new(
            self.run_id.clone(),
            agreement.coordinator().id().clone(),
            self.local_participant,
            operation,
        )
    }
}

impl<Factory, Contexts, Store, Funding>
    ContextOwningLezBridgePorts<Factory, Contexts, Store, Funding>
where
    Contexts: BridgeRequestContextSource,
{
    fn reserve_context(
        &self,
        agreement: &ZecAgreementV1,
        operation: BridgeOperationKind,
    ) -> Result<(BridgeOperationKey, DurableBridgeRequestContext), ContextOwningLezPortError> {
        let key = self.key(agreement, operation);
        let mut journal = self.journal()?;
        if let Some(active) = journal
            .current(&key)
            .map_err(ContextOwningLezPortError::Journal)?
        {
            return Ok((key, active));
        }
        let requested = self
            .contexts
            .next_request(&key)
            .map_err(|error| ContextOwningLezPortError::Context(Box::new(error)))?;
        match journal.begin_or_resume(&key, &requested) {
            Ok(commit) => Ok((key, commit.context().clone())),
            Err(original) => {
                let probe = journal
                    .current(&key)
                    .map_err(ContextOwningLezPortError::Journal)?;
                if let Some(context) =
                    probe.filter(|context| context_matches_spec(context, &requested))
                {
                    Ok((key, context))
                } else {
                    Err(ContextOwningLezPortError::Journal(original))
                }
            }
        }
    }

    fn finish_observation(
        &self,
        key: &BridgeOperationKey,
        current: &DurableBridgeRequestContext,
        outcome: BridgeObservationOutcome,
        retain_window: bool,
    ) -> Result<(), ContextOwningLezPortError> {
        let next = self
            .contexts
            .next_request(key)
            .map_err(|error| ContextOwningLezPortError::Context(Box::new(error)))?;
        if retain_window && next.discovery_window() != current.discovery_window() {
            return Err(ContextOwningLezPortError::DiscoveryWindowChanged);
        }
        let mut journal = self.journal()?;
        match journal.advance_observation(key, current, outcome, &next) {
            Ok(_) => Ok(()),
            Err(_) => journal
                .advance_observation(key, current, outcome, &next)
                .map(|_| ())
                .map_err(ContextOwningLezPortError::Journal),
        }
    }

    fn resume_ambiguous(
        &self,
        key: &BridgeOperationKey,
        current: &DurableBridgeRequestContext,
    ) -> Result<(), ContextOwningLezPortError> {
        self.journal()?
            .resume_after_ambiguous(key, current)
            .map(|_| ())
            .map_err(ContextOwningLezPortError::Journal)
    }
}

impl<Factory, Contexts, Store, Funding>
    ContextOwningLezBridgePorts<Factory, Contexts, Store, Funding>
where
    Factory: FreshLezBridgeTransportFactory,
{
    fn fresh_adapter(
        &self,
    ) -> Result<LezBridgeAdapter<Factory::Transport>, ContextOwningLezPortError> {
        let transport = self
            .factory
            .fresh_transport()
            .map_err(|error| ContextOwningLezPortError::Factory(Box::new(error)))?;
        LezBridgeAdapter::new(
            transport,
            self.run_id.clone(),
            self.runtime.clone(),
            self.local_participant,
        )
        .map_err(ContextOwningLezPortError::Configuration)
    }
}

impl<Factory, Contexts, Store, Funding>
    ContextOwningLezBridgePorts<Factory, Contexts, Store, Funding>
where
    Factory: FreshLezBridgeTransportFactory,
    Factory::Transport: LezBridgeTransport,
    Contexts: BridgeRequestContextSource,
{
    /// Prepares the full initialize/fund plan with a durable caller-owned context.
    ///
    /// # Errors
    ///
    /// Returns a typed context, journal, transport, or agreement-validation error.
    pub async fn prepare_native_first_lock(
        &self,
        agreement: &ZecAgreementV1,
    ) -> Result<FirstLockPlanV1, ContextOwningLezPortError> {
        let (key, context) =
            self.reserve_context(agreement, BridgeOperationKind::NativeEscrowPrepare)?;
        let adapter = self.fresh_adapter()?;
        let result = adapter
            .prepare_native_first_lock(agreement, context.request_id().clone())
            .await;
        if matches!(&result, Err(PrepareNativeFirstLockError::Transport(_))) {
            self.resume_ambiguous(&key, &context)?;
        }
        result.map_err(|error| ContextOwningLezPortError::Adapter(Box::new(error)))
    }
}

impl<Factory, Contexts, Store, Funding>
    ContextOwningLezBridgePorts<Factory, Contexts, Store, Funding>
where
    Store: RecoveryStore,
{
    async fn durable_lez_plan(
        &self,
        agreement: &ZecAgreementV1,
    ) -> Result<FirstLockPlanV1, ContextOwningLezPortError> {
        let swap_id = agreement.coordinator().id();
        let plan = match self.local_participant {
            Participant::Taker => self
                .store
                .load_first_lock_intent(swap_id)
                .await
                .map_err(|error| ContextOwningLezPortError::Recovery(Box::new(error)))?
                .map(|intent| {
                    let valid = intent.swap_id() == swap_id
                        && intent.agreement_commitment() == agreement.agreement_commitment()
                        && intent.local_participant() == self.local_participant;
                    (valid, intent.plan().clone())
                }),
            Participant::Maker => self
                .store
                .load_maker_lock_intent(swap_id)
                .await
                .map_err(|error| ContextOwningLezPortError::Recovery(Box::new(error)))?
                .map(|intent| {
                    let valid = intent.swap_id() == swap_id
                        && intent.agreement_commitment() == agreement.agreement_commitment()
                        && intent.local_participant() == self.local_participant;
                    (valid, intent.plan().clone())
                }),
        };
        let Some((valid, plan @ FirstLockPlanV1::Lez { .. })) = plan else {
            return Err(ContextOwningLezPortError::MissingLezPlan);
        };
        if !valid {
            return Err(ContextOwningLezPortError::LezPlanContextMismatch);
        }
        Ok(plan)
    }
}

#[async_trait]
impl<Factory, Contexts, Store, Funding> LezFirstLockPort
    for ContextOwningLezBridgePorts<Factory, Contexts, Store, Funding>
where
    Factory: FreshLezBridgeTransportFactory,
    Factory::Transport: LezBridgeFirstLockTransport,
    Contexts: BridgeRequestContextSource,
    Store: RecoveryStore,
    Funding: Send + Sync,
{
    type Error = ContextOwningLezPortError;

    async fn observe_first_lock(
        &self,
        agreement: &ZecAgreementV1,
        submission: &PreparedFirstLockSubmissionV1,
    ) -> Result<FirstLockObservation, Self::Error> {
        let plan = self.durable_lez_plan(agreement).await?;
        let (key, context) =
            self.reserve_context(agreement, BridgeOperationKind::NativeEscrowExactObserve)?;
        let adapter = self.fresh_adapter()?;
        let result = adapter
            .observe_native_first_lock_step(
                agreement,
                context.request_id().clone(),
                &plan,
                submission,
            )
            .await;
        let ambiguous = matches!(&result, Err(ObserveNativeEscrowError::Transport(_)));
        if ambiguous {
            self.resume_ambiguous(&key, &context)?;
        } else {
            let retain = matches!(&result, Ok(FirstLockObservation::Unstable) | Err(_));
            self.finish_observation(
                &key,
                &context,
                if result.is_ok() {
                    BridgeObservationOutcome::Succeeded
                } else {
                    BridgeObservationOutcome::TypedError
                },
                retain,
            )?;
        }
        result.map_err(|error| ContextOwningLezPortError::Adapter(Box::new(error)))
    }

    async fn submit_first_lock(
        &self,
        agreement: &ZecAgreementV1,
        submission: &PreparedFirstLockSubmissionV1,
    ) -> Result<(), Self::Error> {
        let plan = self.durable_lez_plan(agreement).await?;
        let operation = match submission.step() {
            FirstLockStepV1::LezInitialize => BridgeOperationKind::NativeEscrowInitializeSubmit,
            FirstLockStepV1::LezFund => BridgeOperationKind::NativeEscrowFundSubmit,
            FirstLockStepV1::ZcashFund => {
                return Err(ContextOwningLezPortError::LezPlanContextMismatch);
            }
        };
        let (key, context) = self.reserve_context(agreement, operation)?;
        let outcome = self
            .fresh_adapter()?
            .submit_native_first_lock_step(
                agreement,
                context.request_id().clone(),
                &plan,
                submission,
            )
            .await
            .map_err(|error| ContextOwningLezPortError::Adapter(Box::new(error)))?;
        match outcome {
            NativeFirstLockSubmitOutcome::Accepted => Ok(()),
            NativeFirstLockSubmitOutcome::Unknown => {
                self.resume_ambiguous(&key, &context)?;
                Err(ContextOwningLezPortError::UnknownSubmission)
            }
        }
    }
}

#[async_trait]
impl<Factory, Contexts, Store, Funding> LezTakerFirstLockObservationPort
    for ContextOwningLezBridgePorts<Factory, Contexts, Store, Funding>
where
    Factory: FreshLezBridgeTransportFactory,
    Factory::Transport: LezBridgeObservationTransport,
    Contexts: BridgeRequestContextSource,
    Store: Send + Sync,
    Funding: Send + Sync,
{
    type Error = ContextOwningLezPortError;

    async fn observe_taker_first_lock(
        &self,
        agreement: &ZecAgreementV1,
        _previous: Option<&CanonicalLezEscrowObservationV1>,
    ) -> Result<TakerFirstLockObservationV1, Self::Error> {
        let (key, context) =
            self.reserve_context(agreement, BridgeOperationKind::NativeEscrowDiscoveryObserve)?;
        let window = context
            .discovery_window()
            .ok_or(ContextOwningLezPortError::MissingDiscoveryWindow)?;
        let result = self
            .fresh_adapter()?
            .observe_native_escrow(
                agreement,
                context.request_id().clone(),
                EscrowObservationTarget::DiscoverByTerms { window },
            )
            .await;
        let ambiguous = matches!(&result, Err(ObserveNativeEscrowError::Transport(_)));
        if ambiguous {
            self.resume_ambiguous(&key, &context)?;
        } else {
            let retain = matches!(&result, Ok(TakerFirstLockObservationV1::Unstable) | Err(_));
            self.finish_observation(
                &key,
                &context,
                if result.is_ok() {
                    BridgeObservationOutcome::Succeeded
                } else {
                    BridgeObservationOutcome::TypedError
                },
                retain,
            )?;
        }
        result.map_err(|error| ContextOwningLezPortError::Adapter(Box::new(error)))
    }
}

#[async_trait]
impl<Factory, Contexts, Store, Funding> LezMakerLockObservationPort
    for ContextOwningLezBridgePorts<Factory, Contexts, Store, Funding>
where
    Factory: FreshLezBridgeTransportFactory,
    Factory::Transport: LezBridgeObservationTransport,
    Contexts: BridgeRequestContextSource,
    Store: Send + Sync,
    Funding: Send + Sync,
{
    type Error = ContextOwningLezPortError;

    async fn observe_maker_lock(
        &self,
        agreement: &ZecAgreementV1,
    ) -> Result<MakerLockObservationV1, Self::Error> {
        let (key, context) =
            self.reserve_context(agreement, BridgeOperationKind::NativeEscrowDiscoveryObserve)?;
        let window = context
            .discovery_window()
            .ok_or(ContextOwningLezPortError::MissingDiscoveryWindow)?;
        let result = self
            .fresh_adapter()?
            .observe_native_maker_lock(agreement, context.request_id().clone(), window)
            .await;
        let ambiguous = matches!(&result, Err(ObserveNativeEscrowError::Transport(_)));
        if ambiguous {
            self.resume_ambiguous(&key, &context)?;
        } else {
            let retain = matches!(&result, Ok(MakerLockObservationV1::Unstable) | Err(_));
            self.finish_observation(
                &key,
                &context,
                if result.is_ok() {
                    BridgeObservationOutcome::Succeeded
                } else {
                    BridgeObservationOutcome::TypedError
                },
                retain,
            )?;
        }
        result.map_err(|error| ContextOwningLezPortError::Adapter(Box::new(error)))
    }
}

#[async_trait]
impl<Factory, Contexts, Store, Funding> LezRefundPort
    for ContextOwningLezBridgePorts<Factory, Contexts, Store, Funding>
where
    Factory: FreshLezBridgeTransportFactory,
    Factory::Transport: LezBridgeRefundTransport,
    Contexts: BridgeRequestContextSource,
    Store: Send + Sync,
    Funding: Send + Sync,
{
    type Error = ContextOwningLezPortError;

    async fn observe_refund_eligibility(
        &self,
        agreement: &ZecAgreementV1,
    ) -> Result<RefundEligibilityObservationV1, Self::Error> {
        let (key, context) = self.reserve_context(
            agreement,
            BridgeOperationKind::NativeRefundEligibilityObserve,
        )?;
        let result = self
            .fresh_adapter()?
            .observe_native_refund_eligibility(agreement, context.request_id().clone())
            .await;
        self.finish_refund_observation(&key, &context, &result, false)?;
        result.map_err(|error| ContextOwningLezPortError::Adapter(Box::new(error)))
    }

    async fn prepare_refund(
        &self,
        agreement: &ZecAgreementV1,
    ) -> Result<PreparedRefundSubmissionV1, Self::Error> {
        let (key, context) =
            self.reserve_context(agreement, BridgeOperationKind::NativeRefundPrepare)?;
        let result = self
            .fresh_adapter()?
            .prepare_native_refund(agreement, context.request_id().clone())
            .await;
        if matches!(&result, Err(NativeRefundAdapterError::Transport(_))) {
            self.resume_ambiguous(&key, &context)?;
        }
        result.map_err(|error| ContextOwningLezPortError::Adapter(Box::new(error)))
    }

    async fn observe_prepared_refund(
        &self,
        agreement: &ZecAgreementV1,
        prepared: &PreparedRefundSubmissionV1,
    ) -> Result<RefundObservationV1, Self::Error> {
        let (key, context) =
            self.reserve_context(agreement, BridgeOperationKind::NativeRefundExactObserve)?;
        let window = context
            .discovery_window()
            .ok_or(ContextOwningLezPortError::MissingDiscoveryWindow)?;
        let result = self
            .fresh_adapter()?
            .observe_prepared_native_refund(
                agreement,
                context.request_id().clone(),
                prepared,
                window,
            )
            .await;
        let retain = matches!(&result, Ok(RefundObservationV1::Unstable) | Err(_));
        self.finish_refund_observation(&key, &context, &result, retain)?;
        result.map_err(|error| ContextOwningLezPortError::Adapter(Box::new(error)))
    }

    async fn observe_counterparty_refund(
        &self,
        agreement: &ZecAgreementV1,
    ) -> Result<RefundObservationV1, Self::Error> {
        let (key, context) =
            self.reserve_context(agreement, BridgeOperationKind::NativeRefundDiscoveryObserve)?;
        let window = context
            .discovery_window()
            .ok_or(ContextOwningLezPortError::MissingDiscoveryWindow)?;
        let result = self
            .fresh_adapter()?
            .observe_counterparty_native_refund(agreement, context.request_id().clone(), window)
            .await;
        let retain = matches!(&result, Ok(RefundObservationV1::Unstable) | Err(_));
        self.finish_refund_observation(&key, &context, &result, retain)?;
        result.map_err(|error| ContextOwningLezPortError::Adapter(Box::new(error)))
    }

    async fn submit_refund(
        &self,
        agreement: &ZecAgreementV1,
        prepared: &PreparedRefundSubmissionV1,
    ) -> Result<RefundSubmitOutcomeV1, Self::Error> {
        let (key, context) =
            self.reserve_context(agreement, BridgeOperationKind::NativeRefundSubmit)?;
        let outcome = self
            .fresh_adapter()?
            .submit_native_refund(agreement, context.request_id().clone(), prepared)
            .await
            .map_err(|error| ContextOwningLezPortError::Adapter(Box::new(error)))?;
        if outcome == RefundSubmitOutcomeV1::Unknown {
            self.resume_ambiguous(&key, &context)?;
        }
        Ok(outcome)
    }
}

impl<Factory, Contexts, Store, Funding>
    ContextOwningLezBridgePorts<Factory, Contexts, Store, Funding>
where
    Contexts: BridgeRequestContextSource,
{
    fn finish_refund_observation<E: Error + Send + Sync + 'static, T>(
        &self,
        key: &BridgeOperationKey,
        context: &DurableBridgeRequestContext,
        result: &Result<T, NativeRefundAdapterError<E>>,
        retain_window: bool,
    ) -> Result<(), ContextOwningLezPortError> {
        if matches!(&result, Err(NativeRefundAdapterError::Transport(_))) {
            self.resume_ambiguous(key, context)
        } else {
            self.finish_observation(
                key,
                context,
                if result.is_ok() {
                    BridgeObservationOutcome::Succeeded
                } else {
                    BridgeObservationOutcome::TypedError
                },
                retain_window,
            )
        }
    }
}

#[async_trait]
impl<Factory, Contexts, Store, Funding> LezClaimPort
    for ContextOwningLezBridgePorts<Factory, Contexts, Store, Funding>
where
    Factory: FreshLezBridgeTransportFactory,
    Factory::Transport: LezBridgeClaimTransport,
    Contexts: BridgeRequestContextSource,
    Store: Send + Sync,
    Funding: CanonicalLezFundingSource,
{
    type Error = ContextOwningLezPortError;

    async fn prepare_revealing_claim(
        &self,
        agreement: &ZecAgreementV1,
        preimage: &ClaimPreimage,
    ) -> Result<PreparedClaimSubmissionV1, Self::Error> {
        let evidence = self
            .funding
            .canonical_lez_funding(agreement)
            .await
            .map_err(|error| ContextOwningLezPortError::Recovery(Box::new(error)))?;
        let funding_transaction_id = validate_funding_evidence(agreement, &evidence)?;
        let (key, context) =
            self.reserve_context(agreement, BridgeOperationKind::RevealingClaimPrepare)?;
        let result = self
            .fresh_adapter()?
            .prepare_native_revealing_claim(
                agreement,
                context.request_id().clone(),
                funding_transaction_id,
                preimage,
            )
            .await;
        if matches!(&result, Err(NativeRevealingClaimAdapterError::Transport(_))) {
            self.resume_ambiguous(&key, &context)?;
        }
        result.map_err(|error| ContextOwningLezPortError::Adapter(Box::new(error)))
    }

    async fn observe_prepared_revealing_claim(
        &self,
        agreement: &ZecAgreementV1,
        prepared: &PreparedClaimSubmissionV1,
    ) -> Result<RevealingClaimObservationV1, Self::Error> {
        let (key, context) =
            self.reserve_context(agreement, BridgeOperationKind::RevealingClaimExactObserve)?;
        let result = self
            .fresh_adapter()?
            .observe_prepared_native_revealing_claim(
                agreement,
                context.request_id().clone(),
                prepared,
            )
            .await;
        let retain = matches!(&result, Ok(RevealingClaimObservationV1::Unstable) | Err(_));
        self.finish_claim_observation(&key, &context, &result, retain)?;
        result.map_err(|error| ContextOwningLezPortError::Adapter(Box::new(error)))
    }

    async fn observe_counterparty_revealing_claim(
        &self,
        agreement: &ZecAgreementV1,
    ) -> Result<RevealingClaimObservationV1, Self::Error> {
        let (key, context) = self.reserve_context(
            agreement,
            BridgeOperationKind::RevealingClaimDiscoveryObserve,
        )?;
        let window = context
            .discovery_window()
            .ok_or(ContextOwningLezPortError::MissingDiscoveryWindow)?;
        let result = self
            .fresh_adapter()?
            .observe_counterparty_native_revealing_claim(
                agreement,
                context.request_id().clone(),
                window,
            )
            .await;
        let retain = matches!(&result, Ok(RevealingClaimObservationV1::Unstable) | Err(_));
        self.finish_claim_observation(&key, &context, &result, retain)?;
        result.map_err(|error| ContextOwningLezPortError::Adapter(Box::new(error)))
    }

    async fn submit_revealing_claim(
        &self,
        agreement: &ZecAgreementV1,
        prepared: &PreparedClaimSubmissionV1,
    ) -> Result<(), Self::Error> {
        let (key, context) =
            self.reserve_context(agreement, BridgeOperationKind::RevealingClaimSubmit)?;
        let outcome = self
            .fresh_adapter()?
            .submit_native_revealing_claim(agreement, context.request_id().clone(), prepared)
            .await
            .map_err(|error| ContextOwningLezPortError::Adapter(Box::new(error)))?;
        match outcome {
            RevealingClaimSubmitOutcome::Accepted => Ok(()),
            RevealingClaimSubmitOutcome::Unknown => {
                self.resume_ambiguous(&key, &context)?;
                Err(ContextOwningLezPortError::UnknownSubmission)
            }
        }
    }
}

impl<Factory, Contexts, Store, Funding>
    ContextOwningLezBridgePorts<Factory, Contexts, Store, Funding>
where
    Contexts: BridgeRequestContextSource,
{
    fn finish_claim_observation<E: Error + Send + Sync + 'static>(
        &self,
        key: &BridgeOperationKey,
        context: &DurableBridgeRequestContext,
        result: &Result<RevealingClaimObservationV1, NativeRevealingClaimAdapterError<E>>,
        retain_window: bool,
    ) -> Result<(), ContextOwningLezPortError> {
        if matches!(result, Err(NativeRevealingClaimAdapterError::Transport(_))) {
            self.resume_ambiguous(key, context)
        } else {
            self.finish_observation(
                key,
                context,
                if result.is_ok() {
                    BridgeObservationOutcome::Succeeded
                } else {
                    BridgeObservationOutcome::TypedError
                },
                retain_window,
            )
        }
    }
}

fn context_matches_spec(context: &DurableBridgeRequestContext, spec: &BridgeRequestSpec) -> bool {
    context.request_id() == spec.request_id()
        && context.discovery_window() == spec.discovery_window()
}

fn validate_funding_evidence(
    agreement: &ZecAgreementV1,
    evidence: &FirstLockConfirmedEvidenceV1,
) -> Result<TransactionId, ContextOwningLezPortError> {
    if evidence.step() != FirstLockStepV1::LezFund
        || evidence.confirmations()
            < agreement
                .coordinator()
                .required_confirmations(agreement.lez_depositor())
    {
        return Err(ContextOwningLezPortError::InvalidCanonicalFunding);
    }
    let transaction_id = TransactionId::from_bytes(
        *Hex32::from_hex(evidence.transaction_id())
            .map_err(|_| ContextOwningLezPortError::InvalidCanonicalFunding)?
            .as_bytes(),
    );
    if transaction_id.as_bytes() != evidence.expected_submission_id() {
        return Err(ContextOwningLezPortError::InvalidCanonicalFunding);
    }
    Ok(transaction_id)
}

#[cfg(test)]
mod tests {
    use std::{
        convert::Infallible,
        fmt, fs,
        sync::atomic::{AtomicU64, Ordering},
    };

    use lez_bridge_protocol::{
        Hex32, Participant as BridgeParticipant, RequestId, RuntimeCompatibility,
    };
    use lez_swap_core::SwapId;

    use super::*;

    static NEXT_DB: AtomicU64 = AtomicU64::new(0);

    struct FixedContext(BridgeRequestSpec);

    struct SensitiveBoundary;

    impl fmt::Debug for SensitiveBoundary {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("sdk-port-secret-must-not-leak")
        }
    }

    impl BridgeRequestContextSource for FixedContext {
        type Error = Infallible;

        fn next_request(
            &self,
            _key: &BridgeOperationKey,
        ) -> Result<BridgeRequestSpec, Self::Error> {
            Ok(self.0.clone())
        }
    }

    #[test]
    fn diagnostics_redact_every_credential_and_persistence_boundary() {
        let path = std::env::temp_dir().join(format!(
            "lez-bridge-sdk-ports-debug-{}-{}.sqlite",
            std::process::id(),
            NEXT_DB.fetch_add(1, Ordering::Relaxed)
        ));
        let ports = ContextOwningLezBridgePorts {
            run_id: RunId::new("sdk-port-debug-run").expect("run id"),
            runtime: RuntimeDescriptor::new(
                BridgeParticipant::Taker,
                RuntimeCompatibility::NssaV0_1_2,
                Hex32::from_bytes([1; 32]),
                Hex32::from_bytes([2; 32]),
                Hex32::from_bytes([3; 32]),
                Hex32::from_bytes([4; 32]),
                Hex32::from_bytes([5; 32]),
            ),
            local_participant: Participant::Taker,
            factory: SensitiveBoundary,
            contexts: SensitiveBoundary,
            store: SensitiveBoundary,
            funding: SensitiveBoundary,
            journal: Mutex::new(
                SqliteBridgeOperationJournal::open(&path).expect("operation journal"),
            ),
        };

        let diagnostic = format!("{ports:?}");
        assert!(!diagnostic.contains("sdk-port-secret-must-not-leak"));
        assert_eq!(diagnostic.matches("<redacted>").count(), 5);

        drop(ports);
        let _ = fs::remove_file(&path);
        let _ = fs::remove_file(path.with_extension("sqlite-wal"));
        let _ = fs::remove_file(path.with_extension("sqlite-shm"));
    }

    #[test]
    fn unknown_commit_recovery_rejects_a_different_outcome_with_the_same_next_context() {
        let path = std::env::temp_dir().join(format!(
            "lez-bridge-sdk-ports-{}-{}.sqlite",
            std::process::id(),
            NEXT_DB.fetch_add(1, Ordering::Relaxed)
        ));
        let run_id = RunId::new("sdk-port-outcome-run").expect("run id");
        let key = BridgeOperationKey::new(
            run_id.clone(),
            SwapId::new("sdk-port-outcome-swap").expect("swap id"),
            Participant::Taker,
            BridgeOperationKind::NativeEscrowExactObserve,
        );
        let first = BridgeRequestSpec::new(
            RequestId::new("sdk-port-outcome-first").expect("request id"),
            None,
        );
        let next = BridgeRequestSpec::new(
            RequestId::new("sdk-port-outcome-next").expect("request id"),
            None,
        );
        let mut journal = SqliteBridgeOperationJournal::open(&path).expect("journal");
        let current = journal
            .begin_or_resume(&key, &first)
            .expect("first context")
            .context()
            .clone();
        let _ = journal
            .advance_observation(&key, &current, BridgeObservationOutcome::Succeeded, &next)
            .expect("concurrent successful outcome");
        let runtime = RuntimeDescriptor::new(
            BridgeParticipant::Taker,
            RuntimeCompatibility::NssaV0_1_2,
            Hex32::from_bytes([1; 32]),
            Hex32::from_bytes([2; 32]),
            Hex32::from_bytes([3; 32]),
            Hex32::from_bytes([4; 32]),
            Hex32::from_bytes([5; 32]),
        );
        let ports = ContextOwningLezBridgePorts {
            run_id,
            runtime,
            local_participant: Participant::Taker,
            factory: (),
            contexts: FixedContext(next),
            store: (),
            funding: (),
            journal: Mutex::new(journal),
        };

        assert!(matches!(
            ports.finish_observation(&key, &current, BridgeObservationOutcome::TypedError, false,),
            Err(ContextOwningLezPortError::Journal(
                StoreError::BridgeOperationContextConflict
            ))
        ));
        drop(ports);
        let _ = fs::remove_file(&path);
        let _ = fs::remove_file(path.with_extension("sqlite-wal"));
        let _ = fs::remove_file(path.with_extension("sqlite-shm"));
    }
}
