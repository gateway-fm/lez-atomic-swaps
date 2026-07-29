//! Main-process composition boundary for a dedicated official LEZ sidecar.

#![forbid(unsafe_code)]

mod btc_asset_first_lock_proof_v2;
mod btc_asset_v2;
mod btc_current_first_lock;
mod btc_first_lock_proof;
mod canonical_funding;
mod client_factory;
mod request_context;
mod sdk_ports;
mod xmr_v3_claim_authorization;
mod xmr_v3_first_lock;

pub use btc_asset_first_lock_proof_v2::{
    BtcLezAssetFirstLockProofV2, BtcLezAssetFirstLockProofV2Error,
};
pub use btc_asset_v2::{
    BtcLezAssetBridgeBindingV2, BtcLezAssetBridgeBindingV2Error, BtcLezAssetBridgeV2Error,
    LezBridgeAssetV2Transport,
};
pub use btc_current_first_lock::{
    CurrentLezFirstLockError, CurrentLezFirstLockEvidenceV1, CurrentLezFundedEscrowError,
    CurrentLezFundedEscrowEvidenceV1, LezBridgeCurrentEscrowTransport,
};
pub use btc_first_lock_proof::{
    BtcLezFirstLockProofError, BtcLezFirstLockProofV1, LezBridgeBtcFirstLockProofTransport,
};
pub use canonical_funding::{
    SqliteCanonicalLezFundingSource, SqliteCanonicalLezFundingSourceError,
};
pub use client_factory::{
    CapabilityFileBridgeClientFactory, CapabilityFileBridgeClientFactoryError,
    CapabilityFileXmrReleaseClientFactory,
};
pub use request_context::{
    ActorBridgeRequestContextError, ActorBridgeRequestContextSource, BridgeDiscoveryWindowSource,
};
pub use sdk_ports::{
    BridgeRequestContextSource, CanonicalLezFundingSource, ContextOwningLezBridgePorts,
    ContextOwningLezPortError, FreshLezBridgeTransportFactory,
};
pub use xmr_v3_claim_authorization::{
    PreparedXmrClaimAuthorizationErrorV3, PreparedXmrClaimAuthorizationEvidenceV3,
};
pub use xmr_v3_first_lock::{
    FinalizedXmrLezFirstLockError, FinalizedXmrLezFirstLockEvidenceV3,
    FinalizedXmrLezFundingSubmissionError, FinalizedXmrLezInitializationError,
    FinalizedXmrLezInitializationEvidenceV3, XmrLezBridgeBindingV3, XmrLezBridgeBindingV3Error,
};

use async_trait::async_trait;
use lez_bridge_client::{BridgeClient, BridgeClientError, FinalizedWitnessedClaimPresence};
use lez_bridge_protocol::{
    AccountIds, DiscoveryWindow, EscrowMetadataFacts, EscrowObservationTarget, EscrowState,
    ExactTransactionBytes, FundingFoundFacts, FundingObservation, Hex32, InitializationFoundFacts,
    InitializationObservation, MessageContext, NativeCustodyFacts, NativeEscrowAccountFacts,
    NativeEscrowAccountObservation, NativeEscrowTerms, NativeEscrowTermsInput,
    NativeRefundFoundFacts, NativeRefundObservation, NativeRefundObservationTarget,
    ObserveEscrowRequest, ObserveEscrowResult, ObserveFinalizedWitnessedClaimRequest,
    ObserveNativeRefundRequest, ObserveNativeRefundResult, ObserveRevealingClaimRequest,
    ObserveRevealingClaimResult, ObservedTransactionFacts, Participant as BridgeParticipant,
    PrepareNativeEscrowRequest, PrepareNativeEscrowResult, PrepareNativeRefundRequest,
    PrepareNativeRefundResult, PrepareRevealingClaimRequest, PrepareRevealingClaimResult,
    PreparedTransaction, ProtocolValueError, RequestId, RevealingClaimFoundFacts,
    RevealingClaimObservation, RevealingClaimObservationTarget, RevealingPreimage, RunId,
    RuntimeCompatibility, RuntimeDescriptor, SubmissionOutcome, SubmitTransactionRequest,
    SubmitTransactionResult, TransactionId,
};
use lez_swap_core::{ChainPosition, LezUnixMilliseconds, Participant};
use lez_zec_swap_sdk::{
    CanonicalLezEscrowObservationV1, ClaimError, ClaimPreimage, ClaimStepV1,
    FirstLockConfirmedEvidenceV1, FirstLockIntentError, FirstLockObservation, FirstLockPlanV1,
    FirstLockStepV1, FirstLockTransitionError, LezAssetV1, LezClaimInstructionV1,
    LezClaimNodeSnapshotV1, LezClaimObservationError, LezClaimTransactionSnapshotV1,
    LezCustodySnapshotV1, LezEnvironmentV1, LezEscrowMetadataSnapshotV1, LezEscrowStatusV1,
    LezFundInstructionV1, LezFundTransactionSnapshotV1, LezInclusionStatusV1, LezNodeSnapshotV1,
    LezObservationError, LezStableTipV1, MakerLockObservationV1, ObservedTakerFirstLockEvidenceV1,
    PreparedClaimSubmissionV1, PreparedFirstLockSubmissionV1, PreparedRefundSubmissionV1,
    RefundEligibilityObservationV1, RefundError, RefundEvidenceV1, RefundFundingWaitReasonV1,
    RefundObservationV1, RefundStepV1, RefundSubmitOutcomeV1, RevealingClaimEvidenceV1,
    RevealingClaimObservationV1, TakerFirstLockObservationV1, ZecAgreementV1,
};
use thiserror::Error;

/// One attempt at randomized native escrow preparation.
///
/// The transport must not retry: an interrupted call has an unknown outcome and
/// the caller-owned request ID is the durable idempotency key.
#[async_trait]
pub trait LezBridgeTransport: Send + Sync {
    /// Concrete transport failure.
    type Error: std::error::Error + Send + Sync + 'static;

    /// Prepares initialization and funding exactly once.
    async fn prepare_native_escrow(
        &self,
        request: PrepareNativeEscrowRequest,
    ) -> Result<PrepareNativeEscrowResult, Self::Error>;
}

#[async_trait]
impl LezBridgeTransport for BridgeClient {
    type Error = BridgeClientError;

    async fn prepare_native_escrow(
        &self,
        request: PrepareNativeEscrowRequest,
    ) -> Result<PrepareNativeEscrowResult, Self::Error> {
        BridgeClient::prepare_native_escrow(self, request).await
    }
}

/// One attempt at native escrow observation.
#[async_trait]
pub trait LezBridgeObservationTransport: Send + Sync {
    /// Concrete transport failure.
    type Error: std::error::Error + Send + Sync + 'static;

    /// Observes initialization and funding facts exactly once.
    async fn observe_escrow(
        &self,
        request: ObserveEscrowRequest,
    ) -> Result<ObserveEscrowResult, Self::Error>;
}

#[async_trait]
impl LezBridgeObservationTransport for BridgeClient {
    type Error = BridgeClientError;

    async fn observe_escrow(
        &self,
        request: ObserveEscrowRequest,
    ) -> Result<ObserveEscrowResult, Self::Error> {
        BridgeClient::observe_escrow(self, request).await
    }
}

/// One read-only attempt to classify exact witnessed-claim presence.
///
/// This boundary is intentionally separate from submission. Implementations
/// must preserve the client's four-way `PresentExact` / `NotFound` /
/// `Unavailable` / `Uncertain` classification; only `NotFound` may authorize
/// an actor's first exact submission attempt.
#[async_trait]
pub trait LezBridgeWitnessedClaimPresenceTransport: Send + Sync {
    /// Concrete transport or evidence-validation failure.
    type Error: std::error::Error + Send + Sync + 'static;

    /// Classifies one exact caller-owned bounded finalized window once.
    async fn classify_finalized_witnessed_claim(
        &self,
        request: ObserveFinalizedWitnessedClaimRequest,
    ) -> Result<FinalizedWitnessedClaimPresence, Self::Error>;
}

#[async_trait]
impl LezBridgeWitnessedClaimPresenceTransport for BridgeClient {
    type Error = BridgeClientError;

    async fn classify_finalized_witnessed_claim(
        &self,
        request: ObserveFinalizedWitnessedClaimRequest,
    ) -> Result<FinalizedWitnessedClaimPresence, Self::Error> {
        BridgeClient::classify_finalized_witnessed_claim(self, request).await
    }
}

/// One fresh-client attempt at exact owner first-lock observation or submission.
///
/// A process-local bridge client rejects request-ID reuse. Context-owning SDK
/// composition therefore creates a fresh transport for an ambiguous retry and
/// reuses the exact durable request context rather than retrying inside this trait.
#[async_trait]
pub trait LezBridgeFirstLockTransport: Send + Sync {
    /// Concrete transport failure.
    type Error: std::error::Error + Send + Sync + 'static;

    /// Observes the complete initialize/fund pair by its two durable identities.
    async fn observe_escrow(
        &self,
        request: ObserveEscrowRequest,
    ) -> Result<ObserveEscrowResult, Self::Error>;

    /// Submits one exact durable transaction.
    async fn submit_transaction(
        &self,
        request: SubmitTransactionRequest,
    ) -> Result<SubmitTransactionResult, Self::Error>;
}

#[async_trait]
impl LezBridgeFirstLockTransport for BridgeClient {
    type Error = BridgeClientError;

    async fn observe_escrow(
        &self,
        request: ObserveEscrowRequest,
    ) -> Result<ObserveEscrowResult, Self::Error> {
        BridgeClient::observe_escrow(self, request).await
    }

    async fn submit_transaction(
        &self,
        request: SubmitTransactionRequest,
    ) -> Result<SubmitTransactionResult, Self::Error> {
        BridgeClient::submit_transaction(self, request).await
    }
}

/// One attempt at each native refund compatibility operation.
///
/// The transport never retries randomized preparation, observation, or exact
/// submission. Durable request IDs and scan windows remain caller-owned.
#[async_trait]
pub trait LezBridgeRefundTransport: Send + Sync {
    /// Concrete transport failure.
    type Error: std::error::Error + Send + Sync + 'static;

    /// Prepares one official fixed-destination native refund.
    async fn prepare_native_refund(
        &self,
        request: PrepareNativeRefundRequest,
    ) -> Result<PrepareNativeRefundResult, Self::Error>;

    /// Observes canonical native account and optional refund facts.
    async fn observe_native_refund(
        &self,
        request: ObserveNativeRefundRequest,
    ) -> Result<ObserveNativeRefundResult, Self::Error>;

    /// Submits exact durable refund bytes through the generic submit method.
    async fn submit_transaction(
        &self,
        request: SubmitTransactionRequest,
    ) -> Result<SubmitTransactionResult, Self::Error>;
}

#[async_trait]
impl LezBridgeRefundTransport for BridgeClient {
    type Error = BridgeClientError;

    async fn prepare_native_refund(
        &self,
        request: PrepareNativeRefundRequest,
    ) -> Result<PrepareNativeRefundResult, Self::Error> {
        BridgeClient::prepare_native_refund(self, request).await
    }

    async fn observe_native_refund(
        &self,
        request: ObserveNativeRefundRequest,
    ) -> Result<ObserveNativeRefundResult, Self::Error> {
        BridgeClient::observe_native_refund(self, request).await
    }

    async fn submit_transaction(
        &self,
        request: SubmitTransactionRequest,
    ) -> Result<SubmitTransactionResult, Self::Error> {
        BridgeClient::submit_transaction(self, request).await
    }
}

/// One attempt at each native preimage-revealing claim operation.
///
/// This boundary deliberately does not implement the SDK's context-free
/// `LezClaimPort`: durable request IDs and bounded discovery windows remain
/// explicit caller-owned inputs.
#[async_trait]
pub trait LezBridgeClaimTransport: Send + Sync {
    /// Concrete transport failure.
    type Error: std::error::Error + Send + Sync + 'static;

    /// Prepares one official claimant-signed native revealing claim.
    async fn prepare_revealing_claim(
        &self,
        request: PrepareRevealingClaimRequest,
    ) -> Result<PrepareRevealingClaimResult, Self::Error>;

    /// Observes one exact or bounded terms-discovered revealing claim.
    async fn observe_revealing_claim(
        &self,
        request: ObserveRevealingClaimRequest,
    ) -> Result<ObserveRevealingClaimResult, Self::Error>;

    /// Submits exact protected claim bytes through generic submit.
    async fn submit_transaction(
        &self,
        request: SubmitTransactionRequest,
    ) -> Result<SubmitTransactionResult, Self::Error>;
}

#[async_trait]
impl LezBridgeClaimTransport for BridgeClient {
    type Error = BridgeClientError;

