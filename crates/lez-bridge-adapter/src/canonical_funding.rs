//! Canonical LEZ funding reconstructed from role-local `SQLite` recovery state.

use std::fmt;

use async_trait::async_trait;
use lez_bridge_protocol::Hex32;
use lez_swap_core::{Participant, SwapDirection};
use lez_swap_store::SqliteZecRecoveryStore;
use lez_zec_swap_sdk::{
    AcceptedZecAgreementV1, FirstLockConfirmedEvidenceV1, FirstLockStepV1, RecoveryStore,
    ZecAgreementV1,
};

use crate::CanonicalLezFundingSource;

const TAKER_FIRST_LOCK_PREDECESSOR: u64 = 0;
const MAKER_SECOND_LOCK_PREDECESSOR: u64 = 1;

/// Store-derived canonical LEZ funding authority for one local claimant.
///
/// The source accepts no caller-supplied transaction primitive. It reopens only
/// records authenticated by [`SqliteZecRecoveryStore`], binds the exact supplied
/// signed agreement, and derives the LEZ funding identity from the direction-fixed
/// durable transition slot.
#[derive(Clone)]
pub struct SqliteCanonicalLezFundingSource {
    store: SqliteZecRecoveryStore,
    local_claimant: Participant,
}

impl SqliteCanonicalLezFundingSource {
    /// Binds a reopened role-local recovery store to its expected LEZ claimant.
    #[must_use]
    pub const fn new(store: SqliteZecRecoveryStore, local_claimant: Participant) -> Self {
        Self {
            store,
            local_claimant,
        }
    }

    async fn validate_durable_agreement(
        &self,
        supplied: &ZecAgreementV1,
    ) -> Result<(), SqliteCanonicalLezFundingSourceError> {
        if self.store.local_participant() != self.local_claimant
            || supplied.lez_claimant() != self.local_claimant
        {
            return Err(SqliteCanonicalLezFundingSourceError::WrongClaimant);
        }
        let envelope = self
            .store
            .load_agreement(supplied.coordinator().id())
            .await
            .map_err(|_| SqliteCanonicalLezFundingSourceError::RecoveryUnavailable)?
            .ok_or(SqliteCanonicalLezFundingSourceError::AgreementUnavailable)?;
        let accepted = AcceptedZecAgreementV1::resume(&envelope)
            .map_err(|_| SqliteCanonicalLezFundingSourceError::InvalidRecoveryState)?;
        if accepted.local_participant() != self.local_claimant
            || accepted.revision() != 0
            || accepted.agreement() != supplied
            || accepted.agreement().agreement_commitment() != supplied.agreement_commitment()
            || accepted.agreement().coordinator().id() != supplied.coordinator().id()
        {
            return Err(SqliteCanonicalLezFundingSourceError::AgreementMismatch);
        }
        Ok(())
    }

    async fn taker_claimant_funding(
        &self,
        agreement: &ZecAgreementV1,
    ) -> Result<FirstLockConfirmedEvidenceV1, SqliteCanonicalLezFundingSourceError> {
        let transition = self
            .store
            .load_observed_maker_lock_transition(
                agreement.coordinator().id(),
                MAKER_SECOND_LOCK_PREDECESSOR,
            )
            .await
            .map_err(|_| SqliteCanonicalLezFundingSourceError::RecoveryUnavailable)?
            .ok_or(SqliteCanonicalLezFundingSourceError::FundingTransitionUnavailable)?;
        if transition.schema_version() != 1
            || transition.swap_id() != agreement.coordinator().id()
            || transition.agreement_commitment() != agreement.agreement_commitment()
            || transition.local_participant() != Participant::Taker
            || transition.predecessor_revision() != MAKER_SECOND_LOCK_PREDECESSOR
        {
            return Err(SqliteCanonicalLezFundingSourceError::InvalidRecoveryState);
        }
        validate_lez_evidence(transition.evidence(), agreement, Participant::Maker)
    }

