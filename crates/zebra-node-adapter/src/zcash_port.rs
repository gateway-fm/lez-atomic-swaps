//! One production Zcash port composed from the existing Zebra delegates.

use std::fmt;

use async_trait::async_trait;
use lez_swap_core::Participant;
use lez_zec_swap_sdk::{
    CanonicalZcashOutputObservation, ClaimPreimage, FollowupClaimObservationV1,
    MakerLockObservationV1, PreparedClaimSubmissionV1, PreparedFirstLockSubmissionV1,
    PreparedRefundSubmissionV1, RefundEligibilityObservationV1, RefundObservationV1,
    RefundSubmitOutcomeV1, TakerFirstLockObservationV1, ZcashClaimContextV1, ZcashClaimPort,
    ZcashFirstLockPort, ZcashFundingContextV1, ZcashMakerLockObservationPort, ZcashRefundPort,
    ZcashTakerFirstLockObservationPort, ZecAgreementV1, ZecProfileId, ZecRefundProfile,
};

use crate::{
    ZebraClaimError, ZebraClaimSigner, ZebraFirstLockError, ZebraRefundError, ZebraRefundSigner,
    ZebraRpc, ZebraRpcClaimPort, ZebraRpcSwapPort,
};

/// A checked composite cannot bind a role-local signer to another actor role.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum ZebraRpcZcashPortConfigError {
    /// Signer capability is fixed to a different actor than the composite.
    #[error("Zcash signer role differs from the configured local actor")]
    SignerRoleMismatch {
        /// Role fixed by the composite.
        expected: Participant,
        /// Role fixed by the signer capability.
        actual: Participant,
    },
}

/// Cloneable production Zcash SDK port composed without behavior duplication.
///
/// First-lock submission and both funding-observation routes delegate to
/// [`ZebraRpcSwapPort`]. Claim and refund routes delegate to
/// [`ZebraRpcClaimPort`]. Exact funding-plan creation deliberately remains a
/// separate [`crate::ExactOutpointZcashFundingPlanner`] concern.
#[derive(Clone)]
pub struct ZebraRpcZcashPort<R, S> {
    funding: ZebraRpcSwapPort<R>,
    spending: ZebraRpcClaimPort<R, S>,
    counterparty_scan_blocks: u32,
}

impl<R, S> ZebraRpcZcashPort<R, S>
where
    R: Clone,
    S: ZebraClaimSigner + ZebraRefundSigner,
{
    /// Binds both delegates to one cloned RPC handle, identity, role, and signer.
    ///
    /// Construction requires both signing capabilities so every SDK Zcash port is
    /// available once `R` implements [`ZebraRpc`]. The refund capability supplies
    /// the signer role used for the checked actor binding; claim signing remains a
    /// separate trait capability and behavior is delegated unchanged.
    ///
    /// # Errors
    ///
    /// Rejects a refund-capable signer fixed to a different role before cloning
    /// the RPC handle or constructing either delegate.
    pub fn new(
        rpc: R,
        signer: S,
        identity: crate::ZebraChainIdentity,
        local_participant: Participant,
    ) -> Result<Self, ZebraRpcZcashPortConfigError> {
        let signer_participant = signer.participant();
        if signer_participant != local_participant {
            return Err(ZebraRpcZcashPortConfigError::SignerRoleMismatch {
                expected: local_participant,
                actual: signer_participant,
            });
        }
        let counterparty_scan_blocks = ZecRefundProfile::for_id(ZecProfileId::PublicTestnetV1)
            .zcash_refund_blocks()
            .saturating_add(1);
        let funding = ZebraRpcSwapPort::new(rpc.clone(), identity, local_participant)
            .with_counterparty_scan_blocks(counterparty_scan_blocks);
        let spending = ZebraRpcClaimPort::new(rpc, signer, identity)
            .with_counterparty_scan_blocks(counterparty_scan_blocks);
        Ok(Self {
            funding,
            spending,
            counterparty_scan_blocks,
        })
    }
}

impl<R, S> ZebraRpcZcashPort<R, S> {
    /// Immutable chain identity shared by both delegates.
    #[must_use]
    pub const fn identity(&self) -> crate::ZebraChainIdentity {
        self.funding.identity()
    }

    /// Local role shared by funding delegation and the checked signer.
    #[must_use]
    pub const fn local_participant(&self) -> Participant {
        self.funding.local_participant()
    }

    /// Shared finite counterparty discovery horizon.
    #[must_use]
    pub const fn counterparty_scan_blocks(&self) -> u32 {
        self.counterparty_scan_blocks
    }

    /// Configures the same nonzero discovery horizon on both delegates.
    #[must_use]
    pub fn with_counterparty_scan_blocks(mut self, maximum: u32) -> Self {
        let normalized = maximum.max(1);
        self.funding = self.funding.with_counterparty_scan_blocks(normalized);
        self.spending = self.spending.with_counterparty_scan_blocks(normalized);
        self.counterparty_scan_blocks = normalized;
        self
    }
}

impl<R, S> fmt::Debug for ZebraRpcZcashPort<R, S> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ZebraRpcZcashPort")
            .field("identity", &self.identity())
            .field("local_participant", &self.local_participant())
            .field("counterparty_scan_blocks", &self.counterparty_scan_blocks)
            .field("funding", &"[REDACTED]")
            .field("spending", &"[REDACTED]")
            .finish()
    }
}