    async fn prepare_revealing_claim(
        &self,
        request: PrepareRevealingClaimRequest,
    ) -> Result<PrepareRevealingClaimResult, Self::Error> {
        BridgeClient::prepare_revealing_claim(self, request).await
    }

    async fn observe_revealing_claim(
        &self,
        request: ObserveRevealingClaimRequest,
    ) -> Result<ObserveRevealingClaimResult, Self::Error> {
        BridgeClient::observe_revealing_claim(self, request).await
    }

    async fn submit_transaction(
        &self,
        request: SubmitTransactionRequest,
    ) -> Result<SubmitTransactionResult, Self::Error> {
        BridgeClient::submit_transaction(self, request).await
    }
}

/// Invalid role binding between the main process and its dedicated sidecar.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum LezBridgeConfigurationError {
    /// The sidecar signer role differs from the local actor.
    #[error("LEZ sidecar role differs from the local participant")]
    SidecarRoleMismatch,
}

/// Role-local main-process adapter for one isolated LEZ sidecar.
#[derive(Debug)]
pub struct LezBridgeAdapter<T> {
    transport: T,
    run_id: RunId,
    runtime: RuntimeDescriptor,
    local_participant: Participant,
}

impl<T> LezBridgeAdapter<T> {
    /// Binds a transport to one run, runtime, and local actor.
    ///
    /// # Errors
    ///
    /// Rejects a sidecar whose isolated signing role differs from the actor.
    pub fn new(
        transport: T,
        run_id: RunId,
        runtime: RuntimeDescriptor,
        local_participant: Participant,
    ) -> Result<Self, LezBridgeConfigurationError> {
        if runtime.sidecar_role != bridge_participant(local_participant) {
            return Err(LezBridgeConfigurationError::SidecarRoleMismatch);
        }
        Ok(Self {
            transport,
            run_id,
            runtime,
            local_participant,
        })
    }
}

