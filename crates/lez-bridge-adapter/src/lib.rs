//! Main-process composition boundary for a dedicated official LEZ sidecar.

#![forbid(unsafe_code)]

use async_trait::async_trait;
use lez_bridge_client::{BridgeClient, BridgeClientError};
use lez_bridge_protocol::{
    AccountIds, EscrowMetadataFacts, EscrowObservationTarget, EscrowState, FundingFoundFacts,
    FundingObservation, Hex32, InitializationFoundFacts, InitializationObservation, MessageContext,
    NativeEscrowTerms, NativeEscrowTermsInput, ObserveEscrowRequest, ObserveEscrowResult,
    Participant as BridgeParticipant, PrepareNativeEscrowRequest, PrepareNativeEscrowResult,
    ProtocolValueError, RequestId, RunId, RuntimeCompatibility, RuntimeDescriptor,
};
use lez_swap_core::Participant;
use lez_zec_swap_sdk::{
    CanonicalLezEscrowObservationV1, FirstLockIntentError, FirstLockPlanV1, FirstLockStepV1,
    LezAssetV1, LezCustodySnapshotV1, LezEnvironmentV1, LezEscrowMetadataSnapshotV1,
    LezEscrowStatusV1, LezFundInstructionV1, LezFundTransactionSnapshotV1, LezInclusionStatusV1,
    LezNodeSnapshotV1, LezObservationError, LezStableTipV1, PreparedFirstLockSubmissionV1,
    TakerFirstLockObservationV1, ZecAgreementV1,
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
    /// This adapter is intentionally pinned to the official v0.1.2 compatibility runtime.
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
    /// This adapter is intentionally pinned to the official v0.1.2 compatibility runtime.
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
        let signed_chain = agreement.lez_terms().chain();
        if signed_chain.environment() != LezEnvironmentV1::DeterministicLocalV0_1_2Compatibility {
            return Err(PrepareNativeFirstLockError::IncompatibleEnvironment);
        }
        if self.runtime.channel_id != Hex32::from_bytes(*signed_chain.channel_id())
            || self.runtime.genesis_block_hash
                != Hex32::from_bytes(*signed_chain.genesis_block_hash())
        {
            return Err(PrepareNativeFirstLockError::ChainIdentityMismatch);
        }
        if self.runtime.escrow_program_id
            != Hex32::from_bytes(program_id_bytes(agreement.lez_terms().escrow_program_id()))
        {
            return Err(PrepareNativeFirstLockError::EscrowProgramMismatch);
        }
        if self.runtime.signer_account_id
            != Hex32::from_bytes(*agreement.lez_account(self.local_participant))
        {
            return Err(PrepareNativeFirstLockError::SignerAccountMismatch);
        }
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
        validate_runtime(agreement, &self.runtime, self.local_participant)
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
                let snapshot = canonical_snapshot(agreement, &response, funding);
                let canonical = CanonicalLezEscrowObservationV1::validate(agreement, &snapshot)
                    .map_err(ObserveNativeEscrowError::Canonical)?;
                Ok(TakerFirstLockObservationV1::CanonicalLez(Box::new(
                    canonical,
                )))
            }
        }
    }
}

#[derive(Debug)]
enum NativeTermsError {
    UnsupportedAsset,
    Protocol(ProtocolValueError),
}

#[derive(Clone, Copy, Debug)]
enum RuntimeBindingError {
    IncompatibleEnvironment,
    ChainIdentityMismatch,
    EscrowProgramMismatch,
    SignerAccountMismatch,
}

fn validate_runtime(
    agreement: &ZecAgreementV1,
    runtime: &RuntimeDescriptor,
    local_participant: Participant,
) -> Result<(), RuntimeBindingError> {
    let signed_chain = agreement.lez_terms().chain();
    if signed_chain.environment() != LezEnvironmentV1::DeterministicLocalV0_1_2Compatibility
        || runtime.compatibility != RuntimeCompatibility::NssaV0_1_2
    {
        return Err(RuntimeBindingError::IncompatibleEnvironment);
    }
    if runtime.channel_id != Hex32::from_bytes(*signed_chain.channel_id())
        || runtime.genesis_block_hash != Hex32::from_bytes(*signed_chain.genesis_block_hash())
    {
        return Err(RuntimeBindingError::ChainIdentityMismatch);
    }
    if runtime.escrow_program_id
        != Hex32::from_bytes(program_id_bytes(agreement.lez_terms().escrow_program_id()))
    {
        return Err(RuntimeBindingError::EscrowProgramMismatch);
    }
    if runtime.signer_account_id != Hex32::from_bytes(*agreement.lez_account(local_participant)) {
        return Err(RuntimeBindingError::SignerAccountMismatch);
    }
    Ok(())
}

fn map_runtime_observation_error<E: std::error::Error + 'static>(
    error: RuntimeBindingError,
) -> ObserveNativeEscrowError<E> {
    match error {
        RuntimeBindingError::IncompatibleEnvironment => {
            ObserveNativeEscrowError::IncompatibleEnvironment
        }
        RuntimeBindingError::ChainIdentityMismatch => {
            ObserveNativeEscrowError::ChainIdentityMismatch
        }
        RuntimeBindingError::EscrowProgramMismatch => {
            ObserveNativeEscrowError::EscrowProgramMismatch
        }
        RuntimeBindingError::SignerAccountMismatch => {
            ObserveNativeEscrowError::SignerAccountMismatch
        }
    }
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
    let expected_metadata = EscrowMetadataFacts::from_native_terms(
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
            // v0.1.2 has no Bedrock-finality primitive. Pending is the only
            // conservative projection; deterministic compatibility policy uses depth.
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