    async fn maker_claimant_funding(
        &self,
        agreement: &ZecAgreementV1,
    ) -> Result<FirstLockConfirmedEvidenceV1, SqliteCanonicalLezFundingSourceError> {
        let taker_lez = self
            .store
            .load_observed_taker_first_lock_transition(
                agreement.coordinator().id(),
                TAKER_FIRST_LOCK_PREDECESSOR,
            )
            .await
            .map_err(|_| SqliteCanonicalLezFundingSourceError::RecoveryUnavailable)?
            .ok_or(SqliteCanonicalLezFundingSourceError::FundingTransitionUnavailable)?;
        if taker_lez.schema_version() != 1
            || taker_lez.swap_id() != agreement.coordinator().id()
            || taker_lez.agreement_commitment() != agreement.agreement_commitment()
            || taker_lez.local_participant() != Participant::Maker
            || taker_lez.predecessor_revision() != TAKER_FIRST_LOCK_PREDECESSOR
            || taker_lez.evidence().step() != FirstLockStepV1::LezFund
            || taker_lez.evidence().confirmations()
                < agreement
                    .coordinator()
                    .required_confirmations(Participant::Taker)
        {
            return Err(SqliteCanonicalLezFundingSourceError::InvalidRecoveryState);
        }

        // A revealing claim must not be prepared from the first LEZ leg alone.
        // The maker-local Zcash second-lock transition proves the opposite asset
        // was durably locked at the exact next aggregate revision.
        let maker_zcash = self
            .store
            .load_maker_lock_transition(agreement.coordinator().id(), MAKER_SECOND_LOCK_PREDECESSOR)
            .await
            .map_err(|_| SqliteCanonicalLezFundingSourceError::RecoveryUnavailable)?
            .ok_or(SqliteCanonicalLezFundingSourceError::SecondLockUnavailable)?;
        if maker_zcash.schema_version() != 1
            || maker_zcash.swap_id() != agreement.coordinator().id()
            || maker_zcash.predecessor_revision() != MAKER_SECOND_LOCK_PREDECESSOR
            || maker_zcash.evidence().schema_version() != 1
            || maker_zcash.evidence().step() != FirstLockStepV1::ZcashFund
            || maker_zcash.evidence().confirmations()
                < agreement
                    .coordinator()
                    .required_confirmations(Participant::Maker)
        {
            return Err(SqliteCanonicalLezFundingSourceError::InvalidRecoveryState);
        }

        let transaction_id = taker_lez.evidence().transaction_id();
        let parsed = Hex32::from_hex(transaction_id)
            .map_err(|_| SqliteCanonicalLezFundingSourceError::InvalidFundingIdentity)?;
        FirstLockConfirmedEvidenceV1::new(
            FirstLockStepV1::LezFund,
            *parsed.as_bytes(),
            transaction_id.to_owned(),
            taker_lez.evidence().confirmations(),
        )
        .map_err(|_| SqliteCanonicalLezFundingSourceError::InvalidRecoveryState)
    }
}

impl fmt::Debug for SqliteCanonicalLezFundingSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SqliteCanonicalLezFundingSource")
            .field("store", &"[REDACTED]")
            .field("local_claimant", &self.local_claimant)
            .finish()
    }
}

/// Redacted failure category for canonical LEZ funding reconstruction.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum SqliteCanonicalLezFundingSourceError {
    /// The configured store role or supplied agreement does not name the local claimant.
    #[error("role-local LEZ funding source is not owned by the signed claimant")]
    WrongClaimant,
    /// No durable agreement exists under the signed swap identity.
    #[error("role-local signed agreement is unavailable")]
    AgreementUnavailable,
    /// The durable agreement differs from the exact supplied signed agreement.
    #[error("role-local signed agreement does not match the supplied agreement")]
    AgreementMismatch,
    /// The direction-fixed durable LEZ funding transition is absent.
    #[error("canonical LEZ funding transition is unavailable")]
    FundingTransitionUnavailable,
    /// Reverse direction has no durable maker Zcash second-lock transition yet.
    #[error("opposite-chain maker lock transition is unavailable")]
    SecondLockUnavailable,
    /// The durable LEZ transaction identity is not exact lowercase hexadecimal.
    #[error("canonical LEZ funding identity is invalid")]
    InvalidFundingIdentity,
    /// The `SQLite` recovery adapter could not safely load the requested state.
    #[error("role-local recovery store is unavailable")]
    RecoveryUnavailable,
    /// Revalidated durable role, revision, kind, or agreement context was inconsistent.
    #[error("role-local recovery state is invalid")]
    InvalidRecoveryState,
}

#[async_trait]
impl CanonicalLezFundingSource for SqliteCanonicalLezFundingSource {
    type Error = SqliteCanonicalLezFundingSourceError;

    async fn canonical_lez_funding(
        &self,
        agreement: &ZecAgreementV1,
    ) -> Result<FirstLockConfirmedEvidenceV1, Self::Error> {
        self.validate_durable_agreement(agreement).await?;
        match agreement.direction() {
            SwapDirection::TakerSellsForeign => self.taker_claimant_funding(agreement).await,
            SwapDirection::TakerSellsLez => self.maker_claimant_funding(agreement).await,
        }
    }
}

fn validate_lez_evidence(
    evidence: &FirstLockConfirmedEvidenceV1,
    agreement: &ZecAgreementV1,
    funded_by: Participant,
) -> Result<FirstLockConfirmedEvidenceV1, SqliteCanonicalLezFundingSourceError> {
    if evidence.schema_version() != 1
        || evidence.step() != FirstLockStepV1::LezFund
        || evidence.confirmations() < agreement.coordinator().required_confirmations(funded_by)
    {
        return Err(SqliteCanonicalLezFundingSourceError::InvalidRecoveryState);
    }
    let parsed = Hex32::from_hex(evidence.transaction_id())
        .map_err(|_| SqliteCanonicalLezFundingSourceError::InvalidFundingIdentity)?;
    if parsed.as_bytes() != evidence.expected_submission_id() {
        return Err(SqliteCanonicalLezFundingSourceError::InvalidFundingIdentity);
    }
    Ok(evidence.clone())
}