/// Failure converting signed terms into one exact sidecar preparation.
#[derive(Debug, Error)]
pub enum PrepareNativeFirstLockError<E: std::error::Error + 'static> {
    /// Only the agreement-bound LEZ depositor can prepare this actor's first lock.
    #[error("local participant is not the signed LEZ depositor")]
    WrongDepositor,
    /// This adapter accepts only supported pinned native compatibility runtimes.
    #[error("signed LEZ environment is not compatible with this bridge")]
    IncompatibleEnvironment,
    /// The signed channel or genesis identity differs from the selected runtime.
    #[error("signed LEZ chain identity differs from the selected runtime")]
    ChainIdentityMismatch,
    /// The signed escrow program differs from the selected runtime.
    #[error("signed LEZ escrow program differs from the selected runtime")]
    EscrowProgramMismatch,
    /// The sidecar signer is not the agreement-bound depositor account.
    #[error("LEZ sidecar signer differs from the signed depositor account")]
    SignerAccountMismatch,
    /// The isolated official compatibility bridge currently supports only native escrow.
    #[error("LEZ bridge does not support this signed asset")]
    UnsupportedAsset,
    /// Exact signed terms could not form a valid primitive bridge request.
    #[error("signed LEZ terms are invalid at the bridge boundary")]
    Protocol(#[source] ProtocolValueError),
    /// The sidecar did not echo the durable request context after preparing randomized bytes.
    #[error("LEZ bridge preparation response context mismatch")]
    ResponseContextMismatch,
    /// The SDK rejected malformed or aliased prepared transaction evidence.
    #[error("LEZ bridge returned an invalid first-lock plan")]
    FirstLockPlan(#[source] FirstLockIntentError),
    /// The selected step is not one of the complete durable LEZ plan's exact values.
    #[error("LEZ first-lock step differs from the complete durable plan")]
    PreparedPlanMismatch,
    /// No retry is attempted because delivery may have succeeded.
    #[error("LEZ bridge preparation outcome is unknown")]
    Transport(#[source] E),
}

/// Failure building or independently validating one native escrow observation.
#[derive(Debug, Error)]
pub enum ObserveNativeEscrowError<E: std::error::Error + 'static> {
    /// The canonical SDK validator only models the taker's LEZ first lock.
    #[error("agreement does not select a taker-funded LEZ first lock")]
    WrongDirection,
    /// Exact IDs are role-local durable material and may only be used by the depositor.
    #[error("exact LEZ observation requires the local signed depositor")]
    ExactTargetRequiresDepositor,
    /// Counterparty discovery may only be requested by the signed claimant.
    #[error("LEZ discovery requires the local signed claimant")]
    DiscoveryRequiresClaimant,
    /// This adapter accepts only supported pinned native compatibility runtimes.
    #[error("signed LEZ environment is not compatible with this bridge")]
    IncompatibleEnvironment,
    /// The signed channel or genesis identity differs from the selected runtime.
    #[error("signed LEZ chain identity differs from the selected runtime")]
    ChainIdentityMismatch,
    /// The signed escrow program differs from the selected runtime.
    #[error("signed LEZ escrow program differs from the selected runtime")]
    EscrowProgramMismatch,
    /// The sidecar signer is not the agreement-bound local account.
    #[error("LEZ sidecar signer differs from the signed local account")]
    SignerAccountMismatch,
    /// The isolated official compatibility bridge currently supports only native escrow.
    #[error("LEZ bridge does not support this signed asset")]
    UnsupportedAsset,
    /// Exact signed terms could not form a valid primitive bridge request.
    #[error("signed LEZ terms are invalid at the bridge boundary")]
    Protocol(#[source] ProtocolValueError),
    /// No retry is attempted because the observation attempt may have reached the sidecar.
    #[error("LEZ bridge observation outcome is unknown")]
    Transport(#[source] E),
    /// The sidecar did not echo the durable request context.
    #[error("LEZ bridge observation response context mismatch")]
    ResponseContextMismatch,
    /// Bracketing node tips were not identical.
    #[error("LEZ bridge observation changed while facts were collected")]
    UnstableTip,
    /// Found initialization/funding facts were partial or internally inconsistent.
    #[error("LEZ bridge returned inconsistent escrow facts")]
    InconsistentFacts,
    /// The official-sidecar primitives failed the independent SDK agreement validator.
    #[error("LEZ bridge facts do not prove the signed escrow")]
    Canonical(#[source] LezObservationError),
    /// The exact step or complete durable two-transaction plan was substituted.
    #[error("LEZ first-lock observation differs from the complete durable plan")]
    PreparedPlanMismatch,
    /// SDK first-lock evidence construction rejected the canonical primitive facts.
    #[error("LEZ first-lock observation evidence is invalid")]
    FirstLock(#[source] FirstLockTransitionError),
}

/// Conservative result of one exact first-lock submission attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeFirstLockSubmitOutcome {
    /// The node accepted the exact bytes or already knew them.
    Accepted,
    /// Delivery or acknowledgement became ambiguous after the attempt began.
    Unknown,
}

/// Failure binding one native refund operation to the accepted agreement.
#[derive(Debug, Error)]
pub enum NativeRefundAdapterError<E: std::error::Error + 'static> {
    /// Only the signed LEZ depositor may check eligibility, prepare, or submit this refund.
    #[error("local participant is not the signed LEZ refund owner")]
    WrongOwner,
    /// Exact durable refund lookup is reserved for the signed depositor.
    #[error("exact LEZ refund observation requires the signed depositor")]
    ExactTargetRequiresOwner,
    /// Terms discovery is reserved for the non-owner observing the refund.
    #[error("LEZ refund discovery requires the signed claimant")]
    DiscoveryRequiresClaimant,
    /// This adapter accepts only supported pinned native compatibility runtimes.
    #[error("signed LEZ environment is not compatible with this refund bridge")]
    IncompatibleEnvironment,
    /// Signed channel or genesis differs from the selected runtime.
    #[error("signed LEZ chain identity differs from the selected runtime")]
    ChainIdentityMismatch,
    /// Signed escrow program differs from the selected runtime.
    #[error("signed LEZ escrow program differs from the selected runtime")]
    EscrowProgramMismatch,
    /// Sidecar signer differs from the agreement-bound local account.
    #[error("LEZ sidecar signer differs from the signed local account")]
    SignerAccountMismatch,
    /// The compatibility bridge currently supports only native escrow.
    #[error("LEZ refund bridge does not support this signed asset")]
    UnsupportedAsset,
    /// Signed terms or durable bytes were invalid at the protocol boundary.
    #[error("signed LEZ refund values are invalid at the bridge boundary")]
    Protocol(#[source] ProtocolValueError),
    /// A non-submit bridge attempt failed with an unknown delivery outcome.
    #[error("LEZ refund bridge operation outcome is unknown")]
    Transport(#[source] E),
    /// Sidecar response did not echo the caller-owned context.
    #[error("LEZ refund bridge response context mismatch")]
    ResponseContextMismatch,
    /// Bracketing canonical clocks differed.
    #[error("LEZ refund state changed while facts were collected")]
    UnstableClock,
    /// Primitive account, transaction, instruction, position, or target facts conflict.
    #[error("LEZ refund bridge returned inconsistent facts")]
    InconsistentFacts,
    /// A found refund predates the exact signed millisecond deadline.
    #[error("LEZ refund transaction was observed before the signed deadline")]
    RefundBeforeDeadline,
    /// Durable prepared bytes are not a LEZ refund.
    #[error("durable prepared submission is not a LEZ refund")]
    WrongPreparedStep,
    /// SDK refund evidence or prepared-submission validation failed.
    #[error("LEZ refund evidence is invalid")]
    Refund(#[source] RefundError),
}

/// Validated refund observation plus private page-progression evidence.
///
/// The SDK-facing observation deliberately does not expose whether a raw miss
/// covered the whole requested page. The context-owning composition needs that
/// fact to advance its durable cursor without conflating other unstable results.
#[derive(Debug)]
pub(crate) struct NativeRefundPageObservation {
    observation: RefundObservationV1,
    fully_covered_miss: bool,
}

impl NativeRefundPageObservation {
    const fn new(observation: RefundObservationV1, fully_covered_miss: bool) -> Self {
        Self {
            observation,
            fully_covered_miss,
        }
    }

    pub(crate) const fn fully_covered_miss(&self) -> bool {
        self.fully_covered_miss
    }

    pub(crate) fn into_observation(self) -> RefundObservationV1 {
        self.observation
    }
}

/// One conservative result from exactly one revealing-claim submission attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RevealingClaimSubmitOutcome {
    /// The node accepted the exact bytes or already knew them.
    Accepted,
    /// Delivery or response validation failed after the attempt began.
    Unknown,
}

/// Failure binding one native revealing-claim operation to the accepted agreement.
#[derive(Debug, Error)]
pub enum NativeRevealingClaimAdapterError<E: std::error::Error + 'static> {
    /// Only the signed LEZ claimant may prepare, exactly observe, or submit the claim.
    #[error("local participant is not the signed LEZ claimant")]
    WrongClaimant,
    /// Exact durable observation is reserved for the signed claimant.
    #[error("exact LEZ revealing-claim observation requires the signed claimant")]
    ExactTargetRequiresClaimant,
    /// Counterparty discovery is reserved for the signed LEZ depositor.
    #[error("LEZ revealing-claim discovery requires the signed depositor")]
    DiscoveryRequiresDepositor,
    /// This adapter accepts only supported pinned native compatibility runtimes.
    #[error("signed LEZ environment is not compatible with this claim bridge")]
    IncompatibleEnvironment,
    /// Signed channel or genesis differs from the selected runtime.
    #[error("signed LEZ chain identity differs from the selected runtime")]
    ChainIdentityMismatch,
    /// Signed escrow program differs from the selected runtime.
    #[error("signed LEZ escrow program differs from the selected runtime")]
    EscrowProgramMismatch,
    /// Sidecar signer differs from the agreement-bound local account.
    #[error("LEZ sidecar signer differs from the signed local account")]
    SignerAccountMismatch,
    /// The compatibility bridge currently supports only native escrow.
    #[error("LEZ revealing-claim bridge does not support this signed asset")]
    UnsupportedAsset,
    /// Signed terms, funding identity, or durable bytes are invalid at the protocol boundary.
    #[error("signed LEZ revealing-claim values are invalid at the bridge boundary")]
    Protocol(#[source] ProtocolValueError),
    /// A zero funding transaction cannot bind the claim preparation request.
    #[error("LEZ revealing claim requires a nonzero retained funding transaction identity")]
    EmptyFundingTransactionId,
    /// One non-submit attempt failed with an unknown delivery outcome; no retry occurred.
    #[error("LEZ revealing-claim bridge operation outcome is unknown")]
    Transport(#[source] E),
    /// Sidecar response did not echo the caller-owned context.
    #[error("LEZ revealing-claim bridge response context mismatch")]
    ResponseContextMismatch,
    /// Primitive target, bytes, transaction, instruction, or position facts conflict.
    #[error("LEZ revealing-claim bridge returned inconsistent facts")]
    InconsistentFacts,
    /// Durable protected bytes are not a LEZ revealing claim.
    #[error("durable prepared submission is not a LEZ revealing claim")]
    WrongPreparedStep,
    /// SDK prepared claim validation failed.
    #[error("LEZ revealing-claim submission is invalid")]
    Claim(#[source] ClaimError),
    /// Canonical SDK claim-snapshot validation failed.
    #[error("LEZ revealing-claim evidence is invalid")]
    Canonical(#[source] LezClaimObservationError),
}

impl<T: LezBridgeTransport> LezBridgeAdapter<T> {
    /// Converts one validated agreement into the exact two-step LEZ first-lock plan.
    ///
    /// The caller supplies and durably owns the request ID. This method makes one
    /// preparation attempt and never retries an unknown outcome. The returned
    /// initialize and fund bytes must be persisted together by the SDK before
    /// either transaction is submitted.
    ///
    /// # Errors
    ///
    /// Fails closed on role, runtime, chain, program, signer, asset, response
    /// context, transport, or exact-plan validation mismatches.
    pub async fn prepare_native_first_lock(
        &self,
        agreement: &ZecAgreementV1,
        request_id: RequestId,
    ) -> Result<FirstLockPlanV1, PrepareNativeFirstLockError<T::Error>> {
        if self.local_participant != agreement.lez_depositor() {
            return Err(PrepareNativeFirstLockError::WrongDepositor);
        }
        validate_runtime_binding(agreement, &self.runtime, self.local_participant)
            .map_err(map_runtime_prepare_error)?;
        let authenticated_transfer_program_id = match agreement.lez_terms().asset() {
            LezAssetV1::Native {
                authenticated_transfer_program_id,
            } => Hex32::from_bytes(program_id_bytes(authenticated_transfer_program_id)),
            LezAssetV1::FungibleToken { .. } => {
                return Err(PrepareNativeFirstLockError::UnsupportedAsset);
            }
        };
        let context = MessageContext::new(
            self.run_id.clone(),
            request_id,
            bridge_participant(self.local_participant),
        );
        let terms = NativeEscrowTerms::new(NativeEscrowTermsInput {
            swap_id: Hex32::from_bytes(*agreement.onchain_swap_id()),
            terms_hash: Hex32::from_bytes(*agreement.agreement_commitment()),
            secret_digest: Hex32::from_bytes(*agreement.secret_digest()),
            depositor: bridge_participant(agreement.lez_depositor()),
            depositor_account_id: Hex32::from_bytes(
                *agreement.lez_account(agreement.lez_depositor()),
            ),
            claimant: bridge_participant(agreement.lez_claimant()),
            claimant_account_id: Hex32::from_bytes(
                *agreement.lez_account(agreement.lez_claimant()),
            ),
            amount: agreement.lez_terms().amount(),
            refund_at_ms: agreement.lez_refund_at_ms(),
            authenticated_transfer_program_id,
        })
        .map_err(PrepareNativeFirstLockError::Protocol)?;
        let response = self
            .transport
            .prepare_native_escrow(PrepareNativeEscrowRequest::new(
                context.clone(),
                self.runtime.clone(),
                terms,
            ))
            .await
            .map_err(PrepareNativeFirstLockError::Transport)?;
        if response.context != context {
            return Err(PrepareNativeFirstLockError::ResponseContextMismatch);
        }
        let initialize = PreparedFirstLockSubmissionV1::new(
            FirstLockStepV1::LezInitialize,
            *response.initialization.transaction_id.as_bytes(),
            response.initialization.exact_bytes.into_vec(),
        )
        .map_err(PrepareNativeFirstLockError::FirstLockPlan)?;
        let fund = PreparedFirstLockSubmissionV1::new(
            FirstLockStepV1::LezFund,
            *response.funding.transaction_id.as_bytes(),
            response.funding.exact_bytes.into_vec(),
        )
        .map_err(PrepareNativeFirstLockError::FirstLockPlan)?;
        FirstLockPlanV1::lez(initialize, fund).map_err(PrepareNativeFirstLockError::FirstLockPlan)
    }
}

impl<T: LezBridgeFirstLockTransport> LezBridgeAdapter<T> {
    /// Observes one exact owner step while retaining the complete durable LEZ plan.
    ///
    /// Both initialize and fund identities are sent on every query. Initialization
    /// can be confirmed independently so the SDK may proceed to funding, while
    /// funding is confirmed only after the complete ordered pair passes validation.
    ///
    /// # Errors
    ///
    /// Rejects role, runtime, plan, response, or canonical observation mismatches
    /// and preserves a single transport-attempt failure.
    // Keeping both ordered steps together makes the no-fund-before-initialize
    // safety invariant auditable in one state machine.
    #[allow(clippy::too_many_lines)]
    pub async fn observe_native_first_lock_step(
        &self,
        agreement: &ZecAgreementV1,
        request_id: RequestId,
        plan: &FirstLockPlanV1,
        submission: &PreparedFirstLockSubmissionV1,
    ) -> Result<FirstLockObservation, ObserveNativeEscrowError<T::Error>> {
        if self.local_participant != agreement.lez_depositor() {
            return Err(ObserveNativeEscrowError::ExactTargetRequiresDepositor);
        }
        let (initialize, fund) = complete_lez_plan(plan, submission)?;
        validate_runtime_binding(agreement, &self.runtime, self.local_participant)
            .map_err(map_runtime_observation_error)?;
        let terms = native_terms(agreement).map_err(|error| match error {
            NativeTermsError::UnsupportedAsset => ObserveNativeEscrowError::UnsupportedAsset,
            NativeTermsError::Protocol(source) => ObserveNativeEscrowError::Protocol(source),
        })?;
        let target = EscrowObservationTarget::Exact {
            initialization_transaction_id: TransactionId::from_bytes(
                *initialize.expected_submission_id(),
            ),
            funding_transaction_id: TransactionId::from_bytes(*fund.expected_submission_id()),
        };
        let context = MessageContext::new(
            self.run_id.clone(),
            request_id,
            bridge_participant(self.local_participant),
        );
        let response = LezBridgeFirstLockTransport::observe_escrow(
            &self.transport,
            ObserveEscrowRequest::new(context.clone(), self.runtime.clone(), terms.clone(), target),
        )
        .await
        .map_err(ObserveNativeEscrowError::Transport)?;
        if response.context != context {
            return Err(ObserveNativeEscrowError::ResponseContextMismatch);
        }
        if response.tip_before != response.tip_after {
            return Err(ObserveNativeEscrowError::UnstableTip);
        }

        match submission.step() {
            FirstLockStepV1::LezInitialize => match &response.initialization {
                InitializationObservation::Absent => {
                    if matches!(&response.funding, FundingObservation::Found(_)) {
                        Err(ObserveNativeEscrowError::InconsistentFacts)
                    } else {
                        Ok(FirstLockObservation::Absent)
                    }
                }
                InitializationObservation::UnknownOrPending => {
                    if matches!(
                        &response.funding,
                        FundingObservation::Absent | FundingObservation::UnknownOrPending
                    ) {
                        Ok(FirstLockObservation::Absent)
                    } else {
                        Err(ObserveNativeEscrowError::InconsistentFacts)
                    }
                }
                InitializationObservation::Found(initialization) => {
                    validate_found_initialization(
                        agreement,
                        &terms,
                        &response,
                        initialization,
                        initialize,
                    )?;
                    if let FundingObservation::Found(funding) = &response.funding {
                        validate_found_pair(
                            agreement,
                            &terms,
                            &target,
                            &response,
                            initialization,
                            funding,
                        )?;
                        validate_prepared_pair(initialization, funding, initialize, fund)?;
                    }
                    confirmed_first_lock_observation(
                        submission,
                        &initialization.transaction,
                        response.tip_after.height,
                    )
                }
            },
            FirstLockStepV1::LezFund => match (&response.initialization, &response.funding) {
                (
                    InitializationObservation::Found(initialization),
                    FundingObservation::Absent | FundingObservation::UnknownOrPending,
                ) => {
                    validate_found_initialization(
                        agreement,
                        &terms,
                        &response,
                        initialization,
                        initialize,
                    )?;
                    Ok(FirstLockObservation::Absent)
                }
                (
                    InitializationObservation::Found(initialization),
                    FundingObservation::Found(funding),
                ) => {
                    validate_found_pair(
                        agreement,
                        &terms,
                        &target,
                        &response,
                        initialization,
                        funding,
                    )?;
                    validate_prepared_pair(initialization, funding, initialize, fund)?;
                    let snapshot = canonical_snapshot(agreement, &response, funding);
                    CanonicalLezEscrowObservationV1::validate(agreement, &snapshot)
                        .map_err(ObserveNativeEscrowError::Canonical)?;
                    confirmed_first_lock_observation(
                        submission,
                        &funding.transaction,
                        response.tip_after.height,
                    )
                }
                (InitializationObservation::Absent, FundingObservation::Found(_)) => {
                    Err(ObserveNativeEscrowError::InconsistentFacts)
                }
                (InitializationObservation::UnknownOrPending, _)
                | (
                    InitializationObservation::Absent,
                    FundingObservation::Absent | FundingObservation::UnknownOrPending,
                ) => Ok(FirstLockObservation::Unstable),
            },
            FirstLockStepV1::ZcashFund => Err(ObserveNativeEscrowError::PreparedPlanMismatch),
        }
    }

    /// Submits one exact step from the complete durable LEZ plan once.
    ///
    /// # Errors
    ///
    /// Rejects role, runtime, plan, or exact-byte mismatches and preserves a
    /// conservative unknown outcome after the single transport attempt.
    pub async fn submit_native_first_lock_step(
        &self,
        agreement: &ZecAgreementV1,
        request_id: RequestId,
        plan: &FirstLockPlanV1,
        submission: &PreparedFirstLockSubmissionV1,
    ) -> Result<NativeFirstLockSubmitOutcome, PrepareNativeFirstLockError<T::Error>> {
        if self.local_participant != agreement.lez_depositor() {
            return Err(PrepareNativeFirstLockError::WrongDepositor);
        }
        validate_runtime_binding(agreement, &self.runtime, self.local_participant)
            .map_err(map_runtime_prepare_error)?;
        match native_terms(agreement) {
            Ok(_) => {}
            Err(NativeTermsError::UnsupportedAsset) => {
                return Err(PrepareNativeFirstLockError::UnsupportedAsset);
            }
            Err(NativeTermsError::Protocol(source)) => {
                return Err(PrepareNativeFirstLockError::Protocol(source));
            }
        }
        let (initialize, fund) = complete_lez_plan_for_submit(plan, submission)?;
        let selected = match submission.step() {
            FirstLockStepV1::LezInitialize => initialize,
            FirstLockStepV1::LezFund => fund,
            FirstLockStepV1::ZcashFund => {
                return Err(PrepareNativeFirstLockError::PreparedPlanMismatch);
            }
        };
        let transaction = PreparedTransaction::new(
            TransactionId::from_bytes(*selected.expected_submission_id()),
            ExactTransactionBytes::new(selected.exact_submission().to_vec())
                .map_err(PrepareNativeFirstLockError::Protocol)?,
        );
        let context = MessageContext::new(
            self.run_id.clone(),
            request_id,
            bridge_participant(self.local_participant),
        );
        let Ok(response) = LezBridgeFirstLockTransport::submit_transaction(
            &self.transport,
            SubmitTransactionRequest::new(
                context.clone(),
                self.runtime.clone(),
                transaction.clone(),
            ),
        )
        .await
        else {
            return Ok(NativeFirstLockSubmitOutcome::Unknown);
        };
        if response.context != context || response.transaction_id != transaction.transaction_id {
            return Ok(NativeFirstLockSubmitOutcome::Unknown);
        }
        Ok(match response.outcome {
            SubmissionOutcome::Accepted | SubmissionOutcome::AlreadyKnown => {
                NativeFirstLockSubmitOutcome::Accepted
            }
        })
    }
}

impl<T: LezBridgeObservationTransport> LezBridgeAdapter<T> {
    /// Observes a native taker-funded LEZ escrow in one transport attempt.
    ///
    /// The caller durably owns both the request ID and either the exact role-local
    /// transaction IDs or the bounded counterparty discovery window. Primitive
    /// sidecar facts are checked for full initialization/funding consistency and
    /// then passed through the SDK's public canonical agreement validator.
    ///
    /// This intentionally does not implement `LezTakerFirstLockObservationPort`:
    /// that port cannot carry a caller-owned request ID or a bounded discovery
    /// window. A higher composition layer must durably allocate those values.
    ///
    /// # Errors
    ///
    /// Fails closed on role, runtime, target ownership, response context, tip,
    /// transaction, instruction, signer, account, metadata, custody, or canonical
    /// agreement mismatches. Transport calls are never retried.
    pub async fn observe_native_escrow(
        &self,
        agreement: &ZecAgreementV1,
        request_id: RequestId,
        target: EscrowObservationTarget,
    ) -> Result<TakerFirstLockObservationV1, ObserveNativeEscrowError<T::Error>> {
        use lez_swap_core::SwapDirection;

        if agreement.direction() != SwapDirection::TakerSellsLez {
            return Err(ObserveNativeEscrowError::WrongDirection);
        }
        self.observe_native_escrow_target(agreement, request_id, target)
            .await
    }

    /// Discovers the maker-funded LEZ escrow for the reverse product direction.
    ///
    /// The taker has no maker-local prepared plan, so the expected submission
    /// identity is asserted from the fully validated canonical node transaction.
    ///
    /// # Errors
    ///
    /// Rejects the wrong product direction or any runtime, transport, response,
    /// canonical observation, or evidence-construction failure.
    pub async fn observe_native_maker_lock(
        &self,
        agreement: &ZecAgreementV1,
        request_id: RequestId,
        window: DiscoveryWindow,
    ) -> Result<MakerLockObservationV1, ObserveNativeEscrowError<T::Error>> {
        use lez_swap_core::SwapDirection;

        if agreement.direction() != SwapDirection::TakerSellsForeign {
            return Err(ObserveNativeEscrowError::WrongDirection);
        }
        let observation = self
            .observe_native_escrow_target(
                agreement,
                request_id,
                EscrowObservationTarget::DiscoverByTerms { window },
            )
            .await?;
        match observation {
            TakerFirstLockObservationV1::Absent => Ok(MakerLockObservationV1::Absent),
            TakerFirstLockObservationV1::Unstable => Ok(MakerLockObservationV1::Unstable),
            TakerFirstLockObservationV1::CanonicalLez(canonical) => {
                let transaction_id = *canonical.transaction_id();
                FirstLockConfirmedEvidenceV1::new(
                    FirstLockStepV1::LezFund,
                    transaction_id,
                    encode_hex32(&transaction_id),
                    canonical.confirmations().get(),
                )
                .map(MakerLockObservationV1::Confirmed)
                .map_err(ObserveNativeEscrowError::FirstLock)
            }
            TakerFirstLockObservationV1::Confirmed(observed) => {
                let transaction_id = TransactionId::from_bytes(
                    *Hex32::from_hex(observed.transaction_id())
                        .map_err(ObserveNativeEscrowError::Protocol)?
                        .as_bytes(),
                );
                FirstLockConfirmedEvidenceV1::new(
                    FirstLockStepV1::LezFund,
                    *transaction_id.as_bytes(),
                    observed.transaction_id(),
                    observed.confirmations(),
                )
                .map(MakerLockObservationV1::Confirmed)
                .map_err(ObserveNativeEscrowError::FirstLock)
            }
            TakerFirstLockObservationV1::LezRemoved(_)
            | TakerFirstLockObservationV1::LezReplaced { .. }
            | TakerFirstLockObservationV1::CanonicalZcash(_)
            | TakerFirstLockObservationV1::ZcashRemoved(_)
            | TakerFirstLockObservationV1::ZcashReplaced { .. } => {
                Err(ObserveNativeEscrowError::InconsistentFacts)
            }
        }
    }

    async fn observe_native_escrow_target(
        &self,
        agreement: &ZecAgreementV1,
        request_id: RequestId,
        target: EscrowObservationTarget,
    ) -> Result<TakerFirstLockObservationV1, ObserveNativeEscrowError<T::Error>> {
        match target {
            EscrowObservationTarget::Exact { .. }
                if self.local_participant != agreement.lez_depositor() =>
            {
                return Err(ObserveNativeEscrowError::ExactTargetRequiresDepositor);
            }
            EscrowObservationTarget::DiscoverByTerms { .. }
                if self.local_participant != agreement.lez_claimant() =>
            {
                return Err(ObserveNativeEscrowError::DiscoveryRequiresClaimant);
            }
            EscrowObservationTarget::Exact { .. }
            | EscrowObservationTarget::DiscoverByTerms { .. } => {}
        }
        validate_runtime_binding(agreement, &self.runtime, self.local_participant)
            .map_err(map_runtime_observation_error)?;
        let terms = native_terms(agreement).map_err(|error| match error {
            NativeTermsError::UnsupportedAsset => ObserveNativeEscrowError::UnsupportedAsset,
            NativeTermsError::Protocol(source) => ObserveNativeEscrowError::Protocol(source),
        })?;
        let context = MessageContext::new(
            self.run_id.clone(),
            request_id,
            bridge_participant(self.local_participant),
        );
        let response = self
            .transport
            .observe_escrow(ObserveEscrowRequest::new(
                context.clone(),
                self.runtime.clone(),
                terms.clone(),
                target,
            ))
            .await
            .map_err(ObserveNativeEscrowError::Transport)?;
        if response.context != context {
            return Err(ObserveNativeEscrowError::ResponseContextMismatch);
        }
        if response.tip_before != response.tip_after {
            return Err(ObserveNativeEscrowError::UnstableTip);
        }
        match (&response.initialization, &response.funding) {
            (InitializationObservation::Absent, FundingObservation::Absent) => {
                if discovery_window_is_fully_covered(&target, response.tip_after.height) {
                    Ok(TakerFirstLockObservationV1::Absent)
                } else {
                    Ok(TakerFirstLockObservationV1::Unstable)
                }
            }
            (InitializationObservation::UnknownOrPending, _)
            | (_, FundingObservation::UnknownOrPending)
            | (InitializationObservation::Found(_), FundingObservation::Absent) => {
                Ok(TakerFirstLockObservationV1::Unstable)
            }
            (InitializationObservation::Absent, FundingObservation::Found(_)) => {
                Err(ObserveNativeEscrowError::InconsistentFacts)
            }
            (
                InitializationObservation::Found(initialization),
                FundingObservation::Found(funding),
            ) => {
                validate_found_pair(
                    agreement,
                    &terms,
                    &target,
                    &response,
                    initialization,
                    funding,
                )?;
                if agreement.direction() == lez_swap_core::SwapDirection::TakerSellsLez {
                    let snapshot = canonical_snapshot(agreement, &response, funding);
                    let canonical = CanonicalLezEscrowObservationV1::validate(agreement, &snapshot)
                        .map_err(ObserveNativeEscrowError::Canonical)?;
                    Ok(TakerFirstLockObservationV1::CanonicalLez(Box::new(
                        canonical,
                    )))
                } else {
                    let confirmations = response
                        .tip_after
                        .height
                        .checked_sub(funding.transaction.position.height)
                        .and_then(|distance| distance.checked_add(1))
                        .and_then(|depth| u32::try_from(depth).ok())
                        .ok_or(ObserveNativeEscrowError::InconsistentFacts)?;
                    ObservedTakerFirstLockEvidenceV1::new(
                        FirstLockStepV1::LezFund,
                        encode_hex32(funding.transaction.transaction_id.as_bytes()),
                        confirmations,
                    )
                    .map(TakerFirstLockObservationV1::Confirmed)
                    .map_err(|_| ObserveNativeEscrowError::InconsistentFacts)
                }
            }
        }
    }
}

impl<T: LezBridgeRefundTransport> LezBridgeAdapter<T> {
    /// Observes the signed depositor's exact native funding state and governing LEZ clock once.
    ///
    /// The caller durably owns the request ID. `StateOnly` never scans for a
    /// refund, and the adapter preserves the exact millisecond clock until the
    /// final conservative projection to the SDK's whole-second position.
    ///
    /// # Errors
    ///
    /// Fails before transport for a non-owner and fails closed on runtime, terms,
    /// response, clock, or account inconsistency.
    pub async fn observe_native_refund_eligibility(
        &self,
        agreement: &ZecAgreementV1,
        request_id: RequestId,
    ) -> Result<RefundEligibilityObservationV1, NativeRefundAdapterError<T::Error>> {
        self.validate_refund_owner(agreement)?;
        let terms = refund_terms(agreement, &self.runtime, self.local_participant)?;
        let context = self.refund_context(request_id);
        let response = self
            .transport
            .observe_native_refund(ObserveNativeRefundRequest::new(
                context.clone(),
                self.runtime.clone(),
                terms.clone(),
                NativeRefundObservationTarget::StateOnly,
            ))
            .await
            .map_err(NativeRefundAdapterError::Transport)?;
        validate_refund_response_context(&context, &response)?;
        if response.refund != NativeRefundObservation::NotRequested {
            return Err(NativeRefundAdapterError::InconsistentFacts);
        }
        let position = ChainPosition::lez_timestamp_from_milliseconds_floor(
            LezUnixMilliseconds::new(response.clock_after.timestamp_ms),
        );
        match validate_refund_accounts(agreement, &terms, &response.accounts)? {
            None | Some(EscrowState::Empty) => {
                Ok(RefundEligibilityObservationV1::FundingUnavailable(
                    RefundFundingWaitReasonV1::Absent,
                ))
            }
            Some(EscrowState::Funded) => Ok(RefundEligibilityObservationV1::canonical(position)),
            Some(EscrowState::Claimed | EscrowState::Refunded) => {
                Ok(RefundEligibilityObservationV1::FundingUnavailable(
                    RefundFundingWaitReasonV1::Spent,
                ))
            }
        }
    }

    /// Prepares one official native refund for the signed depositor exactly once.
    ///
    /// The caller-owned request ID is the durable idempotency key. The returned
    /// exact bytes and official-decoder identity must be persisted before submit.
    ///
    /// # Errors
    ///
    /// Fails before transport for a non-owner, runtime drift, or non-native terms;
    /// otherwise preserves unknown randomized-preparation outcomes as errors.
    pub async fn prepare_native_refund(
        &self,
        agreement: &ZecAgreementV1,
        request_id: RequestId,
    ) -> Result<PreparedRefundSubmissionV1, NativeRefundAdapterError<T::Error>> {
        self.validate_refund_owner(agreement)?;
        let terms = refund_terms(agreement, &self.runtime, self.local_participant)?;
        let context = self.refund_context(request_id);
        let response = self
            .transport
            .prepare_native_refund(PrepareNativeRefundRequest::new(
                context.clone(),
                self.runtime.clone(),
                terms,
            ))
            .await
            .map_err(NativeRefundAdapterError::Transport)?;
        if response.context != context {
            return Err(NativeRefundAdapterError::ResponseContextMismatch);
        }
        PreparedRefundSubmissionV1::new(
            RefundStepV1::Lez,
            *response.refund.transaction_id.as_bytes(),
            response.refund.exact_bytes.into_vec(),
        )
        .map_err(NativeRefundAdapterError::Refund)
    }

    /// Observes an exact durable native refund identity in a caller-owned window.
    ///
    /// # Errors
    ///
    /// Rejects non-owner exact lookup, substituted durable bytes/identity, and
    /// any inconsistent official transaction, account, clock, or window fact.
    pub async fn observe_prepared_native_refund(
        &self,
        agreement: &ZecAgreementV1,
        request_id: RequestId,
        prepared: &PreparedRefundSubmissionV1,
        window: DiscoveryWindow,
    ) -> Result<RefundObservationV1, NativeRefundAdapterError<T::Error>> {
        self.observe_prepared_native_refund_page(agreement, request_id, prepared, window)
            .await
            .map(NativeRefundPageObservation::into_observation)
    }

    pub(crate) async fn observe_prepared_native_refund_page(
        &self,
        agreement: &ZecAgreementV1,
        request_id: RequestId,
        prepared: &PreparedRefundSubmissionV1,
        window: DiscoveryWindow,
    ) -> Result<NativeRefundPageObservation, NativeRefundAdapterError<T::Error>> {
        if self.local_participant != agreement.lez_depositor() {
            return Err(NativeRefundAdapterError::ExactTargetRequiresOwner);
        }
        let transaction = prepared_refund_transaction(prepared)?;
        self.observe_native_refund_target(
            agreement,
            request_id,
            NativeRefundObservationTarget::Exact {
                refund_transaction_id: transaction.transaction_id,
                window,
            },
            Some(prepared),
        )
        .await
    }

    /// Discovers the signed depositor's permissionless refund in a bounded window.
    ///
    /// # Errors
    ///
    /// Rejects owner use of the observer path and all inconsistent official facts.
    pub async fn observe_counterparty_native_refund(
        &self,
        agreement: &ZecAgreementV1,
        request_id: RequestId,
        window: DiscoveryWindow,
    ) -> Result<RefundObservationV1, NativeRefundAdapterError<T::Error>> {
        self.observe_counterparty_native_refund_page(agreement, request_id, window)
            .await
            .map(NativeRefundPageObservation::into_observation)
    }

    pub(crate) async fn observe_counterparty_native_refund_page(
        &self,
        agreement: &ZecAgreementV1,
        request_id: RequestId,
        window: DiscoveryWindow,
    ) -> Result<NativeRefundPageObservation, NativeRefundAdapterError<T::Error>> {
        if self.local_participant != agreement.lez_claimant() {
            return Err(NativeRefundAdapterError::DiscoveryRequiresClaimant);
        }
        self.observe_native_refund_target(
            agreement,
            request_id,
            NativeRefundObservationTarget::DiscoverByTerms { window },
            None,
        )
        .await
    }

    /// Submits exact durable native refund bytes once through generic submit.
    ///
    /// A transport failure or mismatched post-submit response is returned as
    /// [`RefundSubmitOutcomeV1::Unknown`], never rejection and never a retry.
    ///
    /// # Errors
    ///
    /// Fails before transport for a non-owner, runtime drift, non-native terms,
    /// wrong refund step, or malformed durable bytes.
    pub async fn submit_native_refund(
        &self,
        agreement: &ZecAgreementV1,
        request_id: RequestId,
        prepared: &PreparedRefundSubmissionV1,
    ) -> Result<RefundSubmitOutcomeV1, NativeRefundAdapterError<T::Error>> {
        self.validate_refund_owner(agreement)?;
        let _terms = refund_terms(agreement, &self.runtime, self.local_participant)?;
        let transaction = prepared_refund_transaction(prepared)?;
        let context = self.refund_context(request_id);
        let Ok(response) = self
            .transport
            .submit_transaction(SubmitTransactionRequest::new(
                context.clone(),
                self.runtime.clone(),
                transaction.clone(),
            ))
            .await
        else {
            return Ok(RefundSubmitOutcomeV1::Unknown);
        };
        if response.context != context || response.transaction_id != transaction.transaction_id {
            return Ok(RefundSubmitOutcomeV1::Unknown);
        }
        Ok(match response.outcome {
            SubmissionOutcome::Accepted | SubmissionOutcome::AlreadyKnown => {
                RefundSubmitOutcomeV1::Accepted
            }
        })
    }

    async fn observe_native_refund_target(
        &self,
        agreement: &ZecAgreementV1,
        request_id: RequestId,
        target: NativeRefundObservationTarget,
        prepared: Option<&PreparedRefundSubmissionV1>,
    ) -> Result<NativeRefundPageObservation, NativeRefundAdapterError<T::Error>> {
        let terms = refund_terms(agreement, &self.runtime, self.local_participant)?;
        let context = self.refund_context(request_id);
        let response = self
            .transport
            .observe_native_refund(ObserveNativeRefundRequest::new(
                context.clone(),
                self.runtime.clone(),
                terms.clone(),
                target,
            ))
            .await
            .map_err(NativeRefundAdapterError::Transport)?;
        validate_refund_response_context(&context, &response)?;
        let account_state = validate_refund_accounts(agreement, &terms, &response.accounts)?;
        match &response.refund {
            NativeRefundObservation::NotRequested => {
                Err(NativeRefundAdapterError::InconsistentFacts)
            }
            NativeRefundObservation::Absent => {
                let fully_covered_miss =
                    refund_window_is_fully_covered(target, response.clock_after.height);
                if matches!(
                    account_state,
                    Some(EscrowState::Claimed | EscrowState::Refunded)
                ) {
                    return Ok(NativeRefundPageObservation::new(
                        RefundObservationV1::Unstable,
                        fully_covered_miss,
                    ));
                }
                let observation = if fully_covered_miss {
                    RefundObservationV1::Absent
                } else {
                    RefundObservationV1::Unstable
                };
                Ok(NativeRefundPageObservation::new(
                    observation,
                    fully_covered_miss,
                ))
            }
            NativeRefundObservation::UnknownOrPending => {
                let observation = if matches!(target, NativeRefundObservationTarget::Exact { .. })
                    && account_state == Some(EscrowState::Funded)
                {
                    RefundObservationV1::Absent
                } else {
                    RefundObservationV1::Unstable
                };
                Ok(NativeRefundPageObservation::new(observation, false))
            }
            NativeRefundObservation::Found(found) => {
                if account_state != Some(EscrowState::Refunded) {
                    return Err(NativeRefundAdapterError::InconsistentFacts);
                }
                let evidence =
                    validate_refund_found(agreement, &terms, target, &response, found, prepared)?;
                Ok(NativeRefundPageObservation::new(
                    RefundObservationV1::Confirmed(evidence),
                    false,
                ))
            }
        }
    }

    fn refund_context(&self, request_id: RequestId) -> MessageContext {
        MessageContext::new(
            self.run_id.clone(),
            request_id,
            bridge_participant(self.local_participant),
        )
    }

    fn validate_refund_owner(
        &self,
        agreement: &ZecAgreementV1,
    ) -> Result<(), NativeRefundAdapterError<T::Error>> {
        if self.local_participant != agreement.lez_depositor() {
            return Err(NativeRefundAdapterError::WrongOwner);
        }
        Ok(())
    }
}

impl<T: LezBridgeClaimTransport> LezBridgeAdapter<T> {
    /// Prepares one official native revealing claim for the signed claimant.
    ///
    /// The caller durably owns the request ID and retained funding identity. The
    /// preimage is copied once into the protocol's redacted bounded wrapper; the
    /// returned secret-bearing exact bytes must immediately enter protected storage.
    ///
    /// # Errors
    ///
    /// Fails before transport for role/runtime/terms drift and preserves an
    /// unknown one-attempt preparation outcome as a typed error.
    pub async fn prepare_native_revealing_claim(
        &self,
        agreement: &ZecAgreementV1,
        request_id: RequestId,
        funding_transaction_id: TransactionId,
        preimage: &ClaimPreimage,
    ) -> Result<PreparedClaimSubmissionV1, NativeRevealingClaimAdapterError<T::Error>> {
        self.validate_claimant(agreement)?;
        let terms = claim_terms(agreement, &self.runtime, self.local_participant)?;
        if funding_transaction_id.as_bytes() == &[0; 32] {
            return Err(NativeRevealingClaimAdapterError::EmptyFundingTransactionId);
        }
        let context = self.claim_context(request_id);
        let response = self
            .transport
            .prepare_revealing_claim(PrepareRevealingClaimRequest::new(
                context.clone(),
                self.runtime.clone(),
                terms,
                funding_transaction_id,
                RevealingPreimage::new(*preimage.expose_secret()),
            ))
            .await
            .map_err(NativeRevealingClaimAdapterError::Transport)?;
        if response.context != context {
            return Err(NativeRevealingClaimAdapterError::ResponseContextMismatch);
        }
        PreparedClaimSubmissionV1::new(
            ClaimStepV1::RevealingLez,
            *response.claim.transaction_id.as_bytes(),
            response.claim.exact_bytes.into_vec(),
        )
        .map_err(NativeRevealingClaimAdapterError::Claim)
    }

    /// Observes the claimant's exact protected revealing-claim identity once.
    ///
    /// # Errors
    ///
    /// Rejects non-claimant use, a non-LEZ durable submission, substituted exact
    /// identity/bytes, and any noncanonical primitive claim fact.
    pub async fn observe_prepared_native_revealing_claim(
        &self,
        agreement: &ZecAgreementV1,
        request_id: RequestId,
        prepared: &PreparedClaimSubmissionV1,
    ) -> Result<RevealingClaimObservationV1, NativeRevealingClaimAdapterError<T::Error>> {
        if self.local_participant != agreement.lez_claimant() {
            return Err(NativeRevealingClaimAdapterError::ExactTargetRequiresClaimant);
        }
        let transaction = prepared_claim_transaction(prepared)?;
        self.observe_native_revealing_claim_target(
            agreement,
            request_id,
            RevealingClaimObservationTarget::Exact {
                claim_transaction_id: transaction.transaction_id,
            },
            Some(prepared),
        )
        .await
    }

    /// Discovers the counterparty claimant's reveal in one caller-owned window.
    ///
    /// # Errors
    ///
    /// Rejects claimant use of the observer path and all inconsistent official facts.
    pub async fn observe_counterparty_native_revealing_claim(
        &self,
        agreement: &ZecAgreementV1,
        request_id: RequestId,
        window: DiscoveryWindow,
    ) -> Result<RevealingClaimObservationV1, NativeRevealingClaimAdapterError<T::Error>> {
        if self.local_participant != agreement.lez_depositor() {
            return Err(NativeRevealingClaimAdapterError::DiscoveryRequiresDepositor);
        }
        self.observe_native_revealing_claim_target(
            agreement,
            request_id,
            RevealingClaimObservationTarget::DiscoverByTerms { window },
            None,
        )
        .await
    }

    /// Submits exact protected revealing-claim bytes once through generic submit.
    ///
    /// Transport failure or mismatched acknowledgement is `Unknown`, never a
    /// rejection and never an internal retry.
    ///
    /// # Errors
    ///
    /// Fails before transport for a non-claimant, runtime drift, non-native terms,
    /// or malformed/non-LEZ durable bytes.
    pub async fn submit_native_revealing_claim(
        &self,
        agreement: &ZecAgreementV1,
        request_id: RequestId,
        prepared: &PreparedClaimSubmissionV1,
    ) -> Result<RevealingClaimSubmitOutcome, NativeRevealingClaimAdapterError<T::Error>> {
        self.validate_claimant(agreement)?;
        let _terms = claim_terms(agreement, &self.runtime, self.local_participant)?;
        let transaction = prepared_claim_transaction(prepared)?;
        let context = self.claim_context(request_id);
        let Ok(response) = self
            .transport
            .submit_transaction(SubmitTransactionRequest::new(
                context.clone(),
                self.runtime.clone(),
                transaction.clone(),
            ))
            .await
        else {
            return Ok(RevealingClaimSubmitOutcome::Unknown);
        };
        if response.context != context || response.transaction_id != transaction.transaction_id {
            return Ok(RevealingClaimSubmitOutcome::Unknown);
        }
        Ok(match response.outcome {
            SubmissionOutcome::Accepted | SubmissionOutcome::AlreadyKnown => {
                RevealingClaimSubmitOutcome::Accepted
            }
        })
    }

    async fn observe_native_revealing_claim_target(
        &self,
        agreement: &ZecAgreementV1,
        request_id: RequestId,
        target: RevealingClaimObservationTarget,
        prepared: Option<&PreparedClaimSubmissionV1>,
    ) -> Result<RevealingClaimObservationV1, NativeRevealingClaimAdapterError<T::Error>> {
        let terms = claim_terms(agreement, &self.runtime, self.local_participant)?;
        let context = self.claim_context(request_id);
        let response = self
            .transport
            .observe_revealing_claim(ObserveRevealingClaimRequest::new(
                context.clone(),
                self.runtime.clone(),
                terms,
                target,
            ))
            .await
            .map_err(NativeRevealingClaimAdapterError::Transport)?;
        if response.context != context {
            return Err(NativeRevealingClaimAdapterError::ResponseContextMismatch);
        }
        if response.tip_before != response.tip_after {
            return Ok(RevealingClaimObservationV1::Unstable);
        }
        match &response.claim {
            RevealingClaimObservation::Absent => {
                if claim_discovery_window_is_fully_covered(target, response.tip_after.height) {
                    Ok(RevealingClaimObservationV1::Absent)
                } else {
                    Ok(RevealingClaimObservationV1::Unstable)
                }
            }
            RevealingClaimObservation::UnknownOrPending => {
                Ok(RevealingClaimObservationV1::Unstable)
            }
            RevealingClaimObservation::Found(found) => {
                validate_claim_found(agreement, target, &response, found, prepared)
                    .map(RevealingClaimObservationV1::Confirmed)
            }
        }
    }

    fn claim_context(&self, request_id: RequestId) -> MessageContext {
        MessageContext::new(
            self.run_id.clone(),
            request_id,
            bridge_participant(self.local_participant),
        )
    }

    fn validate_claimant(
        &self,
        agreement: &ZecAgreementV1,
    ) -> Result<(), NativeRevealingClaimAdapterError<T::Error>> {
        if self.local_participant != agreement.lez_claimant() {
            return Err(NativeRevealingClaimAdapterError::WrongClaimant);
        }
        Ok(())
    }
}

#[derive(Debug)]
enum NativeTermsError {
    UnsupportedAsset,
    Protocol(ProtocolValueError),
}

/// Failure binding a signed agreement to one role-local LEZ runtime.
///
/// Variants intentionally carry no compared runtime or agreement values, so
/// diagnostics can identify the failed policy without disclosing identities.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum LezRuntimeBindingError {
    /// The signed environment or selected compatibility graph is unsupported.
    #[error("signed LEZ environment is not compatible with this bridge")]
    IncompatibleEnvironment,
    /// The signed channel or genesis identity differs from the selected runtime.
    #[error("signed LEZ chain identity differs from the selected runtime")]
    ChainIdentityMismatch,
    /// The signed escrow program differs from the selected runtime.
    #[error("signed LEZ escrow program differs from the selected runtime")]
    EscrowProgramMismatch,
    /// The role-local sidecar signer is not the account in the signed agreement.
    #[error("LEZ sidecar signer differs from the signed local account")]
    SignerAccountMismatch,
}

/// Validates signed agreement fields against an already role-checked runtime.
///
/// This is the reusable signed-terms check for callers that must reject a
/// sidecar before constructing chain ports. It accepts the SDK's validated
/// agreement, the sidecar's bounded runtime description, and the local actor
/// role; no raw agreement or private sidecar wire messages cross this API. The
/// caller must independently require `runtime.sidecar_role` to equal the local
/// participant. [`LezBridgeAdapter::new`] performs that separate process-role
/// binding for constructed adapters.
///
/// # Errors
///
/// Returns a payload-free category when environment/compatibility, chain,
/// escrow-program, or local signer identity does not match.
pub fn validate_runtime_binding(
    agreement: &ZecAgreementV1,
    runtime: &RuntimeDescriptor,
    local_participant: Participant,
) -> Result<(), LezRuntimeBindingError> {
    let signed_chain = agreement.lez_terms().chain();
    let compatible_generation =
        runtime_generation_is_compatible(signed_chain.environment(), runtime.compatibility);
    if !compatible_generation {
        return Err(LezRuntimeBindingError::IncompatibleEnvironment);
    }
    if runtime.channel_id != Hex32::from_bytes(*signed_chain.channel_id())
        || runtime.genesis_block_hash != Hex32::from_bytes(*signed_chain.genesis_block_hash())
    {
        return Err(LezRuntimeBindingError::ChainIdentityMismatch);
    }
    if runtime.escrow_program_id
        != Hex32::from_bytes(program_id_bytes(agreement.lez_terms().escrow_program_id()))
    {
        return Err(LezRuntimeBindingError::EscrowProgramMismatch);
    }
    if runtime.signer_account_id != Hex32::from_bytes(*agreement.lez_account(local_participant)) {
        return Err(LezRuntimeBindingError::SignerAccountMismatch);
    }
    Ok(())
}

fn map_runtime_observation_error<E: std::error::Error + 'static>(
    error: LezRuntimeBindingError,
) -> ObserveNativeEscrowError<E> {
    match error {
        LezRuntimeBindingError::IncompatibleEnvironment => {
            ObserveNativeEscrowError::IncompatibleEnvironment
        }
        LezRuntimeBindingError::ChainIdentityMismatch => {
            ObserveNativeEscrowError::ChainIdentityMismatch
        }
        LezRuntimeBindingError::EscrowProgramMismatch => {
            ObserveNativeEscrowError::EscrowProgramMismatch
        }
        LezRuntimeBindingError::SignerAccountMismatch => {
            ObserveNativeEscrowError::SignerAccountMismatch
        }
    }
}

fn map_runtime_prepare_error<E: std::error::Error + 'static>(
    error: LezRuntimeBindingError,
) -> PrepareNativeFirstLockError<E> {
    match error {
        LezRuntimeBindingError::IncompatibleEnvironment => {
            PrepareNativeFirstLockError::IncompatibleEnvironment
        }
        LezRuntimeBindingError::ChainIdentityMismatch => {
            PrepareNativeFirstLockError::ChainIdentityMismatch
        }
        LezRuntimeBindingError::EscrowProgramMismatch => {
            PrepareNativeFirstLockError::EscrowProgramMismatch
        }
        LezRuntimeBindingError::SignerAccountMismatch => {
            PrepareNativeFirstLockError::SignerAccountMismatch
        }
    }
}

fn complete_lez_plan<'a, E: std::error::Error + 'static>(
    plan: &'a FirstLockPlanV1,
    submission: &PreparedFirstLockSubmissionV1,
) -> Result<
    (
        &'a PreparedFirstLockSubmissionV1,
        &'a PreparedFirstLockSubmissionV1,
    ),
    ObserveNativeEscrowError<E>,
> {
    let FirstLockPlanV1::Lez { initialize, fund } = plan else {
        return Err(ObserveNativeEscrowError::PreparedPlanMismatch);
    };
    if submission != initialize && submission != fund {
        return Err(ObserveNativeEscrowError::PreparedPlanMismatch);
    }
    Ok((initialize, fund))
}

fn complete_lez_plan_for_submit<'a, E: std::error::Error + 'static>(
    plan: &'a FirstLockPlanV1,
    submission: &PreparedFirstLockSubmissionV1,
) -> Result<
    (
        &'a PreparedFirstLockSubmissionV1,
        &'a PreparedFirstLockSubmissionV1,
    ),
    PrepareNativeFirstLockError<E>,
> {
    let FirstLockPlanV1::Lez { initialize, fund } = plan else {
        return Err(PrepareNativeFirstLockError::PreparedPlanMismatch);
    };
    if submission != initialize && submission != fund {
        return Err(PrepareNativeFirstLockError::PreparedPlanMismatch);
    }
    Ok((initialize, fund))
}

fn validate_found_initialization<E: std::error::Error + 'static>(
    agreement: &ZecAgreementV1,
    terms: &NativeEscrowTerms,
    response: &ObserveEscrowResult,
    initialization: &InitializationFoundFacts,
    prepared: &PreparedFirstLockSubmissionV1,
) -> Result<(), ObserveNativeEscrowError<E>> {
    if prepared.step() != FirstLockStepV1::LezInitialize {
        return Err(ObserveNativeEscrowError::PreparedPlanMismatch);
    }
    let transaction = &initialization.transaction;
    let depositor = Hex32::from_bytes(*agreement.lez_account(agreement.lez_depositor()));
    let expected_signers = AccountIds::new(vec![depositor]).expect("one signer is bounded");
    let expected_accounts = AccountIds::new(vec![
        Hex32::from_bytes(*agreement.lez_terms().metadata_account()),
        Hex32::from_bytes(*agreement.lez_terms().custody_account()),
        depositor,
        Hex32::from_bytes(*agreement.lez_account(agreement.lez_claimant())),
    ])
    .expect("four accounts are bounded");
    let escrow_program =
        Hex32::from_bytes(program_id_bytes(agreement.lez_terms().escrow_program_id()));
    if !matches!(
        initialization.metadata.status,
        EscrowState::Empty | EscrowState::Funded
    ) {
        return Err(ObserveNativeEscrowError::InconsistentFacts);
    }
    let expected_metadata = native_metadata_facts_for_agreement(
        agreement,
        Hex32::from_bytes(*agreement.lez_terms().metadata_account()),
        escrow_program,
        Hex32::from_bytes(*agreement.lez_terms().custody_account()),
        terms,
        initialization.metadata.status,
    );
    let same_height_wrong_hash = transaction.position.height == response.tip_after.height
        && transaction.position.block_hash != response.tip_after.block_hash;
    if transaction.transaction_id.as_bytes() != prepared.expected_submission_id()
        || transaction.exact_bytes.as_slice() != prepared.exact_submission()
        || !transaction.is_public
        || transaction.signer_account_ids != expected_signers
        || initialization.instruction.program_id != escrow_program
        || initialization.instruction.ordered_account_ids != expected_accounts
        || initialization.instruction.terms != *terms
        || initialization.metadata != expected_metadata
        || transaction.position.height > response.tip_after.height
        || same_height_wrong_hash
    {
        return Err(ObserveNativeEscrowError::InconsistentFacts);
    }
    Ok(())
}

fn validate_prepared_pair<E: std::error::Error + 'static>(
    initialization: &InitializationFoundFacts,
    funding: &FundingFoundFacts,
    prepared_initialize: &PreparedFirstLockSubmissionV1,
    prepared_fund: &PreparedFirstLockSubmissionV1,
) -> Result<(), ObserveNativeEscrowError<E>> {
    if initialization.transaction.transaction_id.as_bytes()
        != prepared_initialize.expected_submission_id()
        || initialization.transaction.exact_bytes.as_slice()
            != prepared_initialize.exact_submission()
        || funding.transaction.transaction_id.as_bytes() != prepared_fund.expected_submission_id()
        || funding.transaction.exact_bytes.as_slice() != prepared_fund.exact_submission()
    {
        return Err(ObserveNativeEscrowError::InconsistentFacts);
    }
    Ok(())
}

fn confirmed_first_lock_observation<E: std::error::Error + 'static>(
    submission: &PreparedFirstLockSubmissionV1,
    transaction: &ObservedTransactionFacts,
    tip_height: u64,
) -> Result<FirstLockObservation, ObserveNativeEscrowError<E>> {
    let confirmations = tip_height
        .checked_sub(transaction.position.height)
        .and_then(|distance| distance.checked_add(1))
        .and_then(|depth| u32::try_from(depth).ok())
        .ok_or(ObserveNativeEscrowError::InconsistentFacts)?;
    FirstLockConfirmedEvidenceV1::from_observation(
        submission.step(),
        *submission.expected_submission_id(),
        encode_hex32(transaction.transaction_id.as_bytes()),
        confirmations,
    )
    .map(FirstLockObservation::Confirmed)
    .map_err(ObserveNativeEscrowError::FirstLock)
}

fn native_terms(agreement: &ZecAgreementV1) -> Result<NativeEscrowTerms, NativeTermsError> {
    let authenticated_transfer_program_id = match agreement.lez_terms().asset() {
        LezAssetV1::Native {
            authenticated_transfer_program_id,
        } => Hex32::from_bytes(program_id_bytes(authenticated_transfer_program_id)),
        LezAssetV1::FungibleToken { .. } => return Err(NativeTermsError::UnsupportedAsset),
    };
    NativeEscrowTerms::new(NativeEscrowTermsInput {
        swap_id: Hex32::from_bytes(*agreement.onchain_swap_id()),
        terms_hash: Hex32::from_bytes(*agreement.agreement_commitment()),
        secret_digest: Hex32::from_bytes(*agreement.secret_digest()),
        depositor: bridge_participant(agreement.lez_depositor()),
        depositor_account_id: Hex32::from_bytes(*agreement.lez_account(agreement.lez_depositor())),
        claimant: bridge_participant(agreement.lez_claimant()),
        claimant_account_id: Hex32::from_bytes(*agreement.lez_account(agreement.lez_claimant())),
        amount: agreement.lez_terms().amount(),
        refund_at_ms: agreement.lez_refund_at_ms(),
        authenticated_transfer_program_id,
    })
    .map_err(NativeTermsError::Protocol)
}

fn refund_terms<E: std::error::Error + 'static>(
    agreement: &ZecAgreementV1,
    runtime: &RuntimeDescriptor,
    local_participant: Participant,
) -> Result<NativeEscrowTerms, NativeRefundAdapterError<E>> {
    validate_runtime_binding(agreement, runtime, local_participant).map_err(
        |error| match error {
            LezRuntimeBindingError::IncompatibleEnvironment => {
                NativeRefundAdapterError::IncompatibleEnvironment
            }
            LezRuntimeBindingError::ChainIdentityMismatch => {
                NativeRefundAdapterError::ChainIdentityMismatch
            }
            LezRuntimeBindingError::EscrowProgramMismatch => {
                NativeRefundAdapterError::EscrowProgramMismatch
            }
            LezRuntimeBindingError::SignerAccountMismatch => {
                NativeRefundAdapterError::SignerAccountMismatch
            }
        },
    )?;
    native_terms(agreement).map_err(|error| match error {
        NativeTermsError::UnsupportedAsset => NativeRefundAdapterError::UnsupportedAsset,
        NativeTermsError::Protocol(source) => NativeRefundAdapterError::Protocol(source),
    })
}

fn claim_terms<E: std::error::Error + 'static>(
    agreement: &ZecAgreementV1,
    runtime: &RuntimeDescriptor,
    local_participant: Participant,
) -> Result<NativeEscrowTerms, NativeRevealingClaimAdapterError<E>> {
    validate_runtime_binding(agreement, runtime, local_participant).map_err(
        |error| match error {
            LezRuntimeBindingError::IncompatibleEnvironment => {
                NativeRevealingClaimAdapterError::IncompatibleEnvironment
            }
            LezRuntimeBindingError::ChainIdentityMismatch => {
                NativeRevealingClaimAdapterError::ChainIdentityMismatch
            }
            LezRuntimeBindingError::EscrowProgramMismatch => {
                NativeRevealingClaimAdapterError::EscrowProgramMismatch
            }
            LezRuntimeBindingError::SignerAccountMismatch => {
                NativeRevealingClaimAdapterError::SignerAccountMismatch
            }
        },
    )?;
    native_terms(agreement).map_err(|error| match error {
        NativeTermsError::UnsupportedAsset => NativeRevealingClaimAdapterError::UnsupportedAsset,
        NativeTermsError::Protocol(source) => NativeRevealingClaimAdapterError::Protocol(source),
    })
}

