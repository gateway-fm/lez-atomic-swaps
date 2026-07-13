//! Main-process composition boundary for a dedicated official LEZ sidecar.

#![forbid(unsafe_code)]

use async_trait::async_trait;
use lez_bridge_client::{BridgeClient, BridgeClientError};
use lez_bridge_protocol::{
    Hex32, MessageContext, NativeEscrowTerms, NativeEscrowTermsInput,
    Participant as BridgeParticipant, PrepareNativeEscrowRequest, PrepareNativeEscrowResult,
    ProtocolValueError, RequestId, RunId, RuntimeDescriptor,
};
use lez_swap_core::Participant;
use lez_zec_swap_sdk::{
    FirstLockIntentError, FirstLockPlanV1, FirstLockStepV1, LezAssetV1, LezEnvironmentV1,
    PreparedFirstLockSubmissionV1, ZecAgreementV1,
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