#[async_trait]
impl<R, S> ZcashFirstLockPort for ZebraRpcZcashPort<R, S>
where
    R: ZebraRpc,
    S: Send + Sync,
{
    type Error = ZebraFirstLockError<R::Error>;

    async fn observe_first_lock(
        &self,
        agreement: &ZecAgreementV1,
        submission: &PreparedFirstLockSubmissionV1,
    ) -> Result<lez_zec_swap_sdk::FirstLockObservation, Self::Error> {
        self.funding.observe_first_lock(agreement, submission).await
    }

    async fn submit_first_lock(
        &self,
        agreement: &ZecAgreementV1,
        submission: &PreparedFirstLockSubmissionV1,
    ) -> Result<(), Self::Error> {
        self.funding.submit_first_lock(agreement, submission).await
    }
}

#[async_trait]
impl<R, S> ZcashTakerFirstLockObservationPort for ZebraRpcZcashPort<R, S>
where
    R: ZebraRpc,
    S: Send + Sync,
{
    type Error = ZebraFirstLockError<R::Error>;

    async fn observe_taker_first_lock(
        &self,
        agreement: &ZecAgreementV1,
        previous: Option<&CanonicalZcashOutputObservation>,
    ) -> Result<TakerFirstLockObservationV1, Self::Error> {
        self.funding
            .observe_taker_first_lock(agreement, previous)
            .await
    }
}

#[async_trait]
impl<R, S> ZcashMakerLockObservationPort for ZebraRpcZcashPort<R, S>
where
    R: ZebraRpc,
    S: Send + Sync,
{
    type Error = ZebraFirstLockError<R::Error>;

    async fn observe_maker_lock(
        &self,
        agreement: &ZecAgreementV1,
    ) -> Result<MakerLockObservationV1, Self::Error> {
        self.funding.observe_maker_lock(agreement).await
    }
}

#[async_trait]
impl<R, S> ZcashClaimPort for ZebraRpcZcashPort<R, S>
where
    R: ZebraRpc,
    S: ZebraClaimSigner,
{
    type Error = ZebraClaimError<R::Error, S::Error>;

    async fn observe_funding_before_reveal(
        &self,
        agreement: &ZecAgreementV1,
        context: &ZcashFundingContextV1,
    ) -> Result<lez_zec_swap_sdk::ZcashFundingObservationV1, Self::Error> {
        self.spending
            .observe_funding_before_reveal(agreement, context)
            .await
    }

    async fn prepare_followup_claim(
        &self,
        agreement: &ZecAgreementV1,
        context: &ZcashClaimContextV1,
        preimage: &ClaimPreimage,
    ) -> Result<PreparedClaimSubmissionV1, Self::Error> {
        self.spending
            .prepare_followup_claim(agreement, context, preimage)
            .await
    }

    async fn observe_prepared_followup_claim(
        &self,
        agreement: &ZecAgreementV1,
        context: &ZcashClaimContextV1,
        prepared: &PreparedClaimSubmissionV1,
    ) -> Result<FollowupClaimObservationV1, Self::Error> {
        self.spending
            .observe_prepared_followup_claim(agreement, context, prepared)
            .await
    }

    async fn observe_counterparty_followup_claim(
        &self,
        agreement: &ZecAgreementV1,
        context: &ZcashClaimContextV1,
    ) -> Result<FollowupClaimObservationV1, Self::Error> {
        self.spending
            .observe_counterparty_followup_claim(agreement, context)
            .await
    }

    async fn submit_followup_claim(
        &self,
        agreement: &ZecAgreementV1,
        context: &ZcashClaimContextV1,
        prepared: &PreparedClaimSubmissionV1,
    ) -> Result<(), Self::Error> {
        self.spending
            .submit_followup_claim(agreement, context, prepared)
            .await
    }
}

#[async_trait]
impl<R, S> ZcashRefundPort for ZebraRpcZcashPort<R, S>
where
    R: ZebraRpc,
    S: ZebraRefundSigner,
{
    type Error = ZebraRefundError<R::Error, S::Error>;

    async fn observe_refund_eligibility(
        &self,
        agreement: &ZecAgreementV1,
        context: &ZcashFundingContextV1,
    ) -> Result<RefundEligibilityObservationV1, Self::Error> {
        self.spending
            .observe_refund_eligibility(agreement, context)
            .await
    }

    async fn prepare_refund(
        &self,
        agreement: &ZecAgreementV1,
        context: &ZcashFundingContextV1,
    ) -> Result<PreparedRefundSubmissionV1, Self::Error> {
        self.spending.prepare_refund(agreement, context).await
    }

    async fn observe_prepared_refund(
        &self,
        agreement: &ZecAgreementV1,
        context: &ZcashFundingContextV1,
        prepared: &PreparedRefundSubmissionV1,
    ) -> Result<RefundObservationV1, Self::Error> {
        self.spending
            .observe_prepared_refund(agreement, context, prepared)
            .await
    }

    async fn observe_counterparty_refund(
        &self,
        agreement: &ZecAgreementV1,
        context: &ZcashFundingContextV1,
    ) -> Result<RefundObservationV1, Self::Error> {
        self.spending
            .observe_counterparty_refund(agreement, context)
            .await
    }

    async fn submit_refund(
        &self,
        agreement: &ZecAgreementV1,
        context: &ZcashFundingContextV1,
        prepared: &PreparedRefundSubmissionV1,
    ) -> Result<RefundSubmitOutcomeV1, Self::Error> {
        self.spending
            .submit_refund(agreement, context, prepared)
            .await
    }
}