fn prepared_claim_transaction<E: std::error::Error + 'static>(
    prepared: &PreparedClaimSubmissionV1,
) -> Result<PreparedTransaction, NativeRevealingClaimAdapterError<E>> {
    if prepared.step() != ClaimStepV1::RevealingLez {
        return Err(NativeRevealingClaimAdapterError::WrongPreparedStep);
    }
    let exact_bytes = ExactTransactionBytes::new(prepared.exact_submission().to_vec())
        .map_err(NativeRevealingClaimAdapterError::Protocol)?;
    Ok(PreparedTransaction::new(
        TransactionId::from_bytes(*prepared.expected_submission_id()),
        exact_bytes,
    ))
}

fn claim_discovery_window_is_fully_covered(
    target: RevealingClaimObservationTarget,
    tip_height: u64,
) -> bool {
    match target {
        RevealingClaimObservationTarget::Exact { .. } => true,
        RevealingClaimObservationTarget::DiscoverByTerms { window } => window
            .start_height()
            .checked_add(u64::from(window.max_blocks() - 1))
            .is_some_and(|final_height| final_height <= tip_height),
    }
}

fn validate_claim_found<E: std::error::Error + 'static>(
    agreement: &ZecAgreementV1,
    target: RevealingClaimObservationTarget,
    response: &ObserveRevealingClaimResult,
    found: &RevealingClaimFoundFacts,
    prepared: Option<&PreparedClaimSubmissionV1>,
) -> Result<RevealingClaimEvidenceV1, NativeRevealingClaimAdapterError<E>> {
    let transaction = &found.transaction;
    match target {
        RevealingClaimObservationTarget::Exact {
            claim_transaction_id,
        } if transaction.transaction_id != claim_transaction_id => {
            return Err(NativeRevealingClaimAdapterError::InconsistentFacts);
        }
        RevealingClaimObservationTarget::DiscoverByTerms { window } => {
            let final_height = window
                .start_height()
                .checked_add(u64::from(window.max_blocks() - 1))
                .expect("validated discovery window cannot overflow");
            if !(window.start_height()..=final_height).contains(&transaction.position.height) {
                return Err(NativeRevealingClaimAdapterError::InconsistentFacts);
            }
        }
        RevealingClaimObservationTarget::Exact { .. } => {}
    }
    if let Some(prepared) = prepared
        && (transaction.transaction_id.as_bytes() != prepared.expected_submission_id()
            || transaction.exact_bytes.as_slice() != prepared.exact_submission())
    {
        return Err(NativeRevealingClaimAdapterError::InconsistentFacts);
    }
    let same_height_wrong_hash = transaction.position.height == response.tip_after.height
        && transaction.position.block_hash != response.tip_after.block_hash;
    if same_height_wrong_hash {
        return Err(NativeRevealingClaimAdapterError::InconsistentFacts);
    }
    let snapshot = canonical_claim_snapshot(agreement, response, found);
    match prepared {
        Some(prepared) => RevealingClaimEvidenceV1::from_prepared_lez_claim_snapshot(
            agreement, prepared, snapshot,
        ),
        None => RevealingClaimEvidenceV1::from_lez_claim_snapshot(agreement, snapshot),
    }
    .map_err(NativeRevealingClaimAdapterError::Canonical)
}

