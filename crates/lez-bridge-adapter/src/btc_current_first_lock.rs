//! Fresh current-state proof for agreement-selected LEZ/Bitcoin funded escrows.

use async_trait::async_trait;
use lez_bridge_client::{BridgeClient, BridgeClientError};
use lez_bridge_protocol::{
    ChainClock, EscrowState, Hex32, MessageContext, NativeCustodyFacts,
    NativeEscrowAccountObservation, NativeRefundObservation, NativeRefundObservationTarget,
    ObserveNativeRefundRequest, ObserveNativeRefundResult, Participant as BridgeParticipant,
    ProtocolValueError, RequestId, RuntimeCompatibility, WitnessedEscrowMetadataFacts,
    WitnessedNativeEscrowTerms, WitnessedNativeEscrowTermsInput,
};
use lez_btc_swap_sdk::BtcAgreementV1;
use lez_swap_core::{Chain, Participant};
use thiserror::Error;

use crate::LezBridgeAdapter;

const CURRENT_LEZ_FUNDED_ESCROW_EVIDENCE_SCHEMA_V1: u16 = 1;

/// One strictly read-only state-only bridge observation.
///
/// This narrow port deliberately exposes no preparation or submission method.
#[async_trait]
pub trait LezBridgeCurrentEscrowTransport: Send + Sync {
    /// Concrete transport failure.
    type Error: std::error::Error + Send + Sync + 'static;

    /// Reads canonical escrow accounts and their stable chain clock once.
    async fn observe_native_refund(
        &self,
        request: ObserveNativeRefundRequest,
    ) -> Result<ObserveNativeRefundResult, Self::Error>;
}

#[async_trait]
impl LezBridgeCurrentEscrowTransport for BridgeClient {
    type Error = BridgeClientError;

    async fn observe_native_refund(
        &self,
        request: ObserveNativeRefundRequest,
    ) -> Result<ObserveNativeRefundResult, Self::Error> {
        BridgeClient::observe_native_refund(self, request).await
    }
}

/// Exact current funded escrow facts observed under one stable canonical clock.
///
/// The clock is a state-only canonical node clock. This evidence does not claim
/// that the clock or the current account snapshot is finalized.
#[derive(Clone, Debug, Eq, PartialEq)]
#[must_use]
pub struct CurrentLezFundedEscrowEvidenceV1 {
    schema_version: u16,
    clock: ChainClock,
    metadata: WitnessedEscrowMetadataFacts,
    custody: NativeCustodyFacts,
}

impl CurrentLezFundedEscrowEvidenceV1 {
    /// Evidence schema version.
    #[must_use]
    pub const fn schema_version(&self) -> u16 {
        self.schema_version
    }

    /// Stable canonical state-only clock bracketing the account read.
    pub const fn clock(&self) -> ChainClock {
        self.clock
    }

    /// Exact agreement-bound funded metadata.
    pub const fn metadata(&self) -> &WitnessedEscrowMetadataFacts {
        &self.metadata
    }

    /// Exact agreement-bound custody account holding the complete amount.
    pub const fn custody(&self) -> &NativeCustodyFacts {
        &self.custody
    }
}

/// Backwards-compatible name for current LEZ taker-first-lock evidence.
pub type CurrentLezFirstLockEvidenceV1 = CurrentLezFundedEscrowEvidenceV1;