fn canonical_claim_snapshot(
    agreement: &ZecAgreementV1,
    response: &ObserveRevealingClaimResult,
    found: &RevealingClaimFoundFacts,
) -> LezClaimNodeSnapshotV1 {
    let transaction = &found.transaction;
    let instruction = &found.instruction;
    let metadata = &found.metadata;
    let claimant = *agreement.lez_account(agreement.lez_claimant());
    LezClaimNodeSnapshotV1::new(
        agreement.lez_terms().chain().environment(),
        *agreement.lez_terms().chain().channel_id(),
        *agreement.lez_terms().chain().genesis_block_hash(),
        LezStableTipV1::new(
            *response.tip_before.block_hash.as_bytes(),
            response.tip_before.height,
            *response.tip_after.block_hash.as_bytes(),
            response.tip_after.height,
        ),
        LezClaimTransactionSnapshotV1::new(
            *transaction.transaction_id.as_bytes(),
            *transaction.transaction_id.as_bytes(),
            words_from_bytes(instruction.program_id.as_bytes()),
            claimant,
            instruction
                .ordered_account_ids
                .as_slice()
                .iter()
                .map(|account| *account.as_bytes())
                .collect(),
            LezClaimInstructionV1::Native {
                swap_id: *instruction.swap_id.as_bytes(),
                preimage: ClaimPreimage::new(*instruction.preimage.expose_secret()),
            },
            transaction.is_public,
            transaction.signer_account_ids.as_slice() == [Hex32::from_bytes(claimant)],
            transaction.position.height,
            *transaction.position.block_hash.as_bytes(),
            *transaction.position.block_hash.as_bytes(),
            // The compatibility bridge exposes stable placement but no settlement status.
            // Pending is the conservative projection accepted only by local compatibility.
            LezInclusionStatusV1::Pending,
        ),
        words_from_bytes(metadata.owner_program_id.as_bytes()),
        *metadata.account_id.as_bytes(),
        metadata_snapshot(metadata),
        *found.custody.account_id.as_bytes(),
        custody_snapshot(&found.custody),
    )
}

fn metadata_snapshot(metadata: &EscrowMetadataFacts) -> LezEscrowMetadataSnapshotV1 {
    LezEscrowMetadataSnapshotV1::new(
        metadata.version,
        *metadata.swap_id.as_bytes(),
        *metadata.terms_hash.as_bytes(),
        *metadata.secret_digest.as_bytes(),
        *metadata.depositor_account_id.as_bytes(),
        *metadata.depositor_asset_account_id.as_bytes(),
        *metadata.claimant_account_id.as_bytes(),
        *metadata.claimant_asset_account_id.as_bytes(),
        *metadata.custody_account_id.as_bytes(),
        words_from_bytes(metadata.asset_program_id.as_bytes()),
        words_from_bytes(metadata.custody_program_id.as_bytes()),
        *metadata.asset_definition.as_bytes(),
        metadata.amount.as_u128(),
        metadata.refund_at_ms,
        match metadata.status {
            EscrowState::Empty => LezEscrowStatusV1::Empty,
            EscrowState::Funded => LezEscrowStatusV1::Funded,
            EscrowState::Claimed => LezEscrowStatusV1::Claimed,
            EscrowState::Refunded => LezEscrowStatusV1::Refunded,
        },
    )
}

fn native_metadata_facts_for_agreement(
    agreement: &ZecAgreementV1,
    account_id: Hex32,
    owner_program_id: Hex32,
    custody_account_id: Hex32,
    terms: &NativeEscrowTerms,
    status: EscrowState,
) -> EscrowMetadataFacts {
    match agreement.lez_terms().chain().environment() {
        LezEnvironmentV1::DeterministicLocalV0_1_2Compatibility => {
            EscrowMetadataFacts::from_nssa_v0_1_2_native_terms(
                account_id,
                owner_program_id,
                custody_account_id,
                terms,
                status,
            )
        }
        LezEnvironmentV1::DeterministicLocalV0_2 | LezEnvironmentV1::PublicTestnetV0_2 => {
            EscrowMetadataFacts::from_lee_v0_2_native_terms(
                account_id,
                owner_program_id,
                custody_account_id,
                terms,
                status,
            )
        }
    }
}