/// Failure to prove that the agreement-selected LEZ escrow is currently funded.
#[derive(Debug, Error)]
pub enum CurrentLezFundedEscrowError<E: std::error::Error + 'static> {
    /// The selected sidecar is not the pinned LEZ v0.2 compatibility runtime.
    #[error("LEZ current funded-escrow runtime is incompatible")]
    IncompatibleRuntime,
    /// Runtime channel or genesis differs from the signed agreement.
    #[error("LEZ current funded-escrow chain identity differs from agreement")]
    ChainIdentityMismatch,
    /// Runtime escrow deployment differs from the signed agreement.
    #[error("LEZ current funded-escrow program differs from agreement")]
    EscrowProgramMismatch,
    /// Runtime signer is not the agreement-bound local role account.
    #[error("LEZ current funded-escrow sidecar signer differs from local role")]
    SignerAccountMismatch,
    /// Agreement fields could not form exact witnessed bridge terms.
    #[error("LEZ current funded-escrow terms are invalid")]
    Protocol(#[source] ProtocolValueError),
    /// The single read-only bridge call had an unknown outcome.
    #[error("LEZ current funded-escrow observation is unavailable")]
    Transport(#[source] E),
    /// The sidecar did not echo the exact caller-owned context.
    #[error("LEZ current funded-escrow response context mismatch")]
    ResponseContextMismatch,
    /// The canonical clock changed while accounts were read.
    #[error("LEZ current funded-escrow clock was not stable")]
    UnstableClock,
    /// A state-only request unexpectedly performed a refund lookup.
    #[error("LEZ current funded-escrow state-only observation included refund facts")]
    UnexpectedRefundObservation,
    /// The exact metadata/custody pair was absent.
    #[error("LEZ current funded-escrow accounts are unavailable")]
    AccountsUnavailable,
    /// The escrow no longer has the exact Funded state.
    #[error("LEZ current funded escrow is not funded")]
    EscrowNotFunded,
    /// Metadata, custody, owner, or signed account identity was substituted.
    #[error("LEZ current funded-escrow account facts differ from agreement")]
    AccountMismatch,
    /// Current custody does not hold the complete signed amount.
    #[error("LEZ current funded-escrow custody value differs from agreement")]
    CustodyValueMismatch,
}

/// Failure to prove that the LEZ taker first lock remains currently funded.
#[derive(Debug, Error)]
pub enum CurrentLezFirstLockError<E: std::error::Error + 'static> {
    /// This direction has a Bitcoin, rather than LEZ, taker first lock.
    #[error("agreement does not select a LEZ first lock")]
    WrongDirection,
    /// The selected sidecar is not the pinned LEZ v0.2 compatibility runtime.
    #[error("LEZ current first-lock runtime is incompatible")]
    IncompatibleRuntime,
    /// Runtime channel or genesis differs from the signed agreement.
    #[error("LEZ current first-lock chain identity differs from agreement")]
    ChainIdentityMismatch,
    /// Runtime escrow deployment differs from the signed agreement.
    #[error("LEZ current first-lock escrow program differs from agreement")]
    EscrowProgramMismatch,
    /// Runtime signer is not the agreement-bound local role account.
    #[error("LEZ current first-lock sidecar signer differs from local role")]
    SignerAccountMismatch,
    /// Agreement fields could not form exact witnessed bridge terms.
    #[error("LEZ current first-lock terms are invalid")]
    Protocol(#[source] ProtocolValueError),
    /// The single read-only bridge call had an unknown outcome.
    #[error("LEZ current first-lock observation is unavailable")]
    Transport(#[source] E),
    /// The sidecar did not echo the exact caller-owned context.
    #[error("LEZ current first-lock response context mismatch")]
    ResponseContextMismatch,
    /// The canonical clock changed while accounts were read.
    #[error("LEZ current first-lock clock was not stable")]
    UnstableClock,
    /// A state-only request unexpectedly performed a refund lookup.
    #[error("LEZ current first-lock state-only observation included refund facts")]
    UnexpectedRefundObservation,
    /// The exact metadata/custody pair was absent.
    #[error("LEZ current first-lock accounts are unavailable")]
    AccountsUnavailable,
    /// The escrow no longer has the exact Funded state.
    #[error("LEZ current first-lock escrow is not funded")]
    EscrowNotFunded,
    /// Metadata, custody, owner, or signed account identity was substituted.
    #[error("LEZ current first-lock account facts differ from agreement")]
    AccountMismatch,
    /// Current custody does not hold the complete signed amount.
    #[error("LEZ current first-lock custody value differs from agreement")]
    CustodyValueMismatch,
}

impl<E> From<CurrentLezFundedEscrowError<E>> for CurrentLezFirstLockError<E>
where
    E: std::error::Error + 'static,
{
    fn from(error: CurrentLezFundedEscrowError<E>) -> Self {
        match error {
            CurrentLezFundedEscrowError::IncompatibleRuntime => Self::IncompatibleRuntime,
            CurrentLezFundedEscrowError::ChainIdentityMismatch => Self::ChainIdentityMismatch,
            CurrentLezFundedEscrowError::EscrowProgramMismatch => Self::EscrowProgramMismatch,
            CurrentLezFundedEscrowError::SignerAccountMismatch => Self::SignerAccountMismatch,
            CurrentLezFundedEscrowError::Protocol(source) => Self::Protocol(source),
            CurrentLezFundedEscrowError::Transport(source) => Self::Transport(source),
            CurrentLezFundedEscrowError::ResponseContextMismatch => Self::ResponseContextMismatch,
            CurrentLezFundedEscrowError::UnstableClock => Self::UnstableClock,
            CurrentLezFundedEscrowError::UnexpectedRefundObservation => {
                Self::UnexpectedRefundObservation
            }
            CurrentLezFundedEscrowError::AccountsUnavailable => Self::AccountsUnavailable,
            CurrentLezFundedEscrowError::EscrowNotFunded => Self::EscrowNotFunded,
            CurrentLezFundedEscrowError::AccountMismatch => Self::AccountMismatch,
            CurrentLezFundedEscrowError::CustodyValueMismatch => Self::CustodyValueMismatch,
        }
    }
}

impl<T> LezBridgeAdapter<T>
where
    T: LezBridgeCurrentEscrowTransport,
{
    /// Proves that the agreement-selected LEZ escrow remains currently funded.
    ///
    /// Both agreement roles may perform this public state read through their
    /// own role-bound sidecar. The request is always `StateOnly`; the transport
    /// surface contains no mutation. One unchanged current canonical clock
    /// brackets the exact witnessed metadata and complete custody amount.
    ///
    /// # Errors
    ///
    /// Fails closed on runtime, role, context, clock, account, state, owner, or
    /// amount drift.
    pub async fn observe_current_lez_funded_escrow(
        &self,
        agreement: &BtcAgreementV1,
        request_id: RequestId,
    ) -> Result<CurrentLezFundedEscrowEvidenceV1, CurrentLezFundedEscrowError<T::Error>> {
        validate_runtime(self, agreement)?;
        let terms = witnessed_terms(agreement).map_err(CurrentLezFundedEscrowError::Protocol)?;
        let context = MessageContext::new(
            self.run_id.clone(),
            request_id,
            bridge_participant(self.local_participant),
        );
        let response = self
            .transport
            .observe_native_refund(ObserveNativeRefundRequest::new_witnessed(
                context.clone(),
                self.runtime.clone(),
                terms.clone(),
                NativeRefundObservationTarget::StateOnly,
            ))
            .await
            .map_err(CurrentLezFundedEscrowError::Transport)?;
        if response.context != context {
            return Err(CurrentLezFundedEscrowError::ResponseContextMismatch);
        }
        if response.clock_before != response.clock_after {
            return Err(CurrentLezFundedEscrowError::UnstableClock);
        }
        if response.refund != NativeRefundObservation::NotRequested {
            return Err(CurrentLezFundedEscrowError::UnexpectedRefundObservation);
        }
        let NativeEscrowAccountObservation::Found(accounts) = response.accounts else {
            return Err(CurrentLezFundedEscrowError::AccountsUnavailable);
        };
        let Some(metadata) = accounts.metadata.witnessed() else {
            return Err(CurrentLezFundedEscrowError::AccountMismatch);
        };
        if metadata.status != EscrowState::Funded {
            return Err(CurrentLezFundedEscrowError::EscrowNotFunded);
        }
        let signed = agreement.lez_terms();
        let expected_metadata = WitnessedEscrowMetadataFacts::from_witnessed_native_terms(
            Hex32::from_bytes(*signed.metadata_account()),
            self.runtime.escrow_program_id,
            Hex32::from_bytes(*signed.custody_account()),
            &terms,
            EscrowState::Funded,
        );
        if metadata != &expected_metadata
            || accounts.custody.account_id.as_bytes() != signed.custody_account()
            || accounts.custody.owner_program_id.as_bytes()
                != signed.authenticated_transfer_program_id()
        {
            return Err(CurrentLezFundedEscrowError::AccountMismatch);
        }
        if accounts.custody.balance.as_u128() != signed.amount() {
            return Err(CurrentLezFundedEscrowError::CustodyValueMismatch);
        }
        Ok(CurrentLezFundedEscrowEvidenceV1 {
            schema_version: CURRENT_LEZ_FUNDED_ESCROW_EVIDENCE_SCHEMA_V1,
            clock: response.clock_after,
            metadata: metadata.clone(),
            custody: accounts.custody.clone(),
        })
    }

    /// Proves that the exact LEZ taker first lock remains currently funded.
    ///
    /// Both agreement roles may perform this public state read through their
    /// own role-bound sidecar. The request is always `StateOnly`; the transport
    /// surface contains no mutation. Stable current state complements, but does
    /// not replace, separately retained finalized funding evidence.
    ///
    /// # Errors
    ///
    /// Rejects the Bitcoin-first direction before transport and fails closed on
    /// runtime, role, context, clock, account, state, owner, or amount drift.
    pub async fn observe_current_lez_first_lock(
        &self,
        agreement: &BtcAgreementV1,
        request_id: RequestId,
    ) -> Result<CurrentLezFirstLockEvidenceV1, CurrentLezFirstLockError<T::Error>> {
        if agreement.coordinator().funded_chain(Participant::Taker) != Chain::Lez {
            return Err(CurrentLezFirstLockError::WrongDirection);
        }
        self.observe_current_lez_funded_escrow(agreement, request_id)
            .await
            .map_err(CurrentLezFirstLockError::from)
    }
}

fn validate_runtime<T>(
    adapter: &LezBridgeAdapter<T>,
    agreement: &BtcAgreementV1,
) -> Result<(), CurrentLezFundedEscrowError<T::Error>>
where
    T: LezBridgeCurrentEscrowTransport,
{
    let signed = agreement.lez_terms();
    if adapter.runtime.compatibility != RuntimeCompatibility::LeeV0_2_0 {
        return Err(CurrentLezFundedEscrowError::IncompatibleRuntime);
    }
    if adapter.runtime.channel_id.as_bytes() != signed.channel_id()
        || adapter.runtime.genesis_block_hash.as_bytes() != signed.genesis_block_hash()
    {
        return Err(CurrentLezFundedEscrowError::ChainIdentityMismatch);
    }
    if adapter.runtime.escrow_program_id.as_bytes() != signed.escrow_program_id() {
        return Err(CurrentLezFundedEscrowError::EscrowProgramMismatch);
    }
    if adapter.runtime.signer_account_id.as_bytes()
        != agreement
            .participant(adapter.local_participant)
            .lez_owner_account()
    {
        return Err(CurrentLezFundedEscrowError::SignerAccountMismatch);
    }
    Ok(())
}

fn witnessed_terms(
    agreement: &BtcAgreementV1,
) -> Result<WitnessedNativeEscrowTerms, ProtocolValueError> {
    let signed = agreement.lez_terms();
    WitnessedNativeEscrowTerms::new(WitnessedNativeEscrowTermsInput {
        swap_id: Hex32::from_bytes(*agreement.body().swap_id()),
        terms_hash: Hex32::from_bytes(*agreement.agreement_commitment()),
        depositor: bridge_participant(agreement.lez_depositor()),
        depositor_account_id: Hex32::from_bytes(*signed.depositor_account()),
        claimant: bridge_participant(agreement.lez_claimant()),
        claimant_account_id: Hex32::from_bytes(*signed.claimant_account()),
        aggregate_authority_account_id: Hex32::from_bytes(*signed.aggregate_authority_account()),
        aggregate_x_only_public_key: Hex32::from_bytes(
            agreement.p2tr_contract().aggregate_internal_key_bytes(),
        ),
        amount: signed.amount(),
        refund_at_ms: signed.refund_at_ms(),
        authenticated_transfer_program_id: Hex32::from_bytes(
            *signed.authenticated_transfer_program_id(),
        ),
    })
}

const fn bridge_participant(participant: Participant) -> BridgeParticipant {
    match participant {
        Participant::Maker => BridgeParticipant::Maker,
        Participant::Taker => BridgeParticipant::Taker,
    }
}