fn custody_snapshot(custody: &NativeCustodyFacts) -> LezCustodySnapshotV1 {
    LezCustodySnapshotV1::Native {
        program_owner: words_from_bytes(custody.owner_program_id.as_bytes()),
        balance: custody.balance.as_u128(),
    }
}

fn validate_refund_response_context<E: std::error::Error + 'static>(
    context: &MessageContext,
    response: &ObserveNativeRefundResult,
) -> Result<(), NativeRefundAdapterError<E>> {
    if response.context != *context {
        return Err(NativeRefundAdapterError::ResponseContextMismatch);
    }
    if response.clock_before != response.clock_after {
        return Err(NativeRefundAdapterError::UnstableClock);
    }
    Ok(())
}

fn validate_refund_accounts<E: std::error::Error + 'static>(
    agreement: &ZecAgreementV1,
    terms: &NativeEscrowTerms,
    observation: &NativeEscrowAccountObservation,
) -> Result<Option<EscrowState>, NativeRefundAdapterError<E>> {
    let NativeEscrowAccountObservation::Found(facts) = observation else {
        return Ok(None);
    };
    let state = facts.metadata.status();
    validate_refund_account_facts(agreement, terms, facts, state)?;
    Ok(Some(state))
}

fn validate_refund_account_facts<E: std::error::Error + 'static>(
    agreement: &ZecAgreementV1,
    terms: &NativeEscrowTerms,
    facts: &NativeEscrowAccountFacts,
    state: EscrowState,
) -> Result<(), NativeRefundAdapterError<E>> {
    let metadata_account = Hex32::from_bytes(*agreement.lez_terms().metadata_account());
    let custody_account = Hex32::from_bytes(*agreement.lez_terms().custody_account());
    let escrow_program =
        Hex32::from_bytes(program_id_bytes(agreement.lez_terms().escrow_program_id()));
    let expected_metadata = native_metadata_facts_for_agreement(
        agreement,
        metadata_account,
        escrow_program,
        custody_account,
        terms,
        state,
    );
    let expected_balance = if state == EscrowState::Funded {
        terms.amount().as_u128()
    } else {
        0
    };
    if facts.metadata.hashlock() != Some(&expected_metadata)
        || facts.custody.account_id != custody_account
        || facts.custody.owner_program_id != terms.authenticated_transfer_program_id()
        || facts.custody.balance.as_u128() != expected_balance
    {
        return Err(NativeRefundAdapterError::InconsistentFacts);
    }
    Ok(())
}

fn prepared_refund_transaction<E: std::error::Error + 'static>(
    prepared: &PreparedRefundSubmissionV1,
) -> Result<PreparedTransaction, NativeRefundAdapterError<E>> {
    if prepared.step() != RefundStepV1::Lez {
        return Err(NativeRefundAdapterError::WrongPreparedStep);
    }
    let exact_bytes = ExactTransactionBytes::new(prepared.exact_submission().to_vec())
        .map_err(NativeRefundAdapterError::Protocol)?;
    Ok(PreparedTransaction::new(
        TransactionId::from_bytes(*prepared.expected_submission_id()),
        exact_bytes,
    ))
}

fn refund_window_is_fully_covered(target: NativeRefundObservationTarget, tip_height: u64) -> bool {
    refund_target_window(target).is_some_and(|window| {
        window
            .start_height()
            .checked_add(u64::from(window.max_blocks() - 1))
            .is_some_and(|final_height| final_height <= tip_height)
    })
}

fn refund_target_window(target: NativeRefundObservationTarget) -> Option<DiscoveryWindow> {
    match target {
        NativeRefundObservationTarget::StateOnly => None,
        NativeRefundObservationTarget::Exact { window, .. }
        | NativeRefundObservationTarget::DiscoverByTerms { window } => Some(window),
    }
}

fn validate_refund_found<E: std::error::Error + 'static>(
    agreement: &ZecAgreementV1,
    terms: &NativeEscrowTerms,
    target: NativeRefundObservationTarget,
    response: &ObserveNativeRefundResult,
    found: &NativeRefundFoundFacts,
    prepared: Option<&PreparedRefundSubmissionV1>,
) -> Result<RefundEvidenceV1, NativeRefundAdapterError<E>> {
    let Some(window) = refund_target_window(target) else {
        return Err(NativeRefundAdapterError::InconsistentFacts);
    };
    let final_height = window
        .start_height()
        .checked_add(u64::from(window.max_blocks() - 1))
        .expect("validated discovery window cannot overflow");
    let transaction = &found.transaction;
    if !(window.start_height()..=final_height).contains(&transaction.position.height) {
        return Err(NativeRefundAdapterError::InconsistentFacts);
    }
    if let NativeRefundObservationTarget::Exact {
        refund_transaction_id,
        ..
    } = target
        && transaction.transaction_id != refund_transaction_id
    {
        return Err(NativeRefundAdapterError::InconsistentFacts);
    }
    if let Some(prepared) = prepared
        && (transaction.transaction_id.as_bytes() != prepared.expected_submission_id()
            || transaction.exact_bytes.as_slice() != prepared.exact_submission())
    {
        return Err(NativeRefundAdapterError::InconsistentFacts);
    }

    let metadata_account = Hex32::from_bytes(*agreement.lez_terms().metadata_account());
    let custody_account = Hex32::from_bytes(*agreement.lez_terms().custody_account());
    let depositor = Hex32::from_bytes(*agreement.lez_account(agreement.lez_depositor()));
    let expected_accounts = AccountIds::new(vec![metadata_account, custody_account, depositor])
        .expect("three refund accounts are bounded");
    let escrow_program =
        Hex32::from_bytes(program_id_bytes(agreement.lez_terms().escrow_program_id()));
    let same_height_wrong_hash = transaction.position.height == response.clock_after.height
        && transaction.position.block_hash != response.clock_after.block_hash;
    if !transaction.is_public
        || !transaction.signer_account_ids.as_slice().is_empty()
        || found.instruction.program_id != escrow_program
        || found.instruction.ordered_account_ids != expected_accounts
        || found.instruction.swap_id != terms.swap_id()
        || transaction.position.height > response.clock_after.height
        || same_height_wrong_hash
    {
        return Err(NativeRefundAdapterError::InconsistentFacts);
    }
    if response.clock_after.timestamp_ms < agreement.lez_refund_at_ms() {
        return Err(NativeRefundAdapterError::RefundBeforeDeadline);
    }
    let confirmations = response
        .clock_after
        .height
        .checked_sub(transaction.position.height)
        .and_then(|distance| distance.checked_add(1))
        .and_then(|depth| u32::try_from(depth).ok())
        .ok_or(NativeRefundAdapterError::InconsistentFacts)?;
    RefundEvidenceV1::new(
        agreement,
        RefundStepV1::Lez,
        *transaction.transaction_id.as_bytes(),
        encode_hex32(transaction.transaction_id.as_bytes()),
        ChainPosition::lez_timestamp_from_milliseconds_floor(LezUnixMilliseconds::new(
            response.clock_after.timestamp_ms,
        )),
        confirmations,
    )
    .map_err(NativeRefundAdapterError::Refund)
}

fn encode_hex32(bytes: &[u8; 32]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(64);
    for byte in bytes {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    encoded
}

fn validate_found_pair<E: std::error::Error + 'static>(
    agreement: &ZecAgreementV1,
    terms: &NativeEscrowTerms,
    target: &EscrowObservationTarget,
    response: &ObserveEscrowResult,
    initialization: &InitializationFoundFacts,
    funding: &FundingFoundFacts,
) -> Result<(), ObserveNativeEscrowError<E>> {
    let init = &initialization.transaction;
    let fund = &funding.transaction;
    if let EscrowObservationTarget::Exact {
        initialization_transaction_id,
        funding_transaction_id,
    } = target
        && (init.transaction_id != *initialization_transaction_id
            || fund.transaction_id != *funding_transaction_id)
    {
        return Err(ObserveNativeEscrowError::InconsistentFacts);
    }
    if let EscrowObservationTarget::DiscoverByTerms { window } = target {
        let final_height = window
            .start_height()
            .checked_add(u64::from(window.max_blocks() - 1))
            .expect("validated discovery window cannot overflow");
        if !(window.start_height()..=final_height).contains(&init.position.height)
            || !(window.start_height()..=final_height).contains(&fund.position.height)
        {
            return Err(ObserveNativeEscrowError::InconsistentFacts);
        }
    }
    let depositor = Hex32::from_bytes(*agreement.lez_account(agreement.lez_depositor()));
    let expected_signers = AccountIds::new(vec![depositor]).expect("one signer is bounded");
    let expected_init_accounts = AccountIds::new(vec![
        Hex32::from_bytes(*agreement.lez_terms().metadata_account()),
        Hex32::from_bytes(*agreement.lez_terms().custody_account()),
        depositor,
        Hex32::from_bytes(*agreement.lez_account(agreement.lez_claimant())),
    ])
    .expect("four accounts are bounded");
    let expected_fund_accounts = AccountIds::new(vec![
        Hex32::from_bytes(*agreement.lez_terms().metadata_account()),
        Hex32::from_bytes(*agreement.lez_terms().custody_account()),
        depositor,
    ])
    .expect("three accounts are bounded");
    let escrow_program =
        Hex32::from_bytes(program_id_bytes(agreement.lez_terms().escrow_program_id()));
    let expected_metadata = native_metadata_facts_for_agreement(
        agreement,
        Hex32::from_bytes(*agreement.lez_terms().metadata_account()),
        escrow_program,
        Hex32::from_bytes(*agreement.lez_terms().custody_account()),
        terms,
        EscrowState::Funded,
    );
    let init_position = (init.position.height, init.position.transaction_index);
    let fund_position = (fund.position.height, fund.position.transaction_index);
    let same_height_different_blocks = init.position.height == fund.position.height
        && init.position.block_hash != fund.position.block_hash;
    if init.transaction_id == fund.transaction_id
        || init.exact_bytes == fund.exact_bytes
        || !init.is_public
        || !fund.is_public
        || init.signer_account_ids != expected_signers
        || fund.signer_account_ids != expected_signers
        || initialization.instruction.program_id != escrow_program
        || initialization.instruction.ordered_account_ids != expected_init_accounts
        || initialization.instruction.terms != *terms
        || funding.instruction.program_id != escrow_program
        || funding.instruction.ordered_account_ids != expected_fund_accounts
        || funding.instruction.swap_id != terms.swap_id()
        || initialization.metadata != expected_metadata
        || funding.metadata != expected_metadata
        || initialization.metadata != funding.metadata
        || funding.custody.account_id != Hex32::from_bytes(*agreement.lez_terms().custody_account())
        || funding.custody.owner_program_id != terms.authenticated_transfer_program_id()
        || funding.custody.balance.as_u128() != terms.amount().as_u128()
        || init_position >= fund_position
        || fund.position.height > response.tip_after.height
        || same_height_different_blocks
    {
        return Err(ObserveNativeEscrowError::InconsistentFacts);
    }
    Ok(())
}

fn discovery_window_is_fully_covered(target: &EscrowObservationTarget, tip_height: u64) -> bool {
    match target {
        EscrowObservationTarget::Exact { .. } => true,
        EscrowObservationTarget::DiscoverByTerms { window } => window
            .start_height()
            .checked_add(u64::from(window.max_blocks() - 1))
            .is_some_and(|final_height| final_height <= tip_height),
    }
}

fn canonical_snapshot(
    agreement: &ZecAgreementV1,
    response: &ObserveEscrowResult,
    funding: &FundingFoundFacts,
) -> LezNodeSnapshotV1 {
    let transaction = &funding.transaction;
    let metadata = &funding.metadata;
    LezNodeSnapshotV1::new(
        agreement.lez_terms().chain().environment(),
        *agreement.lez_terms().chain().channel_id(),
        *agreement.lez_terms().chain().genesis_block_hash(),
        LezStableTipV1::new(
            *response.tip_before.block_hash.as_bytes(),
            response.tip_before.height,
            *response.tip_after.block_hash.as_bytes(),
            response.tip_after.height,
        ),
        LezFundTransactionSnapshotV1::new(
            *transaction.transaction_id.as_bytes(),
            *agreement.lez_terms().escrow_program_id(),
            *agreement.lez_account(agreement.lez_depositor()),
            funding
                .instruction
                .ordered_account_ids
                .as_slice()
                .iter()
                .map(|account| *account.as_bytes())
                .collect(),
            LezFundInstructionV1::Native {
                swap_id: *funding.instruction.swap_id.as_bytes(),
            },
            transaction.is_public,
            transaction.signer_account_ids.as_slice()
                == [Hex32::from_bytes(
                    *agreement.lez_account(agreement.lez_depositor()),
                )],
            transaction.position.height,
            *transaction.position.block_hash.as_bytes(),
            *transaction.position.block_hash.as_bytes(),
            // The compatibility bridge has no settlement-finality primitive. Pending is
            // the conservative projection; deterministic compatibility policy uses depth.
            LezInclusionStatusV1::Pending,
        ),
        words_from_bytes(metadata.owner_program_id.as_bytes()),
        *metadata.account_id.as_bytes(),
        LezEscrowMetadataSnapshotV1::new(
            metadata.version,
            *metadata.swap_id.as_bytes(),
            *metadata.terms_hash.as_bytes(),
            *metadata.secret_digest.as_bytes(),
            *metadata.depositor_account_id.as_bytes(),
            *metadata.depositor_asset_account_id.as_bytes(),
            *metadata.claimant_account_id.as_bytes(),
            *metadata.claimant_asset_account_id.as_bytes(),
            *metadata.custody_account_id.as_bytes(),
            words_from_bytes(metadata.asset_program_id.as_bytes()),
            words_from_bytes(metadata.custody_program_id.as_bytes()),
            *metadata.asset_definition.as_bytes(),
            metadata.amount.as_u128(),
            metadata.refund_at_ms,
            match metadata.status {
                EscrowState::Empty => LezEscrowStatusV1::Empty,
                EscrowState::Funded => LezEscrowStatusV1::Funded,
                EscrowState::Claimed => LezEscrowStatusV1::Claimed,
                EscrowState::Refunded => LezEscrowStatusV1::Refunded,
            },
        ),
        *funding.custody.account_id.as_bytes(),
        LezCustodySnapshotV1::Native {
            program_owner: words_from_bytes(funding.custody.owner_program_id.as_bytes()),
            balance: funding.custody.balance.as_u128(),
        },
    )
}

const fn bridge_participant(participant: Participant) -> BridgeParticipant {
    match participant {
        Participant::Maker => BridgeParticipant::Maker,
        Participant::Taker => BridgeParticipant::Taker,
    }
}

fn program_id_bytes(program_id: &[u32; 8]) -> [u8; 32] {
    let mut bytes = [0_u8; 32];
    for (chunk, word) in bytes.chunks_exact_mut(4).zip(program_id) {
        chunk.copy_from_slice(&word.to_le_bytes());
    }
    bytes
}

fn words_from_bytes(bytes: &[u8; 32]) -> [u32; 8] {
    let mut words = [0_u32; 8];
    for (word, chunk) in words.iter_mut().zip(bytes.chunks_exact(4)) {
        *word = u32::from_le_bytes(chunk.try_into().expect("four-byte chunk"));
    }
    words
}

const fn runtime_generation_is_compatible(
    environment: LezEnvironmentV1,
    compatibility: RuntimeCompatibility,
) -> bool {
    matches!(
        (environment, compatibility),
        (
            LezEnvironmentV1::DeterministicLocalV0_1_2Compatibility,
            RuntimeCompatibility::NssaV0_1_2,
        ) | (
            LezEnvironmentV1::DeterministicLocalV0_2 | LezEnvironmentV1::PublicTestnetV0_2,
            RuntimeCompatibility::LeeV0_2_0,
        )
    )
}

#[cfg(test)]
mod runtime_generation_tests {
    use super::*;

    #[test]
    fn accepts_exact_local_and_public_v02_generations_and_rejects_cross_pairs() {
        for (environment, compatibility, expected) in [
            (
                LezEnvironmentV1::DeterministicLocalV0_1_2Compatibility,
                RuntimeCompatibility::NssaV0_1_2,
                true,
            ),
            (
                LezEnvironmentV1::DeterministicLocalV0_2,
                RuntimeCompatibility::LeeV0_2_0,
                true,
            ),
            (
                LezEnvironmentV1::DeterministicLocalV0_1_2Compatibility,
                RuntimeCompatibility::LeeV0_2_0,
                false,
            ),
            (
                LezEnvironmentV1::DeterministicLocalV0_2,
                RuntimeCompatibility::NssaV0_1_2,
                false,
            ),
            (
                LezEnvironmentV1::PublicTestnetV0_2,
                RuntimeCompatibility::NssaV0_1_2,
                false,
            ),
            (
                LezEnvironmentV1::PublicTestnetV0_2,
                RuntimeCompatibility::LeeV0_2_0,
                true,
            ),
        ] {
            assert_eq!(
                runtime_generation_is_compatible(environment, compatibility),
                expected
            );
        }
    }
}
