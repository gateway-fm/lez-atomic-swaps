use std::io::Cursor;

use async_trait::async_trait;
use lez_swap_core::{Chain, ChainPosition, Participant};
use lez_zec_swap_sdk::{
    Bip199Contract, Bip199SpendKind, CanonicalZcashOutputObservation,
    CanonicalZcashSpendObservation, ClaimError, ClaimPreimage, ClaimStepV1, ExpectedBip199Spend,
    FollowupClaimEvidenceV1, FollowupClaimObservationV1, ObservationError,
    PreparedClaimSubmissionV1, PreparedRefundSubmissionV1, RefundEligibilityObservationV1,
    RefundError, RefundEvidenceV1, RefundFundingWaitReasonV1, RefundObservationV1, RefundStepV1,
    RefundSubmitOutcomeV1, SpendObservationError, TransparentSpendRequest, ZcashClaimContextV1,
    ZcashClaimPort, ZcashFundingContextV1, ZcashFundingObservationV1, ZcashNodeSnapshot,
    ZcashRefundPort, ZcashSpendNodeSnapshot, ZcashStableTip, ZcashUnspentOutputSnapshotV1,
    ZecAgreementExecutionError, ZecAgreementV1,
};
use zcash_primitives::{
    block::BlockHash,
    transaction::{Transaction, TxVersion},
};
use zcash_protocol::{TxId, consensus::BlockHeight};
use zcash_script::script::Code;
use zcash_transparent::{
    address::Script,
    bundle::{OutPoint, TxOut},
};

use crate::rpc::{
    ZebraChainIdentity, ZebraChainInfo, ZebraRpc, ZebraSubmissionFailure, ZebraTransactionState,
    ZebraUnspentOutput,
};

const DEFAULT_COUNTERPARTY_SCAN_BLOCKS: u32 = 288;

/// Narrow secret-bearing capability used only after the adapter derives signed spend terms.
#[async_trait]
pub trait ZebraClaimSigner: Send + Sync {
    /// Structured key-provider or signing error.
    type Error: std::error::Error + Send + Sync + 'static;

    /// Signs the exact agreement-derived claim request with the role-local claimant key.
    async fn sign_claim(
        &self,
        contract: &Bip199Contract,
        request: &TransparentSpendRequest,
        preimage: &ClaimPreimage,
    ) -> Result<Vec<u8>, Self::Error>;
}

/// Narrow role-bearing capability for the agreement-derived timeout refund branch.
#[async_trait]
pub trait ZebraRefundSigner: Send + Sync {
    /// Structured key-provider or signing error.
    type Error: std::error::Error + Send + Sync + 'static;

    /// Fixed actor role owning this capability.
    fn participant(&self) -> Participant;

    /// Signs the exact agreement-derived refund request with the role-local refund key.
    async fn sign_refund(
        &self,
        contract: &Bip199Contract,
        request: &TransparentSpendRequest,
    ) -> Result<Vec<u8>, Self::Error>;
}

/// Production Zebra claim adapter with an injected role-scoped signing capability.
#[derive(Clone, Debug)]
pub struct ZebraRpcClaimPort<R, S> {
    rpc: R,
    signer: S,
    identity: ZebraChainIdentity,
    counterparty_scan_blocks: u32,
}

/// Production Zebra refund adapter sharing the bounded RPC implementation.
pub type ZebraRpcRefundPort<R, S> = ZebraRpcClaimPort<R, S>;

#[derive(Clone, Debug, Eq, PartialEq)]
struct FundingTarget {
    transaction_id: TxId,
    outpoint: OutPoint,
    output_index: u32,
}

#[derive(Clone, Debug)]
enum DiscoveredSpend {
    Mempool {
        transaction_id: TxId,
        raw_transaction: Vec<u8>,
    },
    Confirmed {
        transaction_id: TxId,
        raw_transaction: Vec<u8>,
        block_hash: BlockHash,
        block_height: BlockHeight,
    },
}

#[derive(Clone, Debug)]
struct DiscoveryAnchor {
    before: ZebraChainInfo,
    funding_height: BlockHeight,
    canonical_funding: CanonicalZcashOutputObservation,
}

impl DiscoveredSpend {
    fn transaction_id(&self) -> TxId {
        match self {
            Self::Mempool { transaction_id, .. } | Self::Confirmed { transaction_id, .. } => {
                *transaction_id
            }
        }
    }

    fn raw_transaction(&self) -> &[u8] {
        match self {
            Self::Mempool {
                raw_transaction, ..
            }
            | Self::Confirmed {
                raw_transaction, ..
            } => raw_transaction,
        }
    }
}

impl FundingTarget {
    fn from_context(context: &ZcashFundingContextV1) -> Self {
        Self {
            transaction_id: TxId::from_bytes(*context.funding_transaction_id_bytes()),
            outpoint: context.funding_outpoint().clone(),
            output_index: context.funding_output_index(),
        }
    }
}

impl<R, S> ZebraRpcClaimPort<R, S> {
    /// Binds one typed RPC and one signing capability to an immutable chain identity.
    #[must_use]
    pub const fn new(rpc: R, signer: S, identity: ZebraChainIdentity) -> Self {
        Self {
            rpc,
            signer,
            identity,
            counterparty_scan_blocks: DEFAULT_COUNTERPARTY_SCAN_BLOCKS,
        }
    }

    /// Replaces the finite canonical block horizon used for counterparty discovery.
    ///
    /// A zero value is normalized to one block, so discovery can never become unbounded.
    #[must_use]
    pub const fn with_counterparty_scan_blocks(mut self, maximum: u32) -> Self {
        self.counterparty_scan_blocks = if maximum == 0 { 1 } else { maximum };
        self
    }

    /// Typed transport used for all observations and broadcasts.
    #[must_use]
    pub const fn rpc(&self) -> &R {
        &self.rpc
    }

    /// Configured immutable Zebra chain identity.
    #[must_use]
    pub const fn identity(&self) -> ZebraChainIdentity {
        self.identity
    }
}

impl<R, S> ZebraRpcClaimPort<R, S>
where
    R: ZebraRpc,
{
    async fn exact_submission_known_after_failed_send(
        &self,
        before: ZebraChainInfo,
        transaction_id: TxId,
        exact_submission: &[u8],
    ) -> bool {
        let Ok(Some(state)) = self.rpc.transaction_state(transaction_id).await else {
            return false;
        };
        let (raw_transaction, confirmed) = match state {
            ZebraTransactionState::Mempool { raw_transaction } => (raw_transaction, None),
            ZebraTransactionState::Confirmed {
                raw_transaction,
                block_hash,
                block_height,
                confirmations,
                in_active_chain,
            } => (
                raw_transaction,
                Some((block_hash, block_height, confirmations, in_active_chain)),
            ),
        };
        if raw_transaction != exact_submission {
            return false;
        }
        if let Some((block_hash, block_height, _, in_active_chain)) = confirmed {
            let Ok(canonical_block_hash) = self.rpc.block_hash(block_height).await else {
                return false;
            };
            if !in_active_chain || canonical_block_hash != block_hash {
                return false;
            }
        }
        let Ok(after) = self.rpc.chain_info().await else {
            return false;
        };
        if after.rpc_chain() != self.identity.rpc_chain()
            || after.consensus_branch_id() != self.identity.consensus_branch_id()
            || !same_tip(before, after)
        {
            return false;
        }
        let Ok(genesis) = self.rpc.block_hash(BlockHeight::from_u32(0)).await else {
            return false;
        };
        if genesis != self.identity.genesis_hash() {
            return false;
        }
        if let Some((_, block_height, confirmations, _)) = confirmed {
            let Some(expected_confirmations) = u32::from(after.tip_height())
                .checked_sub(u32::from(block_height))
                .and_then(|depth| depth.checked_add(1))
            else {
                return false;
            };
            if confirmations != expected_confirmations {
                return false;
            }
        }
        true
    }
}

/// A fail-closed funding observation, claim validation, signing, or submission failure.
#[derive(Debug, thiserror::Error)]
pub enum ZebraClaimError<RE, SE>
where
    RE: std::error::Error + Send + Sync + 'static,
    SE: std::error::Error + Send + Sync + 'static,
{
    /// The configured network differs from the accepted agreement.
    #[error("configured Zebra network differs from the accepted agreement")]
    ConfiguredNetworkMismatch,
    /// The configured branch differs from the accepted agreement.
    #[error("configured Zebra branch differs from the accepted agreement")]
    ConfiguredConsensusBranchMismatch,
    /// The live RPC chain spelling differs from immutable configuration.
    #[error("Zebra RPC chain differs from configured identity")]
    RpcChainMismatch,
    /// The live RPC tip branch differs from immutable configuration.
    #[error("Zebra RPC consensus branch differs from configured identity")]
    RpcConsensusBranchMismatch,
    /// Height zero differs from the configured immutable genesis hash.
    #[error("Zebra genesis block differs from configured identity")]
    GenesisMismatch,
    /// A typed Zebra RPC operation failed.
    #[error("typed Zebra RPC operation failed: {0}")]
    Rpc(#[source] RE),
    /// Role-local signing failed without exposing key or preimage material.
    #[error("Zcash follow-up claim signing failed: {0}")]
    Signer(#[source] SE),
    /// Agreement-derived claim policy could not be constructed.
    #[error("agreement-derived Zcash claim request failed: {0}")]
    Agreement(#[source] ZecAgreementExecutionError),
    /// SDK canonical funding validation rejected the node snapshot.
    #[error("Zebra canonical funding validation failed: {0}")]
    FundingObservation(#[source] ObservationError),
    /// SDK canonical spend validation rejected the node snapshot or exact bytes.
    #[error("Zebra canonical claim validation failed: {0}")]
    SpendObservation(#[source] SpendObservationError),
    /// SDK claim evidence or prepared-submission validation failed.
    #[error("Zcash follow-up claim evidence was invalid: {0}")]
    Claim(#[source] ClaimError),
    /// A prepared claim carried the wrong ordered step.
    #[error("Zebra claim adapter received wrong claim step {0:?}")]
    WrongStep(ClaimStepV1),
    /// Exact claim bytes were not one canonical V5 transaction.
    #[error("prepared Zcash claim transaction is malformed: {0}")]
    MalformedSubmission(#[source] std::io::Error),
    /// Bytes remained after decoding exactly one claim transaction.
    #[error("prepared Zcash claim transaction contains trailing bytes")]
    TrailingSubmissionBytes,
    /// The exact claim was not a V5 transaction.
    #[error("prepared Zcash claim transaction is not V5")]
    WrongTransactionVersion,
    /// Canonical bytes did not derive the durable expected identity.
    #[error("prepared Zcash claim transaction ID differs from durable identity")]
    ExpectedTransactionIdMismatch,
    /// The prepared transaction did not spend exactly the coordinator-pinned funding outpoint.
    #[error("prepared Zcash claim does not spend exactly the durable funding outpoint")]
    FundingOutpointMismatch,
    /// The transaction expiry cannot have been derived from the signed relative expiry policy.
    #[error("prepared Zcash claim expiry is incompatible with signed policy")]
    ClaimExpiryPolicyMismatch,
    /// A valid spend deviated from the SDK's exact signed destination/fee/shape policy.
    #[error("prepared Zcash claim deviates from signed canonical transaction policy")]
    NonCanonicalClaimPolicy,
    /// The funding `gettxout` response was not bound to the stable bracketing tip.
    #[error("Zebra UTXO response names a different best-chain tip")]
    UtxoTipMismatch,
    /// Funding transaction and UTXO confirmation counts disagreed in one stable view.
    #[error("Zebra funding transaction and UTXO confirmation counts disagree")]
    UtxoConfirmationMismatch,
    /// Zebra returned bytes different from the retained exact claim.
    #[error("Zebra claim bytes differ from the retained exact submission")]
    ObservedClaimBytesMismatch,
    /// The exact transaction validly executes the refund branch, not the required claim branch.
    #[error("Zebra transaction executes the refund branch instead of the claim branch")]
    WrongSpendBranch,
    /// Claim preparation was attempted without stable canonical unspent funding.
    #[error("Zcash funding is not currently stable, canonical, and unspent")]
    FundingNotReady,
    /// Zebra definitively rejected the exact submitted transaction.
    #[error("Zebra definitively rejected the exact follow-up claim")]
    SubmissionRejected(#[source] RE),
    /// The broadcast outcome is unknown and must be observed before any retry.
    #[error("Zebra follow-up claim broadcast outcome is unknown")]
    SubmissionOutcomeUnknown(#[source] RE),
    /// Zebra accepted bytes but returned a different transaction identity.
    #[error("Zebra returned a transaction ID different from the retained claim")]
    SubmittedTransactionIdMismatch,
    /// The chain tip changed after broadcast, so acceptance remains an unknown outcome.
    #[error("Zebra tip changed during follow-up claim broadcast; outcome requires observation")]
    UnstableTipDuringSubmission,
    /// A hash-addressed block inventory disagreed with the canonical height/hash query.
    #[error("Zebra block inventory differs from its canonical height/hash binding")]
    BlockInventoryMismatch,
    /// A transaction identity and its exact fetched bytes disagreed.
    #[error("Zebra transaction inventory identity differs from exact transaction bytes")]
    DiscoveredTransactionIdMismatch,
    /// One stable scan exposed conflicting transactions spending the exact durable outpoint.
    #[error("Zebra exposed multiple transactions spending the exact durable outpoint")]
    ConflictingSpendCandidates,
    /// Role-local refund signing failed without exposing key material.
    #[error("Zcash refund signing failed: {0}")]
    RefundSigner(#[source] SE),
    /// SDK refund evidence or prepared-submission validation failed.
    #[error("Zcash refund evidence was invalid: {0}")]
    Refund(#[source] RefundError),
    /// A prepared refund carried the wrong ordered step.
    #[error("Zebra refund adapter received wrong refund step {0:?}")]
    WrongRefundStep(RefundStepV1),
    /// The fixed signer role does not own the Zcash refund branch.
    #[error("Zcash refund signer role mismatch: expected {expected:?}, got {actual:?}")]
    WrongRefundSignerRole {
        /// Agreement-derived Zcash refund owner.
        expected: Participant,
        /// Role bound to the injected signer.
        actual: Participant,
    },
    /// Durable funding context does not belong to the supplied agreement.
    #[error("Zcash refund funding context differs from the accepted agreement")]
    RefundContextMismatch,
    /// Refund preparation was attempted without stable canonical unspent funding.
    #[error("Zcash refund funding is not currently stable, canonical, and unspent")]
    RefundFundingNotReady,
    /// Refund preparation was attempted before the agreement CLTV height.
    #[error("Zcash refund CLTV height has not been reached")]
    RefundDeadlineNotReached,
    /// The refund expiry cannot have been derived from the signed relative policy.
    #[error("prepared Zcash refund expiry is incompatible with signed policy")]
    RefundExpiryPolicyMismatch,
    /// The exact spend executed the claim branch instead of the refund branch.
    #[error("Zebra transaction executes the claim branch instead of the refund branch")]
    WrongRefundSpendBranch,
    /// A valid refund deviated from exact destination, fee, expiry, or shape policy.
    #[error("prepared Zcash refund deviates from signed canonical transaction policy")]
    NonCanonicalRefundPolicy,
    /// Zebra returned bytes different from the retained exact refund.
    #[error("Zebra refund bytes differ from the retained exact submission")]
    ObservedRefundBytesMismatch,
}

/// Fail-closed Zebra refund error surface.
pub type ZebraRefundError<RE, SE> = ZebraClaimError<RE, SE>;

#[async_trait]
impl<R, S> ZcashRefundPort for ZebraRpcClaimPort<R, S>
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
        self.refund_validate_context(agreement, context)?;
        Ok(
            match self.refund_observe_funding(agreement, context).await? {
                ZcashFundingObservationV1::Unstable => RefundEligibilityObservationV1::Unstable,
                ZcashFundingObservationV1::Absent => {
                    RefundEligibilityObservationV1::FundingUnavailable(
                        RefundFundingWaitReasonV1::Reorged,
                    )
                }
                ZcashFundingObservationV1::Spent => {
                    RefundEligibilityObservationV1::FundingUnavailable(
                        RefundFundingWaitReasonV1::Spent,
                    )
                }
                ZcashFundingObservationV1::Confirmed { canonical, .. } => {
                    RefundEligibilityObservationV1::canonical(ChainPosition::block_height(
                        Chain::Zcash,
                        u64::from(u32::from(canonical.tip_height())),
                    ))
                }
            },
        )
    }

    async fn prepare_refund(
        &self,
        agreement: &ZecAgreementV1,
        context: &ZcashFundingContextV1,
    ) -> Result<PreparedRefundSubmissionV1, Self::Error> {
        self.refund_validate_owner(agreement)?;
        self.refund_validate_context(agreement, context)?;
        let observation = self.refund_observe_funding(agreement, context).await?;
        let ZcashFundingObservationV1::Confirmed { canonical, unspent } = observation else {
            return Err(ZebraClaimError::RefundFundingNotReady);
        };
        if u32::from(canonical.tip_height()) < agreement.zcash_refund_at_height() {
            return Err(ZebraClaimError::RefundDeadlineNotReached);
        }
        let request = agreement
            .refund_spend_request(
                context.funding_outpoint().clone(),
                unspent.output().clone(),
                canonical.tip_height(),
            )
            .map_err(ZebraClaimError::Agreement)?;
        let exact = self
            .signer
            .sign_refund(agreement.binding().expected_output().contract(), &request)
            .await
            .map_err(ZebraClaimError::RefundSigner)?;
        let transaction_id = decode_transaction_id::<R::Error, S::Error>(
            &exact,
            self.identity.consensus_branch_id(),
        )?;
        let prepared =
            PreparedRefundSubmissionV1::new(RefundStepV1::Zcash, *transaction_id.as_ref(), exact)
                .map_err(ZebraClaimError::Refund)?;
        self.refund_validate_prepared(agreement, context, &prepared)?;
        Ok(prepared)
    }

    async fn observe_prepared_refund(
        &self,
        agreement: &ZecAgreementV1,
        context: &ZcashFundingContextV1,
        prepared: &PreparedRefundSubmissionV1,
    ) -> Result<RefundObservationV1, Self::Error> {
        let transaction_id = self.refund_validate_prepared(agreement, context, prepared)?;
        let before = self.refund_sample_validated_tip().await?;
        self.refund_validate_genesis().await?;
        let state = self
            .rpc
            .transaction_state(transaction_id)
            .await
            .map_err(ZebraClaimError::Rpc)?;
        let canonical_block = match &state {
            Some(ZebraTransactionState::Confirmed { block_height, .. }) => Some(
                self.rpc
                    .block_hash(*block_height)
                    .await
                    .map_err(ZebraClaimError::Rpc)?,
            ),
            Some(ZebraTransactionState::Mempool { .. }) | None => None,
        };
        let after = self.refund_sample_validated_tip().await?;
        if !same_tip(before, after) {
            return Ok(RefundObservationV1::Unstable);
        }
        match state {
            None => Ok(RefundObservationV1::Absent),
            Some(ZebraTransactionState::Mempool { raw_transaction }) => {
                require_exact_refund::<R::Error, S::Error>(
                    &raw_transaction,
                    prepared.exact_submission(),
                )?;
                Ok(RefundObservationV1::Unstable)
            }
            Some(ZebraTransactionState::Confirmed {
                raw_transaction,
                block_hash,
                block_height,
                confirmations,
                in_active_chain,
            }) => {
                require_exact_refund::<R::Error, S::Error>(
                    &raw_transaction,
                    prepared.exact_submission(),
                )?;
                let canonical = self.refund_validate_snapshot(
                    agreement,
                    context.funding_outpoint(),
                    transaction_id,
                    raw_transaction,
                    block_hash,
                    canonical_block.expect("confirmed state resolves a block"),
                    block_height,
                    before,
                    after,
                    confirmations,
                    in_active_chain,
                )?;
                Self::refund_evidence(agreement, &canonical).map(RefundObservationV1::Confirmed)
            }
        }
    }

    async fn observe_counterparty_refund(
        &self,
        agreement: &ZecAgreementV1,
        context: &ZcashFundingContextV1,
    ) -> Result<RefundObservationV1, Self::Error> {
        self.refund_discover_counterparty(agreement, context).await
    }

    async fn submit_refund(
        &self,
        agreement: &ZecAgreementV1,
        context: &ZcashFundingContextV1,
        prepared: &PreparedRefundSubmissionV1,
    ) -> Result<RefundSubmitOutcomeV1, Self::Error> {
        let transaction_id = self.refund_validate_prepared(agreement, context, prepared)?;
        let before = self.refund_sample_validated_tip().await?;
        self.refund_validate_genesis().await?;
        let submitted = match self
            .rpc
            .send_raw_transaction(prepared.exact_submission())
            .await
        {
            Ok(transaction_id) => transaction_id,
            Err(error) => {
                if self
                    .exact_submission_known_after_failed_send(
                        before,
                        transaction_id,
                        prepared.exact_submission(),
                    )
                    .await
                {
                    return Ok(RefundSubmitOutcomeV1::Accepted);
                }
                return Ok(match R::classify_submission_failure(&error) {
                    ZebraSubmissionFailure::DefinitiveRejection => {
                        RefundSubmitOutcomeV1::DefinitivelyRejected
                    }
                    ZebraSubmissionFailure::UnknownOutcome => RefundSubmitOutcomeV1::Unknown,
                });
            }
        };
        let Ok(after) = self.rpc.chain_info().await else {
            return Ok(RefundSubmitOutcomeV1::Unknown);
        };
        let Ok(post_submit_genesis) = self.rpc.block_hash(BlockHeight::from_u32(0)).await else {
            return Ok(RefundSubmitOutcomeV1::Unknown);
        };
        if submitted != transaction_id
            || after.rpc_chain() != self.identity.rpc_chain()
            || after.consensus_branch_id() != self.identity.consensus_branch_id()
            || post_submit_genesis != self.identity.genesis_hash()
            || !same_tip(before, after)
        {
            return Ok(RefundSubmitOutcomeV1::Unknown);
        }
        Ok(RefundSubmitOutcomeV1::Accepted)
    }
}

#[async_trait]
impl<R, S> ZcashClaimPort for ZebraRpcClaimPort<R, S>
where
    R: ZebraRpc,
    S: ZebraClaimSigner,
{
    type Error = ZebraClaimError<R::Error, S::Error>;

    async fn observe_funding_before_reveal(
        &self,
        agreement: &ZecAgreementV1,
        context: &ZcashFundingContextV1,
    ) -> Result<ZcashFundingObservationV1, Self::Error> {
        self.observe_funding(agreement, context).await
    }

    async fn prepare_followup_claim(
        &self,
        agreement: &ZecAgreementV1,
        context: &ZcashClaimContextV1,
        preimage: &ClaimPreimage,
    ) -> Result<PreparedClaimSubmissionV1, Self::Error> {
        let observation = self
            .observe_funding(agreement, context.funding_context())
            .await?;
        let ZcashFundingObservationV1::Confirmed { canonical, unspent } = observation else {
            return Err(ZebraClaimError::FundingNotReady);
        };
        let request = agreement
            .claim_spend_request(
                context.funding_outpoint().clone(),
                unspent.output().clone(),
                canonical.tip_height(),
            )
            .map_err(ZebraClaimError::Agreement)?;
        let exact = self
            .signer
            .sign_claim(
                agreement.binding().expected_output().contract(),
                &request,
                preimage,
            )
            .await
            .map_err(ZebraClaimError::Signer)?;
        let transaction_id = decode_transaction_id::<R::Error, S::Error>(
            &exact,
            self.identity.consensus_branch_id(),
        )?;
        let prepared = PreparedClaimSubmissionV1::new(
            ClaimStepV1::FollowupZcash,
            *transaction_id.as_ref(),
            exact,
        )
        .map_err(ZebraClaimError::Claim)?;
        self.validate_prepared(agreement, context, &prepared)?;
        Ok(prepared)
    }

    async fn observe_prepared_followup_claim(
        &self,
        agreement: &ZecAgreementV1,
        context: &ZcashClaimContextV1,
        prepared: &PreparedClaimSubmissionV1,
    ) -> Result<FollowupClaimObservationV1, Self::Error> {
        let transaction_id = self.validate_prepared(agreement, context, prepared)?;
        let before = self.sample_validated_tip().await?;
        self.validate_genesis().await?;
        let state = self
            .rpc
            .transaction_state(transaction_id)
            .await
            .map_err(ZebraClaimError::Rpc)?;
        let canonical_block = match &state {
            Some(ZebraTransactionState::Confirmed { block_height, .. }) => Some(
                self.rpc
                    .block_hash(*block_height)
                    .await
                    .map_err(ZebraClaimError::Rpc)?,
            ),
            Some(ZebraTransactionState::Mempool { .. }) | None => None,
        };
        let after = self.sample_validated_tip().await?;
        if !same_tip(before, after) {
            return Ok(FollowupClaimObservationV1::Unstable);
        }
        match state {
            None => Ok(FollowupClaimObservationV1::Absent),
            Some(ZebraTransactionState::Mempool { raw_transaction }) => {
                require_exact_claim::<R::Error, S::Error>(
                    &raw_transaction,
                    prepared.exact_submission(),
                )?;
                Ok(FollowupClaimObservationV1::Unstable)
            }
            Some(ZebraTransactionState::Confirmed {
                raw_transaction,
                block_hash,
                block_height,
                confirmations,
                in_active_chain,
            }) => {
                require_exact_claim::<R::Error, S::Error>(
                    &raw_transaction,
                    prepared.exact_submission(),
                )?;
                let canonical = self.validate_claim_snapshot(
                    agreement,
                    context.funding_outpoint(),
                    transaction_id,
                    raw_transaction,
                    block_hash,
                    canonical_block.expect("confirmed state resolves a block"),
                    block_height,
                    before,
                    after,
                    confirmations,
                    in_active_chain,
                )?;
                let evidence = FollowupClaimEvidenceV1::new(
                    agreement,
                    *prepared.expected_submission_id(),
                    canonical.transaction_id().to_string(),
                    canonical.confirmations().get(),
                )
                .map_err(ZebraClaimError::Claim)?;
                Ok(FollowupClaimObservationV1::Confirmed(evidence))
            }
        }
    }

    async fn observe_counterparty_followup_claim(
        &self,
        agreement: &ZecAgreementV1,
        context: &ZcashClaimContextV1,
    ) -> Result<FollowupClaimObservationV1, Self::Error> {
        self.discover_counterparty_claim(agreement, context).await
    }

    async fn submit_followup_claim(
        &self,
        agreement: &ZecAgreementV1,
        context: &ZcashClaimContextV1,
        prepared: &PreparedClaimSubmissionV1,
    ) -> Result<(), Self::Error> {
        let transaction_id = self.validate_prepared(agreement, context, prepared)?;
        let before = self.sample_validated_tip().await?;
        self.validate_genesis().await?;
        let submitted = match self
            .rpc
            .send_raw_transaction(prepared.exact_submission())
            .await
        {
            Ok(transaction_id) => transaction_id,
            Err(error) => {
                if self
                    .exact_submission_known_after_failed_send(
                        before,
                        transaction_id,
                        prepared.exact_submission(),
                    )
                    .await
                {
                    return Ok(());
                }
                return Err(match R::classify_submission_failure(&error) {
                    ZebraSubmissionFailure::DefinitiveRejection => {
                        ZebraClaimError::SubmissionRejected(error)
                    }
                    ZebraSubmissionFailure::UnknownOutcome => {
                        ZebraClaimError::SubmissionOutcomeUnknown(error)
                    }
                });
            }
        };
        let after = self
            .rpc
            .chain_info()
            .await
            .map_err(ZebraClaimError::SubmissionOutcomeUnknown)?;
        let post_submit_genesis = self
            .rpc
            .block_hash(BlockHeight::from_u32(0))
            .await
            .map_err(ZebraClaimError::SubmissionOutcomeUnknown)?;
        if after.rpc_chain() != self.identity.rpc_chain()
            || after.consensus_branch_id() != self.identity.consensus_branch_id()
            || post_submit_genesis != self.identity.genesis_hash()
            || !same_tip(before, after)
        {
            return Err(ZebraClaimError::UnstableTipDuringSubmission);
        }
        if submitted != transaction_id {
            return Err(ZebraClaimError::SubmittedTransactionIdMismatch);
        }
        Ok(())
    }
}

impl<R, S> ZebraRpcClaimPort<R, S>
where
    R: ZebraRpc,
    S: ZebraRefundSigner,
{
    fn refund_validate_context(
        &self,
        agreement: &ZecAgreementV1,
        context: &ZcashFundingContextV1,
    ) -> Result<(), ZebraRefundError<R::Error, S::Error>> {
        self.refund_validate_agreement_identity(agreement)?;
        if context.agreement_commitment() != agreement.agreement_commitment()
            || context.swap_id() != agreement.coordinator().id()
            || context.zcash_funder() != agreement.lez_claimant()
        {
            return Err(ZebraClaimError::RefundContextMismatch);
        }
        Ok(())
    }

    fn refund_validate_owner(
        &self,
        agreement: &ZecAgreementV1,
    ) -> Result<(), ZebraRefundError<R::Error, S::Error>> {
        let expected = agreement.lez_claimant();
        let actual = self.signer.participant();
        if actual != expected {
            return Err(ZebraClaimError::WrongRefundSignerRole { expected, actual });
        }
        Ok(())
    }

    async fn refund_observe_funding(
        &self,
        agreement: &ZecAgreementV1,
        context: &ZcashFundingContextV1,
    ) -> Result<ZcashFundingObservationV1, ZebraRefundError<R::Error, S::Error>> {
        self.refund_validate_context(agreement, context)?;
        let target = FundingTarget::from_context(context);
        let before = self.refund_sample_validated_tip().await?;
        self.refund_validate_genesis().await?;
        let state = self
            .rpc
            .transaction_state(target.transaction_id)
            .await
            .map_err(ZebraClaimError::Rpc)?;
        let canonical_block = match &state {
            Some(ZebraTransactionState::Confirmed { block_height, .. }) => Some(
                self.rpc
                    .block_hash(*block_height)
                    .await
                    .map_err(ZebraClaimError::Rpc)?,
            ),
            Some(ZebraTransactionState::Mempool { .. }) | None => None,
        };
        let unspent = match &state {
            Some(ZebraTransactionState::Confirmed { .. }) => self
                .rpc
                .unspent_output(&target.outpoint)
                .await
                .map_err(ZebraClaimError::Rpc)?,
            Some(ZebraTransactionState::Mempool { .. }) | None => None,
        };
        let after = self.refund_sample_validated_tip().await?;
        if !same_tip(before, after) {
            return Ok(ZcashFundingObservationV1::Unstable);
        }
        match state {
            None => Ok(ZcashFundingObservationV1::Absent),
            Some(ZebraTransactionState::Mempool { .. }) => Ok(ZcashFundingObservationV1::Unstable),
            Some(ZebraTransactionState::Confirmed {
                raw_transaction,
                block_hash,
                block_height,
                confirmations,
                in_active_chain,
            }) => {
                if !in_active_chain
                    || canonical_block.expect("confirmed state resolves a block") != block_hash
                {
                    return Ok(ZcashFundingObservationV1::Unstable);
                }
                let snapshot = ZcashNodeSnapshot::new(
                    self.identity.network(),
                    self.identity.consensus_branch_id(),
                    true,
                    block_hash,
                    block_hash,
                    block_height,
                    stable_tip(before, after),
                    target.transaction_id,
                    raw_transaction,
                    target.output_index,
                    confirmations,
                );
                let canonical = CanonicalZcashOutputObservation::validate(
                    agreement.binding().expected_output(),
                    &snapshot,
                )
                .map_err(ZebraClaimError::FundingObservation)?;
                if canonical.transaction_id() != target.transaction_id
                    || canonical.outpoint() != &target.outpoint
                {
                    return Err(ZebraClaimError::FundingOutpointMismatch);
                }
                let Some(unspent) = unspent else {
                    return Ok(ZcashFundingObservationV1::Spent);
                };
                validate_unspent_binding::<R::Error, S::Error>(&unspent, &canonical, before)?;
                let unspent = ZcashUnspentOutputSnapshotV1::new(
                    target.outpoint,
                    unspent.output().clone(),
                    stable_tip(before, after),
                );
                Ok(ZcashFundingObservationV1::confirmed(canonical, unspent))
            }
        }
    }

    fn refund_validate_prepared(
        &self,
        agreement: &ZecAgreementV1,
        context: &ZcashFundingContextV1,
        prepared: &PreparedRefundSubmissionV1,
    ) -> Result<TxId, ZebraRefundError<R::Error, S::Error>> {
        self.refund_validate_owner(agreement)?;
        self.refund_validate_context(agreement, context)?;
        if prepared.step() != RefundStepV1::Zcash {
            return Err(ZebraClaimError::WrongRefundStep(prepared.step()));
        }
        let transaction_id = decode_transaction_id::<R::Error, S::Error>(
            prepared.exact_submission(),
            self.identity.consensus_branch_id(),
        )?;
        if transaction_id.as_ref() != prepared.expected_submission_id() {
            return Err(ZebraClaimError::ExpectedTransactionIdMismatch);
        }
        let transaction = decode_transaction::<R::Error, S::Error>(
            prepared.exact_submission(),
            self.identity.consensus_branch_id(),
        )?;
        let prepared_height = u32::from(transaction.expiry_height())
            .checked_sub(agreement.transaction_policy().expiry_delta_blocks())
            .map(BlockHeight::from_u32)
            .ok_or(ZebraClaimError::RefundExpiryPolicyMismatch)?;
        let expected = expected_refund_spend::<R::Error, S::Error>(
            agreement,
            context.funding_outpoint().clone(),
            prepared.exact_submission(),
        )?;
        let dummy_hash = BlockHash([1; 32]);
        let snapshot = ZcashSpendNodeSnapshot::new(
            self.identity.network(),
            self.identity.consensus_branch_id(),
            true,
            dummy_hash,
            dummy_hash,
            BlockHeight::from_u32(1),
            ZcashStableTip::new(
                dummy_hash,
                BlockHeight::from_u32(1),
                dummy_hash,
                BlockHeight::from_u32(1),
            ),
            transaction_id,
            prepared.exact_submission().to_vec(),
            1,
        );
        let canonical = CanonicalZcashSpendObservation::validate(&expected, &snapshot)
            .map_err(ZebraClaimError::SpendObservation)?;
        if !matches!(canonical.kind(), Bip199SpendKind::Refund { .. }) {
            return Err(ZebraClaimError::WrongRefundSpendBranch);
        }
        if !canonical.sdk_canonical_policy().is_compliant() {
            return Err(ZebraClaimError::NonCanonicalRefundPolicy);
        }
        agreement
            .validate_refund_spend_request(
                &agreement
                    .refund_spend_request(
                        context.funding_outpoint().clone(),
                        expected.funding_output().clone(),
                        prepared_height,
                    )
                    .map_err(ZebraClaimError::Agreement)?,
                prepared_height,
            )
            .map_err(ZebraClaimError::Agreement)?;
        let bundle = transaction
            .transparent_bundle()
            .ok_or(ZebraClaimError::FundingOutpointMismatch)?;
        if bundle.vin.len() != 1
            || bundle.vout.len() != 1
            || bundle.vin[0].prevout() != context.funding_outpoint()
        {
            return Err(ZebraClaimError::FundingOutpointMismatch);
        }
        Ok(transaction_id)
    }

    #[allow(clippy::too_many_arguments)]
    fn refund_validate_snapshot(
        &self,
        agreement: &ZecAgreementV1,
        outpoint: &OutPoint,
        transaction_id: TxId,
        raw_transaction: Vec<u8>,
        block_hash: BlockHash,
        canonical_block_hash: BlockHash,
        block_height: BlockHeight,
        before: ZebraChainInfo,
        after: ZebraChainInfo,
        confirmations: u32,
        in_active_chain: bool,
    ) -> Result<CanonicalZcashSpendObservation, ZebraRefundError<R::Error, S::Error>> {
        let expected = expected_refund_spend(agreement, outpoint.clone(), &raw_transaction)?;
        let snapshot = ZcashSpendNodeSnapshot::new(
            self.identity.network(),
            self.identity.consensus_branch_id(),
            in_active_chain,
            block_hash,
            canonical_block_hash,
            block_height,
            stable_tip(before, after),
            transaction_id,
            raw_transaction,
            confirmations,
        );
        let canonical = CanonicalZcashSpendObservation::validate(&expected, &snapshot)
            .map_err(ZebraClaimError::SpendObservation)?;
        if !matches!(canonical.kind(), Bip199SpendKind::Refund { .. }) {
            return Err(ZebraClaimError::WrongRefundSpendBranch);
        }
        if !canonical.sdk_canonical_policy().is_compliant() {
            return Err(ZebraClaimError::NonCanonicalRefundPolicy);
        }
        Ok(canonical)
    }

    fn refund_evidence(
        agreement: &ZecAgreementV1,
        canonical: &CanonicalZcashSpendObservation,
    ) -> Result<RefundEvidenceV1, ZebraRefundError<R::Error, S::Error>> {
        RefundEvidenceV1::new(
            agreement,
            RefundStepV1::Zcash,
            *canonical.transaction_id().as_ref(),
            canonical.transaction_id().to_string(),
            ChainPosition::block_height(Chain::Zcash, u64::from(u32::from(canonical.tip_height()))),
            canonical.confirmations().get(),
        )
        .map_err(ZebraClaimError::Refund)
    }

    async fn refund_discover_counterparty(
        &self,
        agreement: &ZecAgreementV1,
        context: &ZcashFundingContextV1,
    ) -> Result<RefundObservationV1, ZebraRefundError<R::Error, S::Error>> {
        let Some(anchor) = self
            .refund_validated_discovery_anchor(agreement, context)
            .await?
        else {
            return Ok(RefundObservationV1::Unstable);
        };
        if let Some(unspent) = self
            .rpc
            .unspent_output(context.funding_outpoint())
            .await
            .map_err(ZebraClaimError::Rpc)?
        {
            let after = self.refund_sample_validated_tip().await?;
            if !same_tip(anchor.before, after) {
                return Ok(RefundObservationV1::Unstable);
            }
            validate_unspent_binding::<R::Error, S::Error>(
                &unspent,
                &anchor.canonical_funding,
                anchor.before,
            )?;
            return Ok(RefundObservationV1::Absent);
        }
        let candidates = self
            .refund_scan_discovered_spends(context, anchor.before, anchor.funding_height)
            .await?;
        let after = self.refund_sample_validated_tip().await?;
        if !same_tip(anchor.before, after) {
            return Ok(RefundObservationV1::Unstable);
        }
        let Some(candidate) = candidates.into_iter().next() else {
            return Ok(RefundObservationV1::Unstable);
        };
        match candidate {
            DiscoveredSpend::Mempool {
                transaction_id,
                raw_transaction,
            } => {
                self.refund_validate_discovered_policy(
                    agreement,
                    context.funding_outpoint(),
                    transaction_id,
                    raw_transaction,
                )?;
                Ok(RefundObservationV1::Unstable)
            }
            DiscoveredSpend::Confirmed {
                transaction_id,
                raw_transaction,
                block_hash,
                block_height,
            } => {
                let confirmations = u32::from(anchor.before.tip_height())
                    .checked_sub(u32::from(block_height))
                    .and_then(|distance| distance.checked_add(1))
                    .ok_or(ZebraClaimError::BlockInventoryMismatch)?;
                let canonical = self.refund_validate_snapshot(
                    agreement,
                    context.funding_outpoint(),
                    transaction_id,
                    raw_transaction,
                    block_hash,
                    block_hash,
                    block_height,
                    anchor.before,
                    after,
                    confirmations,
                    true,
                )?;
                Self::refund_evidence(agreement, &canonical).map(RefundObservationV1::Confirmed)
            }
        }
    }

    async fn refund_validated_discovery_anchor(
        &self,
        agreement: &ZecAgreementV1,
        context: &ZcashFundingContextV1,
    ) -> Result<Option<DiscoveryAnchor>, ZebraRefundError<R::Error, S::Error>> {
        self.refund_validate_context(agreement, context)?;
        let before = self.refund_sample_validated_tip().await?;
        self.refund_validate_genesis().await?;
        let funding_id = TxId::from_bytes(*context.funding_transaction_id_bytes());
        let Some(ZebraTransactionState::Confirmed {
            raw_transaction: funding_raw,
            block_hash: funding_block_hash,
            block_height: funding_height,
            confirmations: funding_confirmations,
            in_active_chain: true,
        }) = self
            .rpc
            .transaction_state(funding_id)
            .await
            .map_err(ZebraClaimError::Rpc)?
        else {
            return Ok(None);
        };
        let canonical_funding_block = self
            .rpc
            .block_hash(funding_height)
            .await
            .map_err(ZebraClaimError::Rpc)?;
        if canonical_funding_block != funding_block_hash {
            return Ok(None);
        }
        let snapshot = ZcashNodeSnapshot::new(
            self.identity.network(),
            self.identity.consensus_branch_id(),
            true,
            funding_block_hash,
            canonical_funding_block,
            funding_height,
            stable_tip(before, before),
            funding_id,
            funding_raw,
            context.funding_output_index(),
            funding_confirmations,
        );
        let canonical_funding = CanonicalZcashOutputObservation::validate(
            agreement.binding().expected_output(),
            &snapshot,
        )
        .map_err(ZebraClaimError::FundingObservation)?;
        if canonical_funding.outpoint() != context.funding_outpoint() {
            return Err(ZebraClaimError::FundingOutpointMismatch);
        }
        Ok(Some(DiscoveryAnchor {
            before,
            funding_height,
            canonical_funding,
        }))
    }

    async fn refund_scan_discovered_spends(
        &self,
        context: &ZcashFundingContextV1,
        before: ZebraChainInfo,
        funding_height: BlockHeight,
    ) -> Result<Vec<DiscoveredSpend>, ZebraRefundError<R::Error, S::Error>> {
        let mut candidates = Vec::new();
        for transaction_id in self
            .rpc
            .mempool_transaction_ids()
            .await
            .map_err(ZebraClaimError::Rpc)?
        {
            let Some(raw_transaction) = self
                .rpc
                .raw_transaction(transaction_id)
                .await
                .map_err(ZebraClaimError::Rpc)?
            else {
                continue;
            };
            if self.refund_exact_outpoint_spender(context, transaction_id, &raw_transaction)? {
                push_discovered::<R::Error, S::Error>(
                    &mut candidates,
                    DiscoveredSpend::Mempool {
                        transaction_id,
                        raw_transaction,
                    },
                )?;
            }
        }
        let tip_height = u32::from(before.tip_height());
        let funding_height = u32::from(funding_height);
        if funding_height > tip_height {
            return Ok(candidates);
        }
        let lower_bound = tip_height
            .saturating_add(1)
            .saturating_sub(self.counterparty_scan_blocks)
            .max(funding_height);
        for height in lower_bound..=tip_height {
            let height = BlockHeight::from_u32(height);
            let block_hash = self
                .rpc
                .block_hash(height)
                .await
                .map_err(ZebraClaimError::Rpc)?;
            let block = self
                .rpc
                .canonical_block(block_hash)
                .await
                .map_err(ZebraClaimError::Rpc)?;
            if block.block_hash() != block_hash || block.block_height() != height {
                return Err(ZebraClaimError::BlockInventoryMismatch);
            }
            for transaction_id in block.transaction_ids() {
                let Some(raw_transaction) = self
                    .rpc
                    .block_transaction(*transaction_id, block_hash)
                    .await
                    .map_err(ZebraClaimError::Rpc)?
                else {
                    return Err(ZebraClaimError::BlockInventoryMismatch);
                };
                if self.refund_exact_outpoint_spender(context, *transaction_id, &raw_transaction)? {
                    push_discovered::<R::Error, S::Error>(
                        &mut candidates,
                        DiscoveredSpend::Confirmed {
                            transaction_id: *transaction_id,
                            raw_transaction,
                            block_hash,
                            block_height: height,
                        },
                    )?;
                }
            }
        }
        Ok(candidates)
    }

    fn refund_exact_outpoint_spender(
        &self,
        context: &ZcashFundingContextV1,
        expected_id: TxId,
        raw_transaction: &[u8],
    ) -> Result<bool, ZebraRefundError<R::Error, S::Error>> {
        let transaction = decode_transaction::<R::Error, S::Error>(
            raw_transaction,
            self.identity.consensus_branch_id(),
        )?;
        if transaction.txid() != expected_id {
            return Err(ZebraClaimError::DiscoveredTransactionIdMismatch);
        }
        Ok(transaction.transparent_bundle().is_some_and(|bundle| {
            bundle
                .vin
                .iter()
                .any(|input| input.prevout() == context.funding_outpoint())
        }))
    }

    fn refund_validate_discovered_policy(
        &self,
        agreement: &ZecAgreementV1,
        outpoint: &OutPoint,
        transaction_id: TxId,
        raw_transaction: Vec<u8>,
    ) -> Result<(), ZebraRefundError<R::Error, S::Error>> {
        let dummy_hash = BlockHash([1; 32]);
        let dummy_tip = ZebraChainInfo::new(
            self.identity.rpc_chain(),
            BlockHeight::from_u32(1),
            dummy_hash,
            self.identity.consensus_branch_id(),
        );
        self.refund_validate_snapshot(
            agreement,
            outpoint,
            transaction_id,
            raw_transaction,
            dummy_hash,
            dummy_hash,
            BlockHeight::from_u32(1),
            dummy_tip,
            dummy_tip,
            1,
            true,
        )?;
        Ok(())
    }

    fn refund_validate_agreement_identity(
        &self,
        agreement: &ZecAgreementV1,
    ) -> Result<(), ZebraRefundError<R::Error, S::Error>> {
        let expected = agreement.binding().expected_output();
        if self.identity.network() != expected.network() {
            return Err(ZebraClaimError::ConfiguredNetworkMismatch);
        }
        if self.identity.consensus_branch_id() != expected.consensus_branch_id() {
            return Err(ZebraClaimError::ConfiguredConsensusBranchMismatch);
        }
        Ok(())
    }

    async fn refund_sample_validated_tip(
        &self,
    ) -> Result<ZebraChainInfo, ZebraRefundError<R::Error, S::Error>> {
        let info = self.rpc.chain_info().await.map_err(ZebraClaimError::Rpc)?;
        if info.rpc_chain() != self.identity.rpc_chain() {
            return Err(ZebraClaimError::RpcChainMismatch);
        }
        if info.consensus_branch_id() != self.identity.consensus_branch_id() {
            return Err(ZebraClaimError::RpcConsensusBranchMismatch);
        }
        Ok(info)
    }

    async fn refund_validate_genesis(&self) -> Result<(), ZebraRefundError<R::Error, S::Error>> {
        let genesis = self
            .rpc
            .block_hash(BlockHeight::from_u32(0))
            .await
            .map_err(ZebraClaimError::Rpc)?;
        if genesis != self.identity.genesis_hash() {
            return Err(ZebraClaimError::GenesisMismatch);
        }
        Ok(())
    }
}

impl<R, S> ZebraRpcClaimPort<R, S>
where
    R: ZebraRpc,
    S: ZebraClaimSigner,
{
    async fn discover_counterparty_claim(
        &self,
        agreement: &ZecAgreementV1,
        context: &ZcashClaimContextV1,
    ) -> Result<FollowupClaimObservationV1, ZebraClaimError<R::Error, S::Error>> {
        let Some(anchor) = self.validated_discovery_anchor(agreement, context).await? else {
            return Ok(FollowupClaimObservationV1::Unstable);
        };
        if let Some(unspent) = self
            .rpc
            .unspent_output(context.funding_outpoint())
            .await
            .map_err(ZebraClaimError::Rpc)?
        {
            let after = self.sample_validated_tip().await?;
            if !same_tip(anchor.before, after) {
                return Ok(FollowupClaimObservationV1::Unstable);
            }
            validate_unspent_binding::<R::Error, S::Error>(
                &unspent,
                &anchor.canonical_funding,
                anchor.before,
            )?;
            return Ok(FollowupClaimObservationV1::Absent);
        }

        let candidates = self
            .scan_discovered_spends(context, anchor.before, anchor.funding_height)
            .await?;
        let after = self.sample_validated_tip().await?;
        if !same_tip(anchor.before, after) {
            return Ok(FollowupClaimObservationV1::Unstable);
        }
        self.resolve_discovered_spend(agreement, context, anchor.before, after, candidates)
    }

    async fn validated_discovery_anchor(
        &self,
        agreement: &ZecAgreementV1,
        context: &ZcashClaimContextV1,
    ) -> Result<Option<DiscoveryAnchor>, ZebraClaimError<R::Error, S::Error>> {
        self.validate_agreement_identity(agreement)?;
        let before = self.sample_validated_tip().await?;
        self.validate_genesis().await?;
        let funding_id = TxId::from_bytes(*context.funding_transaction_id_bytes());
        let Some(ZebraTransactionState::Confirmed {
            raw_transaction: funding_raw,
            block_hash: funding_block_hash,
            block_height: funding_height,
            confirmations: funding_confirmations,
            in_active_chain: true,
        }) = self
            .rpc
            .transaction_state(funding_id)
            .await
            .map_err(ZebraClaimError::Rpc)?
        else {
            return Ok(None);
        };
        let canonical_funding_block = self
            .rpc
            .block_hash(funding_height)
            .await
            .map_err(ZebraClaimError::Rpc)?;
        if canonical_funding_block != funding_block_hash {
            return Ok(None);
        }
        let funding_snapshot = ZcashNodeSnapshot::new(
            self.identity.network(),
            self.identity.consensus_branch_id(),
            true,
            funding_block_hash,
            canonical_funding_block,
            funding_height,
            stable_tip(before, before),
            funding_id,
            funding_raw,
            context.funding_output_index(),
            funding_confirmations,
        );
        let canonical_funding = CanonicalZcashOutputObservation::validate(
            agreement.binding().expected_output(),
            &funding_snapshot,
        )
        .map_err(ZebraClaimError::FundingObservation)?;
        if canonical_funding.outpoint() != context.funding_outpoint() {
            return Err(ZebraClaimError::FundingOutpointMismatch);
        }
        Ok(Some(DiscoveryAnchor {
            before,
            funding_height,
            canonical_funding,
        }))
    }

    async fn scan_discovered_spends(
        &self,
        context: &ZcashClaimContextV1,
        before: ZebraChainInfo,
        funding_height: BlockHeight,
    ) -> Result<Vec<DiscoveredSpend>, ZebraClaimError<R::Error, S::Error>> {
        let mut candidates = Vec::new();
        for transaction_id in self
            .rpc
            .mempool_transaction_ids()
            .await
            .map_err(ZebraClaimError::Rpc)?
        {
            let Some(raw_transaction) = self
                .rpc
                .raw_transaction(transaction_id)
                .await
                .map_err(ZebraClaimError::Rpc)?
            else {
                continue;
            };
            if self.exact_outpoint_spender(context, transaction_id, &raw_transaction)? {
                push_discovered::<R::Error, S::Error>(
                    &mut candidates,
                    DiscoveredSpend::Mempool {
                        transaction_id,
                        raw_transaction,
                    },
                )?;
            }
        }

        let tip_height = u32::from(before.tip_height());
        let funding_height_u32 = u32::from(funding_height);
        if funding_height_u32 > tip_height {
            return Ok(candidates);
        }
        let lower_bound = tip_height
            .saturating_add(1)
            .saturating_sub(self.counterparty_scan_blocks)
            .max(funding_height_u32);
        for height in lower_bound..=tip_height {
            let height = BlockHeight::from_u32(height);
            let block_hash = self
                .rpc
                .block_hash(height)
                .await
                .map_err(ZebraClaimError::Rpc)?;
            let block = self
                .rpc
                .canonical_block(block_hash)
                .await
                .map_err(ZebraClaimError::Rpc)?;
            if block.block_hash() != block_hash || block.block_height() != height {
                return Err(ZebraClaimError::BlockInventoryMismatch);
            }
            for transaction_id in block.transaction_ids() {
                let Some(raw_transaction) = self
                    .rpc
                    .block_transaction(*transaction_id, block_hash)
                    .await
                    .map_err(ZebraClaimError::Rpc)?
                else {
                    return Err(ZebraClaimError::BlockInventoryMismatch);
                };
                if self.exact_outpoint_spender(context, *transaction_id, &raw_transaction)? {
                    push_discovered::<R::Error, S::Error>(
                        &mut candidates,
                        DiscoveredSpend::Confirmed {
                            transaction_id: *transaction_id,
                            raw_transaction,
                            block_hash,
                            block_height: height,
                        },
                    )?;
                }
            }
        }
        Ok(candidates)
    }

    fn resolve_discovered_spend(
        &self,
        agreement: &ZecAgreementV1,
        context: &ZcashClaimContextV1,
        before: ZebraChainInfo,
        after: ZebraChainInfo,
        candidates: Vec<DiscoveredSpend>,
    ) -> Result<FollowupClaimObservationV1, ZebraClaimError<R::Error, S::Error>> {
        let Some(candidate) = candidates.into_iter().next() else {
            // `gettxout` proved the output missing, but a bounded scan did not identify
            // its spender. This can be a mempool race or an exhausted historical horizon.
            return Ok(FollowupClaimObservationV1::Unstable);
        };
        match candidate {
            DiscoveredSpend::Mempool {
                transaction_id,
                raw_transaction,
            } => {
                self.validate_discovered_policy(
                    agreement,
                    context.funding_outpoint(),
                    transaction_id,
                    raw_transaction,
                )?;
                Ok(FollowupClaimObservationV1::Unstable)
            }
            DiscoveredSpend::Confirmed {
                transaction_id,
                raw_transaction,
                block_hash,
                block_height,
            } => {
                let confirmations = u32::from(before.tip_height())
                    .checked_sub(u32::from(block_height))
                    .and_then(|distance| distance.checked_add(1))
                    .ok_or(ZebraClaimError::BlockInventoryMismatch)?;
                let canonical = self.validate_claim_snapshot(
                    agreement,
                    context.funding_outpoint(),
                    transaction_id,
                    raw_transaction,
                    block_hash,
                    block_hash,
                    block_height,
                    before,
                    after,
                    confirmations,
                    true,
                )?;
                let evidence = FollowupClaimEvidenceV1::new(
                    agreement,
                    *canonical.transaction_id().as_ref(),
                    canonical.transaction_id().to_string(),
                    canonical.confirmations().get(),
                )
                .map_err(ZebraClaimError::Claim)?;
                Ok(FollowupClaimObservationV1::Confirmed(evidence))
            }
        }
    }

    fn exact_outpoint_spender(
        &self,
        context: &ZcashClaimContextV1,
        expected_id: TxId,
        raw_transaction: &[u8],
    ) -> Result<bool, ZebraClaimError<R::Error, S::Error>> {
        let transaction = decode_transaction::<R::Error, S::Error>(
            raw_transaction,
            self.identity.consensus_branch_id(),
        )?;
        if transaction.txid() != expected_id {
            return Err(ZebraClaimError::DiscoveredTransactionIdMismatch);
        }
        Ok(transaction.transparent_bundle().is_some_and(|bundle| {
            bundle
                .vin
                .iter()
                .any(|input| input.prevout() == context.funding_outpoint())
        }))
    }

    fn validate_discovered_policy(
        &self,
        agreement: &ZecAgreementV1,
        outpoint: &OutPoint,
        transaction_id: TxId,
        raw_transaction: Vec<u8>,
    ) -> Result<(), ZebraClaimError<R::Error, S::Error>> {
        let dummy_hash = BlockHash([1; 32]);
        let dummy_tip = ZebraChainInfo::new(
            self.identity.rpc_chain(),
            BlockHeight::from_u32(1),
            dummy_hash,
            self.identity.consensus_branch_id(),
        );
        self.validate_claim_snapshot(
            agreement,
            outpoint,
            transaction_id,
            raw_transaction,
            dummy_hash,
            dummy_hash,
            BlockHeight::from_u32(1),
            dummy_tip,
            dummy_tip,
            1,
            true,
        )?;
        Ok(())
    }

    async fn observe_funding(
        &self,
        agreement: &ZecAgreementV1,
        context: &ZcashFundingContextV1,
    ) -> Result<ZcashFundingObservationV1, ZebraClaimError<R::Error, S::Error>> {
        self.observe_funding_target(agreement, &FundingTarget::from_context(context))
            .await
    }

    async fn observe_funding_target(
        &self,
        agreement: &ZecAgreementV1,
        target: &FundingTarget,
    ) -> Result<ZcashFundingObservationV1, ZebraClaimError<R::Error, S::Error>> {
        self.validate_agreement_identity(agreement)?;
        let transaction_id = target.transaction_id;
        let before = self.sample_validated_tip().await?;
        self.validate_genesis().await?;
        let state = self
            .rpc
            .transaction_state(transaction_id)
            .await
            .map_err(ZebraClaimError::Rpc)?;
        let canonical_block = match &state {
            Some(ZebraTransactionState::Confirmed { block_height, .. }) => Some(
                self.rpc
                    .block_hash(*block_height)
                    .await
                    .map_err(ZebraClaimError::Rpc)?,
            ),
            Some(ZebraTransactionState::Mempool { .. }) | None => None,
        };
        let unspent = match &state {
            Some(ZebraTransactionState::Confirmed { .. }) => self
                .rpc
                .unspent_output(&target.outpoint)
                .await
                .map_err(ZebraClaimError::Rpc)?,
            Some(ZebraTransactionState::Mempool { .. }) | None => None,
        };
        let after = self.sample_validated_tip().await?;
        if !same_tip(before, after) {
            return Ok(ZcashFundingObservationV1::Unstable);
        }
        match state {
            None => Ok(ZcashFundingObservationV1::Absent),
            Some(ZebraTransactionState::Mempool { .. }) => Ok(ZcashFundingObservationV1::Unstable),
            Some(ZebraTransactionState::Confirmed {
                raw_transaction,
                block_hash,
                block_height,
                confirmations,
                in_active_chain,
            }) => {
                if !in_active_chain
                    || canonical_block.expect("confirmed state resolves a block") != block_hash
                {
                    return Ok(ZcashFundingObservationV1::Unstable);
                }
                let snapshot = ZcashNodeSnapshot::new(
                    self.identity.network(),
                    self.identity.consensus_branch_id(),
                    true,
                    block_hash,
                    block_hash,
                    block_height,
                    stable_tip(before, after),
                    transaction_id,
                    raw_transaction,
                    target.output_index,
                    confirmations,
                );
                let canonical = CanonicalZcashOutputObservation::validate(
                    agreement.binding().expected_output(),
                    &snapshot,
                )
                .map_err(ZebraClaimError::FundingObservation)?;
                if canonical.transaction_id() != target.transaction_id
                    || canonical.outpoint() != &target.outpoint
                {
                    return Err(ZebraClaimError::FundingOutpointMismatch);
                }
                let Some(unspent) = unspent else {
                    return Ok(ZcashFundingObservationV1::Spent);
                };
                validate_unspent_binding::<R::Error, S::Error>(&unspent, &canonical, before)?;
                let unspent = ZcashUnspentOutputSnapshotV1::new(
                    target.outpoint.clone(),
                    unspent.output().clone(),
                    stable_tip(before, after),
                );
                Ok(ZcashFundingObservationV1::confirmed(canonical, unspent))
            }
        }
    }

    fn validate_prepared(
        &self,
        agreement: &ZecAgreementV1,
        context: &ZcashClaimContextV1,
        prepared: &PreparedClaimSubmissionV1,
    ) -> Result<TxId, ZebraClaimError<R::Error, S::Error>> {
        self.validate_agreement_identity(agreement)?;
        if prepared.step() != ClaimStepV1::FollowupZcash {
            return Err(ZebraClaimError::WrongStep(prepared.step()));
        }
        let transaction_id = decode_transaction_id::<R::Error, S::Error>(
            prepared.exact_submission(),
            self.identity.consensus_branch_id(),
        )?;
        if transaction_id.as_ref() != prepared.expected_submission_id() {
            return Err(ZebraClaimError::ExpectedTransactionIdMismatch);
        }
        let transaction = decode_transaction::<R::Error, S::Error>(
            prepared.exact_submission(),
            self.identity.consensus_branch_id(),
        )?;
        let prepared_height = u32::from(transaction.expiry_height())
            .checked_sub(agreement.transaction_policy().expiry_delta_blocks())
            .map(BlockHeight::from_u32)
            .ok_or(ZebraClaimError::ClaimExpiryPolicyMismatch)?;
        let expected_output = agreement.binding().expected_output();
        let funding_output = TxOut::new(
            expected_output.value(),
            Script(Code(
                expected_output.contract().p2sh_script_pubkey().to_vec(),
            )),
        );
        let request = agreement
            .claim_spend_request(
                context.funding_outpoint().clone(),
                funding_output,
                prepared_height,
            )
            .map_err(ZebraClaimError::Agreement)?;
        let expected = ExpectedBip199Spend::from_request(
            self.identity.network(),
            expected_output.contract().clone(),
            &request,
        )
        .map_err(ZebraClaimError::SpendObservation)?;
        let dummy_hash = BlockHash([1; 32]);
        let snapshot = ZcashSpendNodeSnapshot::new(
            self.identity.network(),
            self.identity.consensus_branch_id(),
            true,
            dummy_hash,
            dummy_hash,
            BlockHeight::from_u32(1),
            ZcashStableTip::new(
                dummy_hash,
                BlockHeight::from_u32(1),
                dummy_hash,
                BlockHeight::from_u32(1),
            ),
            transaction_id,
            prepared.exact_submission().to_vec(),
            1,
        );
        let canonical = CanonicalZcashSpendObservation::validate(&expected, &snapshot)
            .map_err(ZebraClaimError::SpendObservation)?;
        if !matches!(canonical.kind(), Bip199SpendKind::Claim { .. }) {
            return Err(ZebraClaimError::WrongSpendBranch);
        }
        if !canonical.sdk_canonical_policy().is_compliant() {
            return Err(ZebraClaimError::NonCanonicalClaimPolicy);
        }
        let bundle = transaction
            .transparent_bundle()
            .ok_or(ZebraClaimError::FundingOutpointMismatch)?;
        if bundle.vin.len() != 1
            || bundle.vout.len() != 1
            || bundle.vin[0].prevout() != context.funding_outpoint()
        {
            return Err(ZebraClaimError::FundingOutpointMismatch);
        }
        Ok(transaction_id)
    }

    #[allow(clippy::too_many_arguments)]
    fn validate_claim_snapshot(
        &self,
        agreement: &ZecAgreementV1,
        outpoint: &OutPoint,
        transaction_id: TxId,
        raw_transaction: Vec<u8>,
        block_hash: BlockHash,
        canonical_block_hash: BlockHash,
        block_height: BlockHeight,
        before: ZebraChainInfo,
        after: ZebraChainInfo,
        confirmations: u32,
        in_active_chain: bool,
    ) -> Result<CanonicalZcashSpendObservation, ZebraClaimError<R::Error, S::Error>> {
        let expected = expected_spend(agreement, outpoint.clone(), &raw_transaction)?;
        let snapshot = ZcashSpendNodeSnapshot::new(
            self.identity.network(),
            self.identity.consensus_branch_id(),
            in_active_chain,
            block_hash,
            canonical_block_hash,
            block_height,
            stable_tip(before, after),
            transaction_id,
            raw_transaction,
            confirmations,
        );
        let canonical = CanonicalZcashSpendObservation::validate(&expected, &snapshot)
            .map_err(ZebraClaimError::SpendObservation)?;
        if !matches!(canonical.kind(), Bip199SpendKind::Claim { .. }) {
            return Err(ZebraClaimError::WrongSpendBranch);
        }
        if !canonical.sdk_canonical_policy().is_compliant() {
            return Err(ZebraClaimError::NonCanonicalClaimPolicy);
        }
        Ok(canonical)
    }

    fn validate_agreement_identity(
        &self,
        agreement: &ZecAgreementV1,
    ) -> Result<(), ZebraClaimError<R::Error, S::Error>> {
        let expected = agreement.binding().expected_output();
        if self.identity.network() != expected.network() {
            return Err(ZebraClaimError::ConfiguredNetworkMismatch);
        }
        if self.identity.consensus_branch_id() != expected.consensus_branch_id() {
            return Err(ZebraClaimError::ConfiguredConsensusBranchMismatch);
        }
        Ok(())
    }

    async fn sample_validated_tip(
        &self,
    ) -> Result<ZebraChainInfo, ZebraClaimError<R::Error, S::Error>> {
        let info = self.rpc.chain_info().await.map_err(ZebraClaimError::Rpc)?;
        if info.rpc_chain() != self.identity.rpc_chain() {
            return Err(ZebraClaimError::RpcChainMismatch);
        }
        if info.consensus_branch_id() != self.identity.consensus_branch_id() {
            return Err(ZebraClaimError::RpcConsensusBranchMismatch);
        }
        Ok(info)
    }

    async fn validate_genesis(&self) -> Result<(), ZebraClaimError<R::Error, S::Error>> {
        let genesis = self
            .rpc
            .block_hash(BlockHeight::from_u32(0))
            .await
            .map_err(ZebraClaimError::Rpc)?;
        if genesis != self.identity.genesis_hash() {
            return Err(ZebraClaimError::GenesisMismatch);
        }
        Ok(())
    }
}

fn push_discovered<RE, SE>(
    candidates: &mut Vec<DiscoveredSpend>,
    candidate: DiscoveredSpend,
) -> Result<(), ZebraClaimError<RE, SE>>
where
    RE: std::error::Error + Send + Sync + 'static,
    SE: std::error::Error + Send + Sync + 'static,
{
    if let Some(existing) = candidates.first() {
        if existing.transaction_id() != candidate.transaction_id()
            || existing.raw_transaction() != candidate.raw_transaction()
        {
            return Err(ZebraClaimError::ConflictingSpendCandidates);
        }
        if matches!(existing, DiscoveredSpend::Mempool { .. })
            && matches!(candidate, DiscoveredSpend::Confirmed { .. })
        {
            candidates[0] = candidate;
        }
    } else {
        candidates.push(candidate);
    }
    Ok(())
}

fn expected_spend<RE, SE>(
    agreement: &ZecAgreementV1,
    outpoint: OutPoint,
    raw_transaction: &[u8],
) -> Result<ExpectedBip199Spend, ZebraClaimError<RE, SE>>
where
    RE: std::error::Error + Send + Sync + 'static,
    SE: std::error::Error + Send + Sync + 'static,
{
    let output = agreement.binding().expected_output();
    let funding = TxOut::new(
        output.value(),
        Script(Code(output.contract().p2sh_script_pubkey().to_vec())),
    );
    let transaction = decode_transaction::<RE, SE>(raw_transaction, output.consensus_branch_id())?;
    let prepared_height = u32::from(transaction.expiry_height())
        .checked_sub(agreement.transaction_policy().expiry_delta_blocks())
        .map(BlockHeight::from_u32)
        .ok_or(ZebraClaimError::ClaimExpiryPolicyMismatch)?;
    let request = agreement
        .claim_spend_request(outpoint, funding, prepared_height)
        .map_err(ZebraClaimError::Agreement)?;
    ExpectedBip199Spend::from_request(output.network(), output.contract().clone(), &request)
        .map_err(ZebraClaimError::SpendObservation)
}

fn expected_refund_spend<RE, SE>(
    agreement: &ZecAgreementV1,
    outpoint: OutPoint,
    raw_transaction: &[u8],
) -> Result<ExpectedBip199Spend, ZebraRefundError<RE, SE>>
where
    RE: std::error::Error + Send + Sync + 'static,
    SE: std::error::Error + Send + Sync + 'static,
{
    let output = agreement.binding().expected_output();
    let funding = TxOut::new(
        output.value(),
        Script(Code(output.contract().p2sh_script_pubkey().to_vec())),
    );
    let transaction = decode_transaction::<RE, SE>(raw_transaction, output.consensus_branch_id())?;
    let prepared_height = u32::from(transaction.expiry_height())
        .checked_sub(agreement.transaction_policy().expiry_delta_blocks())
        .map(BlockHeight::from_u32)
        .ok_or(ZebraClaimError::RefundExpiryPolicyMismatch)?;
    let request = agreement
        .refund_spend_request(outpoint, funding, prepared_height)
        .map_err(ZebraClaimError::Agreement)?;
    ExpectedBip199Spend::from_request(output.network(), output.contract().clone(), &request)
        .map_err(ZebraClaimError::SpendObservation)
}

fn validate_unspent_binding<RE, SE>(
    unspent: &ZebraUnspentOutput,
    canonical: &CanonicalZcashOutputObservation,
    tip: ZebraChainInfo,
) -> Result<(), ZebraClaimError<RE, SE>>
where
    RE: std::error::Error + Send + Sync + 'static,
    SE: std::error::Error + Send + Sync + 'static,
{
    if unspent.best_block() != tip.tip_hash() {
        return Err(ZebraClaimError::UtxoTipMismatch);
    }
    if unspent.confirmations() != canonical.confirmations().get() {
        return Err(ZebraClaimError::UtxoConfirmationMismatch);
    }
    if unspent.output() != canonical.output() {
        return Err(ZebraClaimError::FundingOutpointMismatch);
    }
    Ok(())
}

fn decode_transaction_id<RE, SE>(
    raw: &[u8],
    branch: zcash_protocol::consensus::BranchId,
) -> Result<TxId, ZebraClaimError<RE, SE>>
where
    RE: std::error::Error + Send + Sync + 'static,
    SE: std::error::Error + Send + Sync + 'static,
{
    decode_transaction::<RE, SE>(raw, branch).map(|transaction| transaction.txid())
}

fn decode_transaction<RE, SE>(
    raw: &[u8],
    branch: zcash_protocol::consensus::BranchId,
) -> Result<Transaction, ZebraClaimError<RE, SE>>
where
    RE: std::error::Error + Send + Sync + 'static,
    SE: std::error::Error + Send + Sync + 'static,
{
    let mut cursor = Cursor::new(raw);
    let transaction =
        Transaction::read(&mut cursor, branch).map_err(ZebraClaimError::MalformedSubmission)?;
    let exact_length =
        u64::try_from(raw.len()).map_err(|_| ZebraClaimError::TrailingSubmissionBytes)?;
    if cursor.position() != exact_length {
        return Err(ZebraClaimError::TrailingSubmissionBytes);
    }
    if transaction.version() != TxVersion::V5 {
        return Err(ZebraClaimError::WrongTransactionVersion);
    }
    Ok(transaction)
}

fn require_exact_claim<RE, SE>(
    actual: &[u8],
    expected: &[u8],
) -> Result<(), ZebraClaimError<RE, SE>>
where
    RE: std::error::Error + Send + Sync + 'static,
    SE: std::error::Error + Send + Sync + 'static,
{
    if actual == expected {
        Ok(())
    } else {
        Err(ZebraClaimError::ObservedClaimBytesMismatch)
    }
}

fn require_exact_refund<RE, SE>(
    actual: &[u8],
    expected: &[u8],
) -> Result<(), ZebraRefundError<RE, SE>>
where
    RE: std::error::Error + Send + Sync + 'static,
    SE: std::error::Error + Send + Sync + 'static,
{
    if actual == expected {
        Ok(())
    } else {
        Err(ZebraClaimError::ObservedRefundBytesMismatch)
    }
}

fn stable_tip(before: ZebraChainInfo, after: ZebraChainInfo) -> ZcashStableTip {
    ZcashStableTip::new(
        before.tip_hash(),
        before.tip_height(),
        after.tip_hash(),
        after.tip_height(),
    )
}

fn same_tip(before: ZebraChainInfo, after: ZebraChainInfo) -> bool {
    before.tip_hash() == after.tip_hash() && before.tip_height() == after.tip_height()
}

#[cfg(test)]
mod tests {
    use std::{
        collections::VecDeque,
        sync::{Arc, Mutex},
    };

    use async_trait::async_trait;
    use lez_swap_core::{Participant, SwapDirection, UnixSeconds};
    use lez_swap_store::SqliteZecRecoveryStore;
    use lez_zec_swap_sdk::{
        FirstLockConfirmedEvidenceV1, FirstLockPlanV1, FirstLockStepV1, LezAssetV1,
        LezChainIdentityV1, LezClaimInstructionV1, LezClaimNodeSnapshotV1, LezClaimPort,
        LezClaimTransactionSnapshotV1, LezCustodySnapshotV1, LezEnvironmentV1,
        LezEscrowMetadataSnapshotV1, LezEscrowStatusV1, LezInclusionStatusV1,
        LezMakerLockObservationPort, LezStableTipV1, MakerLockObservationV1, NegotiationChannel,
        NegotiationTranscriptV1, OfferDiscovery, PreparedFirstLockSubmissionV1, ProtectedClaimKey,
        RevealingClaimEvidenceV1, RevealingClaimObservationV1, TransparentFundingRequest,
        TransparentUtxo, ZEC_CONCRETE_AGREEMENT_SCHEMA_V2, ZcashMakerLockObservationPort,
        ZcashRefundPort, ZcashTransparentDestinationV1, ZecAgreementBodyV1, ZecAgreementRecordV1,
        ZecLezTermsV1, ZecPairSdk, ZecParticipantIdentityV1, ZecParticipantsV1, ZecProfileId,
        ZecProfileRecordV1, ZecRefundPlanV1, ZecSwapBinding, ZecSwapBindingRecordV1,
        ZecTransactionPolicyV1, build_claim_transaction, build_funding_transaction,
        build_refund_transaction, derive_lez_metadata_account_v1,
        derive_lez_native_custody_account_v1, derive_lez_swap_id_v1,
    };
    use secp256k1::{Message, PublicKey, Secp256k1, SecretKey};
    use sha2::{Digest as _, Sha256};
    use tempfile::TempDir;
    use zcash_primitives::transaction::Transaction;
    use zcash_protocol::{
        consensus::{BlockHeight, BranchId, NetworkType},
        value::Zatoshis,
    };
    use zcash_transparent::{
        address::{Script, TransparentAddress},
        bundle::{OutPoint, TxOut},
    };

    use super::*;
    use crate::rpc::{ZebraCanonicalBlock, ZebraRpcChain};

    const INCLUSION_HEIGHT: u32 = 100;
    const TIP_HEIGHT: u32 = 104;

    #[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
    enum FakeError {
        #[error("fake Zebra RPC error {0}")]
        RpcCode(i32),
        #[error("fake Zebra transport failure")]
        Transport,
    }

    #[derive(Debug, thiserror::Error)]
    enum SignerError {
        #[error(transparent)]
        Build(#[from] lez_zec_swap_sdk::TransactionBuildError),
        #[error("canonical transaction serialization failed: {0}")]
        Serialization(#[from] std::io::Error),
    }

    #[derive(Clone, Debug)]
    struct CanonicalSigner(SecretKey);

    #[async_trait]
    impl ZebraClaimSigner for CanonicalSigner {
        type Error = SignerError;

        async fn sign_claim(
            &self,
            contract: &Bip199Contract,
            request: &TransparentSpendRequest,
            preimage: &ClaimPreimage,
        ) -> Result<Vec<u8>, Self::Error> {
            let transaction =
                build_claim_transaction(contract, request, &self.0, preimage.expose_secret())?;
            let mut exact = Vec::new();
            transaction.write(&mut exact)?;
            Ok(exact)
        }
    }

    #[derive(Clone, Debug)]
    struct NeverSigner;

    #[async_trait]
    impl ZebraClaimSigner for NeverSigner {
        type Error = FakeError;

        async fn sign_claim(
            &self,
            _contract: &Bip199Contract,
            _request: &TransparentSpendRequest,
            _preimage: &ClaimPreimage,
        ) -> Result<Vec<u8>, Self::Error> {
            panic!("funding observation tests never sign")
        }
    }

    #[derive(Clone, Debug)]
    struct NeverRefundSigner;

    #[async_trait]
    impl ZebraRefundSigner for NeverRefundSigner {
        type Error = FakeError;

        fn participant(&self) -> Participant {
            Participant::Taker
        }

        async fn sign_refund(
            &self,
            _contract: &Bip199Contract,
            _request: &TransparentSpendRequest,
        ) -> Result<Vec<u8>, Self::Error> {
            panic!("refund boundary assertion never signs")
        }
    }

    #[derive(Clone, Debug)]
    struct CanonicalRefundSigner {
        participant: Participant,
        key: SecretKey,
    }

    #[async_trait]
    impl ZebraRefundSigner for CanonicalRefundSigner {
        type Error = SignerError;

        fn participant(&self) -> Participant {
            self.participant
        }

        async fn sign_refund(
            &self,
            contract: &Bip199Contract,
            request: &TransparentSpendRequest,
        ) -> Result<Vec<u8>, Self::Error> {
            let transaction = build_refund_transaction(contract, request, &self.key)?;
            let mut exact = Vec::new();
            transaction.write(&mut exact)?;
            Ok(exact)
        }
    }

    #[derive(Clone, Debug)]
    struct FakeRpc {
        state: Arc<Mutex<FakeState>>,
    }

    #[derive(Debug)]
    struct FakeState {
        chain_infos: VecDeque<ZebraChainInfo>,
        chain_info_overrides: VecDeque<Result<ZebraChainInfo, FakeError>>,
        block_hash_overrides: VecDeque<Result<BlockHash, FakeError>>,
        canonical_block_overrides: VecDeque<Result<ZebraCanonicalBlock, FakeError>>,
        block_transaction_overrides: VecDeque<Result<Option<Vec<u8>>, FakeError>>,
        mempool_overrides: VecDeque<Result<Vec<TxId>, FakeError>>,
        raw_transaction_overrides: VecDeque<Result<Option<Vec<u8>>, FakeError>>,
        identity: ZebraChainIdentity,
        canonical_block: BlockHash,
        funding_transaction_id: TxId,
        funding_transaction: ZebraTransactionState,
        discovery_enabled: bool,
        transaction: Option<ZebraTransactionState>,
        unspent: Option<ZebraUnspentOutput>,
        submissions: VecDeque<Result<TxId, FakeError>>,
        submitted: Vec<Vec<u8>>,
        calls: Vec<String>,
    }

    impl FakeRpc {
        fn confirmed(
            _agreement: &ZecAgreementV1,
            transaction: &Transaction,
        ) -> (Self, FundingTarget) {
            let identity = ZebraChainIdentity::deterministic_regtest_nu6_2();
            let tip = ZebraChainInfo::new(
                ZebraRpcChain::Test,
                BlockHeight::from_u32(TIP_HEIGHT),
                BlockHash([0x33; 32]),
                BranchId::Nu6_2,
            );
            let canonical_block = BlockHash([0x22; 32]);
            let mut raw = Vec::new();
            transaction
                .write(&mut raw)
                .expect("canonical funding bytes");
            let output = transaction
                .transparent_bundle()
                .expect("transparent funding")
                .vout[0]
                .clone();
            let transaction_id = transaction.txid();
            let target = FundingTarget {
                transaction_id,
                outpoint: OutPoint::new(*transaction_id.as_ref(), 0),
                output_index: 0,
            };
            (
                Self {
                    state: Arc::new(Mutex::new(FakeState {
                        chain_infos: VecDeque::from([tip, tip]),
                        chain_info_overrides: VecDeque::new(),
                        block_hash_overrides: VecDeque::new(),
                        canonical_block_overrides: VecDeque::new(),
                        block_transaction_overrides: VecDeque::new(),
                        mempool_overrides: VecDeque::new(),
                        raw_transaction_overrides: VecDeque::new(),
                        identity,
                        canonical_block,
                        funding_transaction_id: transaction_id,
                        funding_transaction: ZebraTransactionState::Confirmed {
                            raw_transaction: raw,
                            block_hash: canonical_block,
                            block_height: BlockHeight::from_u32(INCLUSION_HEIGHT),
                            confirmations: TIP_HEIGHT - INCLUSION_HEIGHT + 1,
                            in_active_chain: true,
                        },
                        discovery_enabled: false,
                        transaction: Some(ZebraTransactionState::Confirmed {
                            raw_transaction: {
                                let mut exact = Vec::new();
                                transaction
                                    .write(&mut exact)
                                    .expect("canonical funding bytes");
                                exact
                            },
                            block_hash: canonical_block,
                            block_height: BlockHeight::from_u32(INCLUSION_HEIGHT),
                            confirmations: TIP_HEIGHT - INCLUSION_HEIGHT + 1,
                            in_active_chain: true,
                        }),
                        unspent: Some(ZebraUnspentOutput::new(
                            tip.tip_hash(),
                            TIP_HEIGHT - INCLUSION_HEIGHT + 1,
                            output,
                        )),
                        submissions: VecDeque::new(),
                        submitted: Vec::new(),
                        calls: Vec::new(),
                    })),
                },
                target,
            )
        }

        fn edit(&self, edit: impl FnOnce(&mut FakeState)) {
            edit(&mut self.state.lock().expect("fake state"));
        }

        fn calls(&self) -> Vec<String> {
            self.state.lock().expect("fake state").calls.clone()
        }

        fn submitted(&self) -> Vec<Vec<u8>> {
            self.state.lock().expect("fake state").submitted.clone()
        }
    }

    #[async_trait]
    impl ZebraRpc for FakeRpc {
        type Error = FakeError;

        fn classify_submission_failure(error: &Self::Error) -> ZebraSubmissionFailure {
            match error {
                FakeError::RpcCode(-22 | -25 | -26) => ZebraSubmissionFailure::DefinitiveRejection,
                FakeError::RpcCode(_) | FakeError::Transport => {
                    ZebraSubmissionFailure::UnknownOutcome
                }
            }
        }

        async fn chain_info(&self) -> Result<ZebraChainInfo, Self::Error> {
            let mut state = self.state.lock().expect("fake state");
            state.calls.push("chain_info".to_owned());
            if let Some(override_result) = state.chain_info_overrides.pop_front() {
                return override_result;
            }
            Ok(state.chain_infos.pop_front().unwrap_or_else(canonical_tip))
        }

        async fn block_hash(&self, height: BlockHeight) -> Result<BlockHash, Self::Error> {
            let mut state = self.state.lock().expect("fake state");
            state
                .calls
                .push(format!("block_hash:{}", u32::from(height)));
            if let Some(override_result) = state.block_hash_overrides.pop_front() {
                return override_result;
            }
            Ok(if u32::from(height) == 0 {
                state.identity.genesis_hash()
            } else if u32::from(height) == INCLUSION_HEIGHT {
                state.canonical_block
            } else if u32::from(height) == TIP_HEIGHT {
                canonical_tip().tip_hash()
            } else {
                BlockHash([u32::from(height).to_le_bytes()[0]; 32])
            })
        }

        async fn canonical_block(
            &self,
            block_hash: BlockHash,
        ) -> Result<ZebraCanonicalBlock, Self::Error> {
            let mut state = self.state.lock().expect("fake state");
            state.calls.push("canonical_block".to_owned());
            if let Some(override_result) = state.canonical_block_overrides.pop_front() {
                return override_result;
            }
            let height = if block_hash == state.canonical_block {
                INCLUSION_HEIGHT
            } else if block_hash == canonical_tip().tip_hash() {
                TIP_HEIGHT
            } else {
                u32::from(block_hash.0[0])
            };
            let transaction_ids = match state.transaction.as_ref() {
                Some(
                    transaction @ ZebraTransactionState::Confirmed {
                        block_hash: transaction_block,
                        ..
                    },
                ) if state.discovery_enabled && block_hash == *transaction_block => {
                    fake_state_transaction_id(transaction).into_iter().collect()
                }
                Some(
                    ZebraTransactionState::Confirmed { .. } | ZebraTransactionState::Mempool { .. },
                )
                | None => Vec::new(),
            };
            Ok(ZebraCanonicalBlock::new(
                block_hash,
                BlockHeight::from_u32(height),
                transaction_ids,
            ))
        }

        async fn block_transaction(
            &self,
            transaction_id: TxId,
            _block_hash: BlockHash,
        ) -> Result<Option<Vec<u8>>, Self::Error> {
            let mut state = self.state.lock().expect("fake state");
            state.calls.push("block_transaction".to_owned());
            if let Some(override_result) = state.block_transaction_overrides.pop_front() {
                return override_result;
            }
            Ok(state.transaction.as_ref().and_then(|transaction| {
                (fake_state_transaction_id(transaction) == Some(transaction_id))
                    .then(|| fake_state_transaction_bytes(transaction))
            }))
        }

        async fn mempool_transaction_ids(&self) -> Result<Vec<TxId>, Self::Error> {
            let mut state = self.state.lock().expect("fake state");
            state.calls.push("mempool_transaction_ids".to_owned());
            if let Some(override_result) = state.mempool_overrides.pop_front() {
                return override_result;
            }
            Ok(match state.transaction.as_ref() {
                Some(transaction @ ZebraTransactionState::Mempool { .. }) => {
                    fake_state_transaction_id(transaction).into_iter().collect()
                }
                Some(ZebraTransactionState::Confirmed { .. }) | None => Vec::new(),
            })
        }

        async fn raw_transaction(
            &self,
            transaction_id: TxId,
        ) -> Result<Option<Vec<u8>>, Self::Error> {
            let mut state = self.state.lock().expect("fake state");
            state.calls.push("raw_transaction".to_owned());
            if let Some(override_result) = state.raw_transaction_overrides.pop_front() {
                return override_result;
            }
            Ok(state.transaction.as_ref().and_then(|transaction| {
                (fake_state_transaction_id(transaction) == Some(transaction_id))
                    .then(|| fake_state_transaction_bytes(transaction))
            }))
        }

        async fn transaction_state(
            &self,
            transaction_id: TxId,
        ) -> Result<Option<ZebraTransactionState>, Self::Error> {
            let mut state = self.state.lock().expect("fake state");
            state.calls.push("transaction_state".to_owned());
            if state.discovery_enabled && transaction_id == state.funding_transaction_id {
                Ok(Some(state.funding_transaction.clone()))
            } else {
                Ok(state.transaction.clone())
            }
        }

        async fn unspent_output(
            &self,
            _outpoint: &OutPoint,
        ) -> Result<Option<ZebraUnspentOutput>, Self::Error> {
            let mut state = self.state.lock().expect("fake state");
            state.calls.push("unspent_output".to_owned());
            Ok(state.unspent.clone())
        }

        async fn send_raw_transaction(&self, transaction: &[u8]) -> Result<TxId, Self::Error> {
            let mut state = self.state.lock().expect("fake state");
            state.calls.push("send_raw_transaction".to_owned());
            state.submitted.push(transaction.to_vec());
            state
                .submissions
                .pop_front()
                .expect("submission result configured")
        }
    }

    fn fake_state_transaction_id(state: &ZebraTransactionState) -> Option<TxId> {
        let raw = fake_state_transaction_bytes(state);
        Transaction::read(&mut Cursor::new(raw), BranchId::Nu6_2)
            .ok()
            .map(|transaction| transaction.txid())
    }

    fn fake_state_transaction_bytes(state: &ZebraTransactionState) -> Vec<u8> {
        match state {
            ZebraTransactionState::Mempool { raw_transaction }
            | ZebraTransactionState::Confirmed {
                raw_transaction, ..
            } => raw_transaction.clone(),
        }
    }

    #[derive(Clone, Debug)]
    struct FixedDiscovery;

    #[async_trait]
    impl OfferDiscovery for FixedDiscovery {
        type Error = FakeError;
        type Offer = ();
        type OfferRef = ();
        type Query = ();

        async fn publish(&self, offer: Self::Offer) -> Result<Self::OfferRef, Self::Error> {
            Ok(offer)
        }

        async fn discover(&self, _query: &Self::Query) -> Result<Vec<Self::OfferRef>, Self::Error> {
            Ok(vec![()])
        }
    }

    #[derive(Clone, Debug)]
    struct FixedNegotiation(Vec<u8>);

    #[async_trait]
    impl NegotiationChannel for FixedNegotiation {
        type Error = FakeError;
        type LocalProposal = ();
        type OfferRef = ();

        async fn negotiate(
            &self,
            _local_participant: Participant,
            _offer: &Self::OfferRef,
            _proposal: Self::LocalProposal,
        ) -> Result<Vec<u8>, Self::Error> {
            Ok(self.0.clone())
        }
    }

    #[derive(Clone, Debug)]
    struct ClaimLifecycleLez {
        secret: [u8; 32],
    }

    #[async_trait]
    impl LezMakerLockObservationPort for ClaimLifecycleLez {
        type Error = FakeError;

        async fn observe_maker_lock(
            &self,
            _agreement: &ZecAgreementV1,
        ) -> Result<MakerLockObservationV1, Self::Error> {
            Ok(MakerLockObservationV1::Confirmed(
                FirstLockConfirmedEvidenceV1::new(
                    FirstLockStepV1::LezFund,
                    [0x42; 32],
                    hex::encode([0x42; 32]),
                    100,
                )
                .expect("canonical maker lock evidence"),
            ))
        }
    }

    #[async_trait]
    impl LezClaimPort for ClaimLifecycleLez {
        type Error = FakeError;

        async fn prepare_revealing_claim(
            &self,
            _agreement: &ZecAgreementV1,
            preimage: &ClaimPreimage,
        ) -> Result<PreparedClaimSubmissionV1, Self::Error> {
            assert_eq!(preimage.expose_secret(), &self.secret);
            let mut exact = b"canonical-lez-claim".to_vec();
            exact.extend_from_slice(preimage.expose_secret());
            Ok(
                PreparedClaimSubmissionV1::new(ClaimStepV1::RevealingLez, [0xc1; 32], exact)
                    .expect("valid revealing claim"),
            )
        }

        async fn observe_prepared_revealing_claim(
            &self,
            agreement: &ZecAgreementV1,
            prepared: &PreparedClaimSubmissionV1,
        ) -> Result<RevealingClaimObservationV1, Self::Error> {
            Ok(RevealingClaimObservationV1::Confirmed(
                RevealingClaimEvidenceV1::from_prepared_lez_claim_snapshot(
                    agreement,
                    prepared,
                    canonical_lez_claim_snapshot(agreement, self.secret),
                )
                .expect("canonical revealing claim evidence"),
            ))
        }

        async fn observe_counterparty_revealing_claim(
            &self,
            _agreement: &ZecAgreementV1,
        ) -> Result<RevealingClaimObservationV1, Self::Error> {
            panic!("claim-context fixture owns the revealing claim")
        }

        async fn submit_revealing_claim(
            &self,
            _agreement: &ZecAgreementV1,
            _prepared: &PreparedClaimSubmissionV1,
        ) -> Result<(), Self::Error> {
            panic!("already-confirmed revealing claim is never submitted")
        }
    }

    #[derive(Clone, Debug)]
    struct NeverZcash;

    #[async_trait]
    impl ZcashMakerLockObservationPort for NeverZcash {
        type Error = FakeError;

        async fn observe_maker_lock(
            &self,
            _agreement: &ZecAgreementV1,
        ) -> Result<MakerLockObservationV1, Self::Error> {
            panic!("forward fixture observes the maker lock on LEZ")
        }
    }

    #[async_trait]
    impl ZcashClaimPort for NeverZcash {
        type Error = FakeError;

        async fn observe_funding_before_reveal(
            &self,
            _agreement: &ZecAgreementV1,
            _context: &ZcashFundingContextV1,
        ) -> Result<ZcashFundingObservationV1, Self::Error> {
            panic!("already-confirmed revealing claim needs no funding recheck")
        }

        async fn prepare_followup_claim(
            &self,
            _agreement: &ZecAgreementV1,
            _context: &ZcashClaimContextV1,
            _preimage: &ClaimPreimage,
        ) -> Result<PreparedClaimSubmissionV1, Self::Error> {
            panic!("claim-context fixture stops before follow-up preparation")
        }

        async fn observe_prepared_followup_claim(
            &self,
            _agreement: &ZecAgreementV1,
            _context: &ZcashClaimContextV1,
            _prepared: &PreparedClaimSubmissionV1,
        ) -> Result<FollowupClaimObservationV1, Self::Error> {
            panic!("claim-context fixture stops before follow-up observation")
        }

        async fn observe_counterparty_followup_claim(
            &self,
            _agreement: &ZecAgreementV1,
            _context: &ZcashClaimContextV1,
        ) -> Result<FollowupClaimObservationV1, Self::Error> {
            panic!("claim-context fixture stops before follow-up observation")
        }

        async fn submit_followup_claim(
            &self,
            _agreement: &ZecAgreementV1,
            _context: &ZcashClaimContextV1,
            _prepared: &PreparedClaimSubmissionV1,
        ) -> Result<(), Self::Error> {
            panic!("claim-context fixture stops before follow-up submission")
        }
    }

    #[derive(Clone, Debug)]
    struct RefundLifecycleZcash {
        funding_id: TxId,
    }

    #[async_trait]
    impl ZcashMakerLockObservationPort for RefundLifecycleZcash {
        type Error = FakeError;

        async fn observe_maker_lock(
            &self,
            _agreement: &ZecAgreementV1,
        ) -> Result<MakerLockObservationV1, Self::Error> {
            Ok(MakerLockObservationV1::Confirmed(
                FirstLockConfirmedEvidenceV1::new(
                    FirstLockStepV1::ZcashFund,
                    *self.funding_id.as_ref(),
                    self.funding_id.to_string(),
                    100,
                )
                .expect("canonical maker Zcash lock evidence"),
            ))
        }
    }

    struct ClaimFixture {
        agreement: ZecAgreementV1,
        context: ZcashClaimContextV1,
        funding: Transaction,
        secret: [u8; 32],
    }

    async fn claim_fixture() -> ClaimFixture {
        let secret = [0x91; 32];
        let agreement = agreement_with_secret(secret);
        let wire = agreement.encode_wire().expect("bounded agreement wire");
        let data = TempDir::new().expect("isolated Zebra claim fixture");
        let store = SqliteZecRecoveryStore::open_claim_capable(
            data.path().join("taker.sqlite3"),
            Participant::Taker,
            ProtectedClaimKey::new("zebra-claim-port-test-v1", [0x7a; 32])
                .expect("valid test recovery key"),
        )
        .expect("claim-capable SQLite store");
        let sdk = ZecPairSdk::new(
            Participant::Taker,
            FixedDiscovery,
            FixedNegotiation(wire),
            ClaimLifecycleLez { secret },
            NeverZcash,
            store,
        );
        let accepted = sdk
            .negotiate_at(&(), (), UnixSeconds::new(10))
            .await
            .expect("authentic accepted agreement");
        let mut active = sdk
            .activate_with_claim_preimage(accepted, ClaimPreimage::new(secret))
            .await
            .expect("durably protected claim activation");
        let funding = funding_transaction(active.agreement());
        let funding_id = funding.txid();
        let mut exact_funding = Vec::new();
        funding
            .write(&mut exact_funding)
            .expect("canonical funding serialization");
        active
            .stage_first_lock(
                FirstLockPlanV1::zcash(
                    PreparedFirstLockSubmissionV1::new(
                        FirstLockStepV1::ZcashFund,
                        *funding_id.as_ref(),
                        exact_funding,
                    )
                    .expect("valid Zcash funding submission"),
                )
                .expect("direction-correct first-lock plan"),
            )
            .await
            .expect("durable first-lock intent");
        active
            .project_first_lock(
                FirstLockConfirmedEvidenceV1::new(
                    FirstLockStepV1::ZcashFund,
                    *funding_id.as_ref(),
                    funding_id.to_string(),
                    100,
                )
                .expect("canonical first-lock evidence"),
            )
            .await
            .expect("durable first-lock projection");
        active
            .observe_maker_lock()
            .await
            .expect("canonical maker LEZ lock projection");
        active
            .drive_claim()
            .await
            .expect("canonical revealing claim projection");
        let context = active
            .zcash_claim_context()
            .expect("authentic claim-ready context");
        ClaimFixture {
            agreement: active.agreement().clone(),
            context,
            funding,
            secret,
        }
    }

    struct RefundFixture {
        agreement: ZecAgreementV1,
        context: ZcashFundingContextV1,
        funding: Transaction,
        owner: Participant,
    }

    #[allow(clippy::too_many_lines)]
    async fn refund_fixture(direction: SwapDirection) -> RefundFixture {
        let secret = [0x91; 32];
        let agreement = agreement_with_direction_and_secret(direction, secret);
        let funding = funding_transaction(&agreement);
        let funding_id = funding.txid();
        let wire = agreement.encode_wire().expect("bounded agreement wire");
        let data = TempDir::new().expect("isolated Zebra refund fixture");
        let store = SqliteZecRecoveryStore::open_claim_capable(
            data.path().join("taker.sqlite3"),
            Participant::Taker,
            ProtectedClaimKey::new("zebra-refund-port-test-v1", [0x7b; 32])
                .expect("valid test recovery key"),
        )
        .expect("refund-capable SQLite store");
        let sdk = ZecPairSdk::new(
            Participant::Taker,
            FixedDiscovery,
            FixedNegotiation(wire),
            ClaimLifecycleLez { secret },
            RefundLifecycleZcash { funding_id },
            store,
        );
        let accepted = sdk
            .negotiate_at(&(), (), UnixSeconds::new(10))
            .await
            .expect("authentic accepted agreement");
        let mut active = sdk
            .activate(accepted)
            .await
            .expect("durably protected activation");
        match direction {
            SwapDirection::TakerSellsForeign => {
                let mut exact_funding = Vec::new();
                funding
                    .write(&mut exact_funding)
                    .expect("canonical funding serialization");
                active
                    .stage_first_lock(
                        FirstLockPlanV1::zcash(
                            PreparedFirstLockSubmissionV1::new(
                                FirstLockStepV1::ZcashFund,
                                *funding_id.as_ref(),
                                exact_funding,
                            )
                            .expect("valid Zcash funding submission"),
                        )
                        .expect("direction-correct first-lock plan"),
                    )
                    .await
                    .expect("durable first-lock intent");
                active
                    .project_first_lock(
                        FirstLockConfirmedEvidenceV1::new(
                            FirstLockStepV1::ZcashFund,
                            *funding_id.as_ref(),
                            funding_id.to_string(),
                            100,
                        )
                        .expect("canonical first-lock evidence"),
                    )
                    .await
                    .expect("durable first-lock projection");
            }
            SwapDirection::TakerSellsLez => {
                active
                    .stage_first_lock(
                        FirstLockPlanV1::lez(
                            PreparedFirstLockSubmissionV1::new(
                                FirstLockStepV1::LezInitialize,
                                [0x41; 32],
                                b"initialize".to_vec(),
                            )
                            .expect("valid initialize submission"),
                            PreparedFirstLockSubmissionV1::new(
                                FirstLockStepV1::LezFund,
                                [0x42; 32],
                                b"fund".to_vec(),
                            )
                            .expect("valid fund submission"),
                        )
                        .expect("direction-correct first-lock plan"),
                    )
                    .await
                    .expect("durable first-lock intent");
                active
                    .project_first_lock(
                        FirstLockConfirmedEvidenceV1::new(
                            FirstLockStepV1::LezFund,
                            [0x42; 32],
                            hex::encode([0x42; 32]),
                            100,
                        )
                        .expect("canonical LEZ first-lock evidence"),
                    )
                    .await
                    .expect("durable first-lock projection");
            }
        }
        active
            .observe_maker_lock()
            .await
            .expect("canonical maker lock projection");
        let context = active
            .zcash_funding_context()
            .expect("authentic durable Zcash funding context");
        RefundFixture {
            owner: active.agreement().lez_claimant(),
            agreement: active.agreement().clone(),
            context,
            funding,
        }
    }

    fn canonical_lez_claim_snapshot(
        agreement: &ZecAgreementV1,
        preimage: [u8; 32],
    ) -> LezClaimNodeSnapshotV1 {
        let terms = agreement.lez_terms();
        let LezAssetV1::Native {
            authenticated_transfer_program_id,
        } = terms.asset()
        else {
            panic!("claim fixture uses native LEZ")
        };
        let depositor = *agreement.lez_account(agreement.lez_depositor());
        let claimant = *agreement.lez_account(agreement.lez_claimant());
        let metadata = LezEscrowMetadataSnapshotV1::new(
            metadata_version_for_environment(terms.chain().environment()),
            *agreement.onchain_swap_id(),
            *agreement.agreement_commitment(),
            *agreement.secret_digest(),
            depositor,
            depositor,
            claimant,
            claimant,
            *terms.custody_account(),
            *authenticated_transfer_program_id,
            *authenticated_transfer_program_id,
            [0; 32],
            terms.amount(),
            agreement.lez_refund_at_ms(),
            LezEscrowStatusV1::Claimed,
        );
        LezClaimNodeSnapshotV1::new(
            terms.chain().environment(),
            *terms.chain().channel_id(),
            *terms.chain().genesis_block_hash(),
            LezStableTipV1::new([0xc3; 32], 199, [0xc3; 32], 199),
            LezClaimTransactionSnapshotV1::new(
                [0xc1; 32],
                [0xc1; 32],
                *terms.escrow_program_id(),
                claimant,
                vec![
                    *terms.metadata_account(),
                    *terms.custody_account(),
                    claimant,
                ],
                LezClaimInstructionV1::Native {
                    swap_id: *agreement.onchain_swap_id(),
                    preimage: ClaimPreimage::new(preimage),
                },
                true,
                true,
                100,
                [0xc2; 32],
                [0xc2; 32],
                LezInclusionStatusV1::Finalized,
            ),
            *terms.escrow_program_id(),
            *terms.metadata_account(),
            metadata,
            *terms.custody_account(),
            LezCustodySnapshotV1::Native {
                program_owner: *authenticated_transfer_program_id,
                balance: 0,
            },
        )
    }

    const fn metadata_version_for_environment(environment: LezEnvironmentV1) -> u8 {
        match environment {
            LezEnvironmentV1::DeterministicLocalV0_1_2Compatibility => 1,
            LezEnvironmentV1::DeterministicLocalV0_2 | LezEnvironmentV1::PublicTestnetV0_2 => 2,
        }
    }

    #[tokio::test]
    async fn funding_observation_distinguishes_confirmed_spent_absent_and_moving_tip() {
        let agreement = agreement();
        let transaction = funding_transaction(&agreement);

        let (confirmed, target) = FakeRpc::confirmed(&agreement, &transaction);
        let port = ZebraRpcClaimPort::new(
            confirmed.clone(),
            NeverSigner,
            ZebraChainIdentity::deterministic_regtest_nu6_2(),
        );
        assert!(matches!(
            port.observe_funding_target(&agreement, &target)
                .await
                .expect("canonical unspent funding"),
            ZcashFundingObservationV1::Confirmed { .. }
        ));
        assert_eq!(
            confirmed.calls(),
            [
                "chain_info",
                "block_hash:0",
                "transaction_state",
                "block_hash:100",
                "unspent_output",
                "chain_info",
            ]
        );

        let (spent, target) = FakeRpc::confirmed(&agreement, &transaction);
        spent.edit(|state| state.unspent = None);
        assert_eq!(
            ZebraRpcClaimPort::new(
                spent,
                NeverSigner,
                ZebraChainIdentity::deterministic_regtest_nu6_2(),
            )
            .observe_funding_target(&agreement, &target)
            .await
            .expect("canonical transaction with current UTXO absence"),
            ZcashFundingObservationV1::Spent
        );

        let (absent, target) = FakeRpc::confirmed(&agreement, &transaction);
        absent.edit(|state| state.transaction = None);
        assert_eq!(
            ZebraRpcClaimPort::new(
                absent.clone(),
                NeverSigner,
                ZebraChainIdentity::deterministic_regtest_nu6_2(),
            )
            .observe_funding_target(&agreement, &target)
            .await
            .expect("stable current-view transaction absence"),
            ZcashFundingObservationV1::Absent
        );
        assert!(!absent.calls().contains(&"unspent_output".to_owned()));

        let (moving, target) = FakeRpc::confirmed(&agreement, &transaction);
        moving.edit(|state| {
            let previous = state.chain_infos[1];
            state.chain_infos[1] = ZebraChainInfo::new(
                previous.rpc_chain(),
                BlockHeight::from_u32(TIP_HEIGHT + 1),
                BlockHash([0x44; 32]),
                previous.consensus_branch_id(),
            );
        });
        assert_eq!(
            ZebraRpcClaimPort::new(
                moving,
                NeverSigner,
                ZebraChainIdentity::deterministic_regtest_nu6_2(),
            )
            .observe_funding_target(&agreement, &target)
            .await
            .expect("moving tip is never classified as spent or absent"),
            ZcashFundingObservationV1::Unstable
        );

        let (replaced, target) = FakeRpc::confirmed(&agreement, &transaction);
        replaced.edit(|state| state.canonical_block = BlockHash([0x55; 32]));
        assert_eq!(
            ZebraRpcClaimPort::new(
                replaced,
                NeverSigner,
                ZebraChainIdentity::deterministic_regtest_nu6_2(),
            )
            .observe_funding_target(&agreement, &target)
            .await
            .expect("same-tip replacement is not canonical funding"),
            ZcashFundingObservationV1::Unstable
        );
    }

    #[tokio::test]
    async fn stable_mismatched_utxo_tip_is_rejected_as_ambiguous() {
        let agreement = agreement();
        let transaction = funding_transaction(&agreement);
        let (rpc, target) = FakeRpc::confirmed(&agreement, &transaction);
        rpc.edit(|state| {
            let previous = state.unspent.as_ref().expect("UTXO");
            state.unspent = Some(ZebraUnspentOutput::new(
                BlockHash([0x99; 32]),
                previous.confirmations(),
                previous.output().clone(),
            ));
        });
        let error = ZebraRpcClaimPort::new(
            rpc,
            NeverSigner,
            ZebraChainIdentity::deterministic_regtest_nu6_2(),
        )
        .observe_funding_target(&agreement, &target)
        .await
        .expect_err("same-bracket UTXO tip mismatch is response ambiguity");
        assert!(matches!(error, ZebraClaimError::UtxoTipMismatch));
    }

    #[tokio::test]
    async fn fresh_confirmed_funding_prepares_exact_agreement_derived_claim() {
        let (fixture, rpc, port, prepared) = prepared_claim_fixture().await;
        let funding_output = fixture.funding.transparent_bundle().expect("funding").vout[0].clone();
        let request = fixture
            .agreement
            .claim_spend_request(
                fixture.context.funding_outpoint().clone(),
                funding_output,
                BlockHeight::from_u32(TIP_HEIGHT),
            )
            .expect("agreement-derived claim request");
        let expected = build_claim_transaction(
            fixture.agreement.binding().expected_output().contract(),
            &request,
            &claimant_key(),
            &fixture.secret,
        )
        .expect("canonical SDK claim");
        let mut exact = Vec::new();
        expected.write(&mut exact).expect("canonical serialization");

        assert_eq!(prepared.step(), ClaimStepV1::FollowupZcash);
        assert_eq!(prepared.exact_submission(), exact);
        assert_eq!(prepared.expected_submission_id(), expected.txid().as_ref());
        assert_eq!(
            rpc.calls(),
            [
                "chain_info",
                "block_hash:0",
                "transaction_state",
                "block_hash:100",
                "unspent_output",
                "chain_info",
            ]
        );
        assert!(rpc.submitted().is_empty());
        assert_eq!(
            fixture.context.funding_outpoint().hash(),
            fixture.funding.txid().as_ref(),
            "the signer must consume the coordinator-pinned funding transaction"
        );
        drop(port);
    }

    #[tokio::test]
    async fn prepared_mutations_and_policy_drift_fail_before_any_send() {
        let (fixture, rpc, port, prepared) = prepared_claim_fixture().await;
        rpc.edit(|state| state.calls.clear());

        let mut appended = prepared.exact_submission().to_vec();
        appended.push(0);
        let cases = [
            PreparedClaimSubmissionV1::new(
                ClaimStepV1::RevealingLez,
                *prepared.expected_submission_id(),
                prepared.exact_submission().to_vec(),
            )
            .expect("wrong-step envelope"),
            PreparedClaimSubmissionV1::new(
                ClaimStepV1::FollowupZcash,
                [0xaa; 32],
                prepared.exact_submission().to_vec(),
            )
            .expect("wrong-txid envelope"),
            PreparedClaimSubmissionV1::new(
                ClaimStepV1::FollowupZcash,
                *prepared.expected_submission_id(),
                appended,
            )
            .expect("trailing-byte mutation"),
            policy_mutated_prepared(&fixture, ClaimPolicyMutation::Outpoint),
            policy_mutated_prepared(&fixture, ClaimPolicyMutation::Destination),
            policy_mutated_prepared(&fixture, ClaimPolicyMutation::Fee),
            policy_mutated_prepared(&fixture, ClaimPolicyMutation::Expiry),
        ];

        for candidate in cases {
            port.submit_followup_claim(&fixture.agreement, &fixture.context, &candidate)
                .await
                .expect_err("mutated or policy-drifted claim must fail closed");
        }
        assert!(rpc.calls().is_empty(), "validation must precede every RPC");
        assert!(rpc.submitted().is_empty());

        let configured = port.identity();
        let wrong_branch = ZebraChainIdentity::new(
            configured.network(),
            configured.rpc_chain(),
            BranchId::Nu6,
            configured.genesis_hash(),
        )
        .expect("nonzero same-network identity");
        let error =
            ZebraRpcClaimPort::new(rpc.clone(), CanonicalSigner(claimant_key()), wrong_branch)
                .submit_followup_claim(&fixture.agreement, &fixture.context, &prepared)
                .await
                .expect_err("agreement branch mismatch must fail before RPC");
        assert!(matches!(
            error,
            ZebraClaimError::ConfiguredConsensusBranchMismatch
        ));
        assert!(rpc.calls().is_empty());
        assert!(rpc.submitted().is_empty());
    }

    #[tokio::test]
    async fn exact_observation_distinguishes_absent_mempool_and_confirmed() {
        let (fixture, rpc, port, prepared) = prepared_claim_fixture().await;
        rpc.edit(|state| {
            state.calls.clear();
            state.transaction = None;
        });
        assert_eq!(
            port.observe_prepared_followup_claim(&fixture.agreement, &fixture.context, &prepared)
                .await
                .expect("stable absence"),
            FollowupClaimObservationV1::Absent
        );

        rpc.edit(|state| {
            state.calls.clear();
            state.transaction = Some(ZebraTransactionState::Mempool {
                raw_transaction: prepared.exact_submission().to_vec(),
            });
        });
        assert_eq!(
            port.observe_prepared_followup_claim(&fixture.agreement, &fixture.context, &prepared)
                .await
                .expect("exact mempool claim"),
            FollowupClaimObservationV1::Unstable
        );

        set_confirmed_claim(&rpc, &prepared);
        let observation = port
            .observe_prepared_followup_claim(&fixture.agreement, &fixture.context, &prepared)
            .await
            .expect("stable canonical claim");
        let FollowupClaimObservationV1::Confirmed(evidence) = observation else {
            panic!("canonical claim must confirm")
        };
        assert_eq!(
            evidence.observed_submission_id(),
            prepared.expected_submission_id()
        );
        assert_eq!(evidence.confirmations(), TIP_HEIGHT - INCLUSION_HEIGHT + 1);
        assert_eq!(
            rpc.calls(),
            [
                "chain_info",
                "block_hash:0",
                "transaction_state",
                "block_hash:100",
                "chain_info",
            ]
        );
        assert!(rpc.submitted().is_empty());
    }

    #[tokio::test]
    async fn observation_rejects_byte_block_and_tip_identity_drift() {
        let (fixture, rpc, port, prepared) = prepared_claim_fixture().await;

        set_confirmed_claim(&rpc, &prepared);
        rpc.edit(|state| {
            let Some(ZebraTransactionState::Confirmed {
                raw_transaction, ..
            }) = &mut state.transaction
            else {
                unreachable!()
            };
            raw_transaction.push(0);
        });
        assert!(matches!(
            port.observe_prepared_followup_claim(&fixture.agreement, &fixture.context, &prepared)
                .await,
            Err(ZebraClaimError::ObservedClaimBytesMismatch)
        ));

        set_confirmed_claim(&rpc, &prepared);
        rpc.edit(|state| state.canonical_block = BlockHash([0x55; 32]));
        assert!(matches!(
            port.observe_prepared_followup_claim(&fixture.agreement, &fixture.context, &prepared)
                .await,
            Err(ZebraClaimError::SpendObservation(_))
        ));

        set_confirmed_claim(&rpc, &prepared);
        rpc.edit(|state| {
            state.canonical_block = BlockHash([0x22; 32]);
            let tip = canonical_tip();
            state.chain_infos = VecDeque::from([
                tip,
                ZebraChainInfo::new(
                    tip.rpc_chain(),
                    BlockHeight::from_u32(TIP_HEIGHT + 1),
                    BlockHash([0x44; 32]),
                    tip.consensus_branch_id(),
                ),
            ]);
        });
        assert_eq!(
            port.observe_prepared_followup_claim(&fixture.agreement, &fixture.context, &prepared)
                .await
                .expect("moving tip is an unstable observation"),
            FollowupClaimObservationV1::Unstable
        );
        assert!(rpc.submitted().is_empty());
    }

    #[tokio::test]
    async fn submit_preserves_exact_bytes_and_stable_rpc_order() {
        let (fixture, rpc, port, prepared) = prepared_claim_fixture().await;
        let transaction_id = TxId::from_bytes(*prepared.expected_submission_id());
        rpc.edit(|state| {
            state.calls.clear();
            state.submissions.push_back(Ok(transaction_id));
        });

        port.submit_followup_claim(&fixture.agreement, &fixture.context, &prepared)
            .await
            .expect("stable exact submission");
        assert_eq!(rpc.submitted(), [prepared.exact_submission().to_vec()]);
        assert_eq!(
            rpc.calls(),
            [
                "chain_info",
                "block_hash:0",
                "send_raw_transaction",
                "chain_info",
                "block_hash:0",
            ]
        );
    }

    #[tokio::test]
    async fn submit_rejects_returned_transaction_id_after_one_exact_send() {
        let (fixture, rpc, port, prepared) = prepared_claim_fixture().await;
        rpc.edit(|state| {
            state.calls.clear();
            state
                .submissions
                .push_back(Ok(TxId::from_bytes([0x99; 32])));
        });

        assert!(matches!(
            port.submit_followup_claim(&fixture.agreement, &fixture.context, &prepared)
                .await,
            Err(ZebraClaimError::SubmittedTransactionIdMismatch)
        ));
        assert_exactly_one_send(&rpc, &prepared);
    }

    #[tokio::test]
    async fn post_send_tip_chain_branch_or_genesis_drift_is_unknown_without_retry() {
        let (fixture, rpc, port, prepared) = prepared_claim_fixture().await;
        let transaction_id = TxId::from_bytes(*prepared.expected_submission_id());
        let identity = ZebraChainIdentity::deterministic_regtest_nu6_2();
        let tip = canonical_tip();
        let moved_tip = ZebraChainInfo::new(
            tip.rpc_chain(),
            BlockHeight::from_u32(TIP_HEIGHT + 1),
            BlockHash([0x44; 32]),
            tip.consensus_branch_id(),
        );
        let wrong_chain = ZebraChainInfo::new(
            ZebraRpcChain::Main,
            tip.tip_height(),
            tip.tip_hash(),
            tip.consensus_branch_id(),
        );
        let wrong_branch = ZebraChainInfo::new(
            tip.rpc_chain(),
            tip.tip_height(),
            tip.tip_hash(),
            BranchId::Nu6,
        );

        for (chain_samples, block_samples) in [
            (vec![Ok(tip), Ok(moved_tip)], Vec::new()),
            (vec![Ok(tip), Ok(wrong_chain)], Vec::new()),
            (vec![Ok(tip), Ok(wrong_branch)], Vec::new()),
            (
                vec![Ok(tip), Ok(tip)],
                vec![Ok(identity.genesis_hash()), Ok(BlockHash([0x99; 32]))],
            ),
        ] {
            let previous_sends = rpc.submitted().len();
            rpc.edit(|state| {
                state.calls.clear();
                state.chain_info_overrides = chain_samples.into();
                state.block_hash_overrides = block_samples.into();
                state.submissions.push_back(Ok(transaction_id));
            });
            assert!(matches!(
                port.submit_followup_claim(&fixture.agreement, &fixture.context, &prepared)
                    .await,
                Err(ZebraClaimError::UnstableTipDuringSubmission)
            ));
            assert_eq!(rpc.submitted().len(), previous_sends + 1);
            assert_eq!(
                rpc.submitted().last().expect("one exact send"),
                prepared.exact_submission()
            );
            assert_eq!(
                rpc.calls()
                    .iter()
                    .filter(|call| call.as_str() == "send_raw_transaction")
                    .count(),
                1,
                "post-send drift must never trigger an adapter retry"
            );
        }
    }

    #[tokio::test]
    async fn post_send_rpc_failures_are_unknown_after_one_exact_send() {
        let (fixture, rpc, port, prepared) = prepared_claim_fixture().await;
        let transaction_id = TxId::from_bytes(*prepared.expected_submission_id());
        let identity = ZebraChainIdentity::deterministic_regtest_nu6_2();
        let tip = canonical_tip();

        rpc.edit(|state| {
            state.calls.clear();
            state.chain_info_overrides = VecDeque::from([Ok(tip), Err(FakeError::Transport)]);
            state.submissions.push_back(Ok(transaction_id));
        });
        assert!(matches!(
            port.submit_followup_claim(&fixture.agreement, &fixture.context, &prepared)
                .await,
            Err(ZebraClaimError::SubmissionOutcomeUnknown(
                FakeError::Transport
            ))
        ));
        assert_exactly_one_send(&rpc, &prepared);

        rpc.edit(|state| {
            state.calls.clear();
            state.chain_info_overrides = VecDeque::from([Ok(tip), Ok(tip)]);
            state.block_hash_overrides =
                VecDeque::from([Ok(identity.genesis_hash()), Err(FakeError::RpcCode(-1))]);
            state.submissions.push_back(Ok(transaction_id));
            state.submitted.clear();
        });
        assert!(matches!(
            port.submit_followup_claim(&fixture.agreement, &fixture.context, &prepared)
                .await,
            Err(ZebraClaimError::SubmissionOutcomeUnknown(
                FakeError::RpcCode(-1)
            ))
        ));
        assert_exactly_one_send(&rpc, &prepared);
    }

    #[tokio::test]
    async fn observation_rejects_live_chain_branch_and_genesis_mismatch_before_lookup() {
        let (fixture, rpc, port, prepared) = prepared_claim_fixture().await;
        let tip = canonical_tip();
        let wrong_chain = ZebraChainInfo::new(
            ZebraRpcChain::Main,
            tip.tip_height(),
            tip.tip_hash(),
            tip.consensus_branch_id(),
        );
        let wrong_branch = ZebraChainInfo::new(
            tip.rpc_chain(),
            tip.tip_height(),
            tip.tip_hash(),
            BranchId::Nu6,
        );

        rpc.edit(|state| {
            state.calls.clear();
            state.chain_info_overrides.push_back(Ok(wrong_chain));
        });
        assert!(matches!(
            port.observe_prepared_followup_claim(&fixture.agreement, &fixture.context, &prepared)
                .await,
            Err(ZebraClaimError::RpcChainMismatch)
        ));
        assert!(!rpc.calls().contains(&"transaction_state".to_owned()));

        rpc.edit(|state| {
            state.calls.clear();
            state.chain_info_overrides.push_back(Ok(wrong_branch));
        });
        assert!(matches!(
            port.observe_prepared_followup_claim(&fixture.agreement, &fixture.context, &prepared)
                .await,
            Err(ZebraClaimError::RpcConsensusBranchMismatch)
        ));
        assert!(!rpc.calls().contains(&"transaction_state".to_owned()));

        rpc.edit(|state| {
            state.calls.clear();
            state
                .block_hash_overrides
                .push_back(Ok(BlockHash([0x99; 32])));
        });
        assert!(matches!(
            port.observe_prepared_followup_claim(&fixture.agreement, &fixture.context, &prepared)
                .await,
            Err(ZebraClaimError::GenesisMismatch)
        ));
        assert!(!rpc.calls().contains(&"transaction_state".to_owned()));
        assert!(rpc.submitted().is_empty());
    }

    #[tokio::test]
    async fn submit_classifies_definitive_and_unknown_zebra_outcomes() {
        let (fixture, rpc, port, prepared) = prepared_claim_fixture().await;
        for code in [-22, -25, -26] {
            rpc.edit(|state| state.submissions.push_back(Err(FakeError::RpcCode(code))));
            assert!(matches!(
                port.submit_followup_claim(&fixture.agreement, &fixture.context, &prepared)
                    .await,
                Err(ZebraClaimError::SubmissionRejected(FakeError::RpcCode(actual)))
                    if actual == code
            ));
        }
        for error in [
            FakeError::RpcCode(-27),
            FakeError::RpcCode(-123),
            FakeError::Transport,
        ] {
            rpc.edit(|state| state.submissions.push_back(Err(error.clone())));
            assert!(matches!(
                port.submit_followup_claim(&fixture.agreement, &fixture.context, &prepared)
                    .await,
                Err(ZebraClaimError::SubmissionOutcomeUnknown(actual)) if actual == error
            ));
        }
        assert_eq!(rpc.submitted().len(), 6);
        assert!(
            rpc.submitted()
                .iter()
                .all(|exact| exact == prepared.exact_submission())
        );
    }

    #[tokio::test]
    async fn failed_claim_broadcast_reconciles_exact_known_transaction() {
        for confirmed in [false, true] {
            let (fixture, rpc, port, prepared) = prepared_claim_fixture().await;
            if confirmed {
                set_confirmed_claim(&rpc, &prepared);
            } else {
                rpc.edit(|state| {
                    state.calls.clear();
                    state.transaction = Some(ZebraTransactionState::Mempool {
                        raw_transaction: prepared.exact_submission().to_vec(),
                    });
                });
            }
            rpc.edit(|state| {
                state.submissions.push_back(Err(FakeError::RpcCode(-27)));
            });

            port.submit_followup_claim(&fixture.agreement, &fixture.context, &prepared)
                .await
                .expect("the exact transaction is already known on the stable configured chain");
            assert_exactly_one_send(&rpc, &prepared);
            assert_eq!(
                rpc.calls()
                    .iter()
                    .filter(|call| call.as_str() == "transaction_state")
                    .count(),
                1,
                "one read-only reconciliation follows the failed send"
            );
        }
    }

    #[tokio::test]
    async fn failed_claim_broadcast_never_reconciles_unstable_or_unbound_visibility() {
        enum Mutation {
            Bytes,
            Tip,
            Genesis,
            Inactive,
            Confirmations,
        }

        for mutation in [
            Mutation::Bytes,
            Mutation::Tip,
            Mutation::Genesis,
            Mutation::Inactive,
            Mutation::Confirmations,
        ] {
            let (fixture, rpc, port, prepared) = prepared_claim_fixture().await;
            rpc.edit(|state| {
                state.calls.clear();
                state.submissions.push_back(Err(FakeError::RpcCode(-27)));
                match mutation {
                    Mutation::Bytes => {
                        state.transaction = Some(ZebraTransactionState::Mempool {
                            raw_transaction: vec![0xff],
                        });
                    }
                    Mutation::Tip => {
                        state.transaction = Some(ZebraTransactionState::Mempool {
                            raw_transaction: prepared.exact_submission().to_vec(),
                        });
                        let tip = canonical_tip();
                        state.chain_info_overrides = VecDeque::from([
                            Ok(tip),
                            Ok(ZebraChainInfo::new(
                                tip.rpc_chain(),
                                BlockHeight::from_u32(TIP_HEIGHT + 1),
                                BlockHash([0x44; 32]),
                                tip.consensus_branch_id(),
                            )),
                        ]);
                    }
                    Mutation::Genesis => {
                        state.transaction = Some(ZebraTransactionState::Mempool {
                            raw_transaction: prepared.exact_submission().to_vec(),
                        });
                        state.block_hash_overrides = VecDeque::from([
                            Ok(state.identity.genesis_hash()),
                            Ok(BlockHash([0x99; 32])),
                        ]);
                    }
                    Mutation::Inactive | Mutation::Confirmations => {
                        state.transaction = Some(ZebraTransactionState::Confirmed {
                            raw_transaction: prepared.exact_submission().to_vec(),
                            block_hash: state.canonical_block,
                            block_height: BlockHeight::from_u32(INCLUSION_HEIGHT),
                            confirmations: if matches!(mutation, Mutation::Confirmations) {
                                1
                            } else {
                                TIP_HEIGHT - INCLUSION_HEIGHT + 1
                            },
                            in_active_chain: !matches!(mutation, Mutation::Inactive),
                        });
                    }
                }
            });

            assert!(matches!(
                port.submit_followup_claim(&fixture.agreement, &fixture.context, &prepared)
                    .await,
                Err(ZebraClaimError::SubmissionOutcomeUnknown(
                    FakeError::RpcCode(-27)
                ))
            ));
            assert_exactly_one_send(&rpc, &prepared);
        }
    }

    #[tokio::test]
    async fn counterparty_discovery_finds_exact_canonical_spend() {
        let (fixture, rpc, port, prepared) = prepared_claim_fixture().await;
        rpc.edit(|state| state.calls.clear());
        set_confirmed_claim(&rpc, &prepared);
        let observed = port
            .observe_counterparty_followup_claim(&fixture.agreement, &fixture.context)
            .await
            .expect("bounded canonical scan finds the exact counterparty claim");
        let FollowupClaimObservationV1::Confirmed(evidence) = observed else {
            panic!("canonical counterparty spend must be confirmed")
        };
        assert_eq!(
            evidence.observed_submission_id(),
            prepared.expected_submission_id()
        );
        assert_eq!(
            evidence.transaction_id(),
            TxId::from_bytes(*prepared.expected_submission_id()).to_string()
        );
        assert!(rpc.submitted().is_empty());
    }

    #[tokio::test]
    async fn counterparty_discovery_distinguishes_unspent_mempool_and_exhausted_scan() {
        let (fixture, _unspent_rpc, unspent_port, _prepared) = prepared_claim_fixture().await;
        assert_eq!(
            unspent_port
                .observe_counterparty_followup_claim(&fixture.agreement, &fixture.context)
                .await
                .expect("stable exact UTXO proves no spend"),
            FollowupClaimObservationV1::Absent
        );

        let (fixture, mempool_rpc, mempool_port, prepared) = prepared_claim_fixture().await;
        set_confirmed_claim(&mempool_rpc, &prepared);
        mempool_rpc.edit(|state| {
            state.transaction = Some(ZebraTransactionState::Mempool {
                raw_transaction: prepared.exact_submission().to_vec(),
            });
        });
        assert_eq!(
            mempool_port
                .observe_counterparty_followup_claim(&fixture.agreement, &fixture.context)
                .await
                .expect("valid mempool claim is not final"),
            FollowupClaimObservationV1::Unstable
        );

        let (fixture, exhausted_rpc, exhausted_port, exhausted_prepared) =
            prepared_claim_fixture().await;
        set_confirmed_claim(&exhausted_rpc, &exhausted_prepared);
        let exhausted_port = exhausted_port.with_counterparty_scan_blocks(1);
        assert_eq!(
            exhausted_port
                .observe_counterparty_followup_claim(&fixture.agreement, &fixture.context)
                .await
                .expect("a bounded miss never becomes global absence"),
            FollowupClaimObservationV1::Unstable
        );
    }

    #[tokio::test]
    async fn counterparty_discovery_rejects_tip_drift_and_block_context_mutation() {
        let (fixture, rpc, port, prepared) = prepared_claim_fixture().await;
        set_confirmed_claim(&rpc, &prepared);
        rpc.edit(|state| {
            let moving = ZebraChainInfo::new(
                ZebraRpcChain::Test,
                BlockHeight::from_u32(TIP_HEIGHT + 1),
                BlockHash([0x44; 32]),
                BranchId::Nu6_2,
            );
            state.chain_infos = VecDeque::from([canonical_tip(), moving]);
        });
        assert_eq!(
            port.observe_counterparty_followup_claim(&fixture.agreement, &fixture.context)
                .await
                .expect("tip drift is an unstable observation"),
            FollowupClaimObservationV1::Unstable
        );

        let (fixture, rpc, port, prepared) = prepared_claim_fixture().await;
        set_confirmed_claim(&rpc, &prepared);
        rpc.edit(|state| {
            state
                .canonical_block_overrides
                .push_back(Ok(ZebraCanonicalBlock::new(
                    state.canonical_block,
                    BlockHeight::from_u32(INCLUSION_HEIGHT + 1),
                    Vec::new(),
                )));
        });
        assert!(matches!(
            port.observe_counterparty_followup_claim(&fixture.agreement, &fixture.context)
                .await,
            Err(ZebraClaimError::BlockInventoryMismatch)
        ));
    }

    #[tokio::test]
    async fn counterparty_discovery_rejects_conflicts_and_transaction_byte_mutation() {
        let (fixture, rpc, port, prepared) = prepared_claim_fixture().await;
        let conflicting = policy_mutated_prepared(&fixture, ClaimPolicyMutation::Destination);
        set_confirmed_claim(&rpc, &prepared);
        rpc.edit(|state| {
            state
                .canonical_block_overrides
                .push_back(Ok(ZebraCanonicalBlock::new(
                    state.canonical_block,
                    BlockHeight::from_u32(INCLUSION_HEIGHT),
                    vec![
                        TxId::from_bytes(*prepared.expected_submission_id()),
                        TxId::from_bytes(*conflicting.expected_submission_id()),
                    ],
                )));
            state
                .block_transaction_overrides
                .push_back(Ok(Some(prepared.exact_submission().to_vec())));
            state
                .block_transaction_overrides
                .push_back(Ok(Some(conflicting.exact_submission().to_vec())));
        });
        assert!(matches!(
            port.observe_counterparty_followup_claim(&fixture.agreement, &fixture.context)
                .await,
            Err(ZebraClaimError::ConflictingSpendCandidates)
        ));

        let (fixture, rpc, port, prepared) = prepared_claim_fixture().await;
        let changed = policy_mutated_prepared(&fixture, ClaimPolicyMutation::Fee);
        set_confirmed_claim(&rpc, &prepared);
        rpc.edit(|state| {
            state
                .block_transaction_overrides
                .push_back(Ok(Some(changed.exact_submission().to_vec())));
        });
        assert!(matches!(
            port.observe_counterparty_followup_claim(&fixture.agreement, &fixture.context)
                .await,
            Err(ZebraClaimError::DiscoveredTransactionIdMismatch)
        ));
    }

    #[tokio::test]
    async fn counterparty_discovery_validates_signed_policy_and_preserves_transport_ambiguity() {
        for mutation in [
            ClaimPolicyMutation::Destination,
            ClaimPolicyMutation::Fee,
            ClaimPolicyMutation::Expiry,
        ] {
            let (fixture, rpc, port, _prepared) = prepared_claim_fixture().await;
            let changed = policy_mutated_prepared(&fixture, mutation);
            set_confirmed_claim(&rpc, &changed);
            assert!(
                port.observe_counterparty_followup_claim(&fixture.agreement, &fixture.context)
                    .await
                    .is_err(),
                "signed policy mutation must fail closed"
            );
        }

        let (fixture, rpc, port, prepared) = prepared_claim_fixture().await;
        set_confirmed_claim(&rpc, &prepared);
        rpc.edit(|state| state.mempool_overrides.push_back(Err(FakeError::Transport)));
        assert!(matches!(
            port.observe_counterparty_followup_claim(&fixture.agreement, &fixture.context)
                .await,
            Err(ZebraClaimError::Rpc(FakeError::Transport))
        ));
    }

    const REFUND_TIP_HEIGHT: u32 = 125;
    const REFUND_INCLUSION_HEIGHT: u32 = 121;
    const REFUND_TIP_HASH_BYTE: u8 = 125;
    const REFUND_INCLUSION_HASH_BYTE: u8 = 121;
    const NEXT_REFUND_TIP_HASH_BYTE: u8 = 126;

    fn refund_key(direction: SwapDirection) -> SecretKey {
        let bytes = match direction {
            SwapDirection::TakerSellsForeign => [2; 32],
            SwapDirection::TakerSellsLez => [1; 32],
        };
        SecretKey::from_slice(&bytes).expect("direction-fixed refund key")
    }

    fn set_refund_tip(rpc: &FakeRpc) {
        rpc.edit(|state| {
            let tip = ZebraChainInfo::new(
                ZebraRpcChain::Test,
                BlockHeight::from_u32(REFUND_TIP_HEIGHT),
                BlockHash([REFUND_TIP_HASH_BYTE; 32]),
                BranchId::Nu6_2,
            );
            state.chain_infos = VecDeque::from(vec![tip; 512]);
            let confirmations = REFUND_TIP_HEIGHT - INCLUSION_HEIGHT + 1;
            if let ZebraTransactionState::Confirmed {
                confirmations: actual,
                ..
            } = &mut state.funding_transaction
            {
                *actual = confirmations;
            }
            if let Some(ZebraTransactionState::Confirmed {
                confirmations: actual,
                ..
            }) = &mut state.transaction
            {
                *actual = confirmations;
            }
            if let Some(unspent) = state.unspent.as_ref() {
                state.unspent = Some(ZebraUnspentOutput::new(
                    tip.tip_hash(),
                    confirmations,
                    unspent.output().clone(),
                ));
            }
        });
    }

    async fn prepared_refund_fixture(
        direction: SwapDirection,
    ) -> (
        RefundFixture,
        FakeRpc,
        ZebraRpcRefundPort<FakeRpc, CanonicalRefundSigner>,
        PreparedRefundSubmissionV1,
    ) {
        let fixture = refund_fixture(direction).await;
        let (rpc, target) = FakeRpc::confirmed(&fixture.agreement, &fixture.funding);
        assert_eq!(target.outpoint, *fixture.context.funding_outpoint());
        set_refund_tip(&rpc);
        let port = ZebraRpcRefundPort::new(
            rpc.clone(),
            CanonicalRefundSigner {
                participant: fixture.owner,
                key: refund_key(direction),
            },
            ZebraChainIdentity::deterministic_regtest_nu6_2(),
        );
        assert_eq!(
            port.observe_refund_eligibility(&fixture.agreement, &fixture.context)
                .await
                .expect("stable eligibility"),
            RefundEligibilityObservationV1::canonical(ChainPosition::block_height(
                Chain::Zcash,
                u64::from(REFUND_TIP_HEIGHT),
            )),
        );
        let prepared = port
            .prepare_refund(&fixture.agreement, &fixture.context)
            .await
            .expect("mature canonical funding prepares exact refund");
        (fixture, rpc, port, prepared)
    }

    fn set_refund_absent(rpc: &FakeRpc) {
        rpc.edit(|state| {
            state.calls.clear();
            state.discovery_enabled = true;
            state.transaction = None;
        });
    }

    fn set_confirmed_refund(rpc: &FakeRpc, prepared: &PreparedRefundSubmissionV1) {
        rpc.edit(|state| {
            state.calls.clear();
            state.discovery_enabled = true;
            state.unspent = None;
            state.transaction = Some(ZebraTransactionState::Confirmed {
                raw_transaction: prepared.exact_submission().to_vec(),
                block_hash: BlockHash([REFUND_INCLUSION_HASH_BYTE; 32]),
                block_height: BlockHeight::from_u32(REFUND_INCLUSION_HEIGHT),
                confirmations: REFUND_TIP_HEIGHT - REFUND_INCLUSION_HEIGHT + 1,
                in_active_chain: true,
            });
        });
    }

    #[derive(Clone, Copy)]
    enum RefundPolicyMutation {
        Outpoint,
        Destination,
        Fee,
        Expiry,
    }

    fn refund_policy_mutated_prepared(
        fixture: &RefundFixture,
        direction: SwapDirection,
        mutation: RefundPolicyMutation,
    ) -> PreparedRefundSubmissionV1 {
        let expected = fixture.agreement.binding().expected_output();
        let funding_output = fixture.funding.transparent_bundle().expect("funding").vout[0].clone();
        let canonical = fixture
            .agreement
            .refund_spend_request(
                fixture.context.funding_outpoint().clone(),
                funding_output.clone(),
                BlockHeight::from_u32(REFUND_TIP_HEIGHT),
            )
            .expect("canonical refund request");
        let (outpoint, destination, fee, expiry_height) = match mutation {
            RefundPolicyMutation::Outpoint => (
                OutPoint::new([0x66; 32], 0),
                canonical.destination(),
                canonical.fee(),
                canonical.expiry_height(),
            ),
            RefundPolicyMutation::Destination => (
                canonical.prevout().clone(),
                TransparentAddress::PublicKeyHash([0x55; 20]),
                canonical.fee(),
                canonical.expiry_height(),
            ),
            RefundPolicyMutation::Fee => (
                canonical.prevout().clone(),
                canonical.destination(),
                Zatoshis::from_u64(10_001).expect("mutated fee"),
                canonical.expiry_height(),
            ),
            RefundPolicyMutation::Expiry => (
                canonical.prevout().clone(),
                canonical.destination(),
                canonical.fee(),
                BlockHeight::from_u32(1),
            ),
        };
        let request = TransparentSpendRequest::new(
            expected.contract(),
            outpoint,
            funding_output,
            destination,
            fee,
            expiry_height,
            canonical.consensus_branch_id(),
        )
        .expect("consensus-valid refund policy mutation");
        let transaction =
            build_refund_transaction(expected.contract(), &request, &refund_key(direction))
                .expect("signed refund policy mutation");
        let mut exact = Vec::new();
        transaction
            .write(&mut exact)
            .expect("canonical refund mutation bytes");
        PreparedRefundSubmissionV1::new(RefundStepV1::Zcash, *transaction.txid().as_ref(), exact)
            .expect("well-formed refund policy mutation")
    }

    #[derive(Clone, Copy)]
    enum ClaimPolicyMutation {
        Outpoint,
        Destination,
        Fee,
        Expiry,
    }

    async fn prepared_claim_fixture() -> (
        ClaimFixture,
        FakeRpc,
        ZebraRpcClaimPort<FakeRpc, CanonicalSigner>,
        PreparedClaimSubmissionV1,
    ) {
        let fixture = claim_fixture().await;
        let (rpc, target) = FakeRpc::confirmed(&fixture.agreement, &fixture.funding);
        assert_eq!(target.outpoint, *fixture.context.funding_outpoint());
        let port = ZebraRpcClaimPort::new(
            rpc.clone(),
            CanonicalSigner(claimant_key()),
            ZebraChainIdentity::deterministic_regtest_nu6_2(),
        );
        let prepared = port
            .prepare_followup_claim(
                &fixture.agreement,
                &fixture.context,
                &ClaimPreimage::new(fixture.secret),
            )
            .await
            .expect("fresh canonical funding prepares an exact claim");
        (fixture, rpc, port, prepared)
    }

    fn policy_mutated_prepared(
        fixture: &ClaimFixture,
        mutation: ClaimPolicyMutation,
    ) -> PreparedClaimSubmissionV1 {
        let expected = fixture.agreement.binding().expected_output();
        let funding_output = fixture.funding.transparent_bundle().expect("funding").vout[0].clone();
        let canonical = fixture
            .agreement
            .claim_spend_request(
                fixture.context.funding_outpoint().clone(),
                funding_output.clone(),
                BlockHeight::from_u32(TIP_HEIGHT),
            )
            .expect("canonical request");
        let (outpoint, destination, fee, expiry_height) = match mutation {
            ClaimPolicyMutation::Outpoint => (
                OutPoint::new([0x66; 32], 0),
                canonical.destination(),
                canonical.fee(),
                canonical.expiry_height(),
            ),
            ClaimPolicyMutation::Destination => (
                canonical.prevout().clone(),
                TransparentAddress::PublicKeyHash([0x55; 20]),
                canonical.fee(),
                canonical.expiry_height(),
            ),
            ClaimPolicyMutation::Fee => (
                canonical.prevout().clone(),
                canonical.destination(),
                Zatoshis::from_u64(10_001).expect("mutated fee"),
                canonical.expiry_height(),
            ),
            ClaimPolicyMutation::Expiry => (
                canonical.prevout().clone(),
                canonical.destination(),
                canonical.fee(),
                BlockHeight::from_u32(1),
            ),
        };
        let request = TransparentSpendRequest::new(
            expected.contract(),
            outpoint,
            funding_output,
            destination,
            fee,
            expiry_height,
            canonical.consensus_branch_id(),
        )
        .expect("consensus-valid policy mutation");
        let transaction = build_claim_transaction(
            expected.contract(),
            &request,
            &claimant_key(),
            &fixture.secret,
        )
        .expect("signed policy mutation");
        let mut exact = Vec::new();
        transaction
            .write(&mut exact)
            .expect("canonical mutation bytes");
        PreparedClaimSubmissionV1::new(
            ClaimStepV1::FollowupZcash,
            *transaction.txid().as_ref(),
            exact,
        )
        .expect("well-formed policy mutation")
    }

    fn set_confirmed_claim(rpc: &FakeRpc, prepared: &PreparedClaimSubmissionV1) {
        rpc.edit(|state| {
            state.calls.clear();
            state.chain_infos.clear();
            state.canonical_block = BlockHash([0x22; 32]);
            state.discovery_enabled = true;
            state.unspent = None;
            state.transaction = Some(ZebraTransactionState::Confirmed {
                raw_transaction: prepared.exact_submission().to_vec(),
                block_hash: state.canonical_block,
                block_height: BlockHeight::from_u32(INCLUSION_HEIGHT),
                confirmations: TIP_HEIGHT - INCLUSION_HEIGHT + 1,
                in_active_chain: true,
            });
        });
    }

    fn canonical_tip() -> ZebraChainInfo {
        ZebraChainInfo::new(
            ZebraRpcChain::Test,
            BlockHeight::from_u32(TIP_HEIGHT),
            BlockHash([0x33; 32]),
            BranchId::Nu6_2,
        )
    }

    fn claimant_key() -> SecretKey {
        SecretKey::from_slice(&[1; 32]).expect("maker claimant key")
    }

    fn assert_exactly_one_send(rpc: &FakeRpc, prepared: &PreparedClaimSubmissionV1) {
        assert_eq!(rpc.submitted(), [prepared.exact_submission().to_vec()]);
        assert_eq!(
            rpc.calls()
                .iter()
                .filter(|call| call.as_str() == "send_raw_transaction")
                .count(),
            1,
            "post-send handling must never retry"
        );
    }

    fn funding_transaction(agreement: &ZecAgreementV1) -> Transaction {
        let expected = agreement.binding().expected_output();
        let key = SecretKey::from_slice(&[7; 32]).expect("fixed funding key");
        let public_key = PublicKey::from_secret_key(&Secp256k1::new(), &key);
        let owner_script: Script = TransparentAddress::from_pubkey(&public_key).script().into();
        let input_value =
            Zatoshis::from_u64(u64::from(expected.value()) + 20_000).expect("input value");
        let request = TransparentFundingRequest::new(
            vec![TransparentUtxo::new(
                OutPoint::new([0x77; 32], 0),
                TxOut::new(input_value, owner_script),
            )],
            public_key,
            expected.value(),
            Zatoshis::from_u64(1_000).expect("fee"),
            Zatoshis::from_u64(1_000).expect("change floor"),
            BlockHeight::from_u32(200),
            expected.consensus_branch_id(),
        )
        .expect("funding request");
        build_funding_transaction(expected.contract(), &request, &key)
            .expect("signed V5 funding transaction")
    }

    fn agreement() -> ZecAgreementV1 {
        agreement_with_secret([0x91; 32])
    }

    fn agreement_with_secret(secret: [u8; 32]) -> ZecAgreementV1 {
        agreement_with_direction_and_secret(SwapDirection::TakerSellsForeign, secret)
    }

    fn agreement_with_direction_and_secret(
        direction: SwapDirection,
        secret: [u8; 32],
    ) -> ZecAgreementV1 {
        let maker_secret = SecretKey::from_slice(&[1; 32]).expect("maker key");
        let taker_secret = SecretKey::from_slice(&[2; 32]).expect("taker key");
        let secp = Secp256k1::new();
        let maker_key = PublicKey::from_secret_key(&secp, &maker_secret).serialize();
        let taker_key = PublicKey::from_secret_key(&secp, &taker_secret).serialize();
        let (refund_key, claimant_key) = match direction {
            SwapDirection::TakerSellsForeign => (taker_key, maker_key),
            SwapDirection::TakerSellsLez => (maker_key, taker_key),
        };
        let refund_hash = pubkey_hash(&refund_key);
        let claimant_hash = pubkey_hash(&claimant_key);
        let secret_digest = Sha256::digest(secret).into();
        let contract = Bip199Contract::new(120, refund_hash, secret_digest, claimant_hash);
        let binding = ZecSwapBinding::new(
            ZecProfileId::DeterministicLocalV1,
            lez_zec_swap_sdk::ExpectedBip199Output::new(
                NetworkType::Regtest,
                BranchId::Nu6_2,
                Zatoshis::from_u64(100_000_000).expect("principal"),
                contract,
            ),
        )
        .expect("profile binding");
        let id = match direction {
            SwapDirection::TakerSellsForeign => "zebra-claim-funding-test",
            SwapDirection::TakerSellsLez => "zebra-refund-reverse-test",
        };
        let escrow_program = [1; 8];
        let onchain_id = derive_lez_swap_id_v1(id.as_bytes());
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
                LezChainIdentityV1::new(LezEnvironmentV1::DeterministicLocalV0_2, [8; 32], [7; 32]),
                escrow_program,
                LezAssetV1::Native {
                    authenticated_transfer_program_id: [2; 8],
                },
                42,
                derive_lez_metadata_account_v1(&escrow_program, &onchain_id),
                derive_lez_native_custody_account_v1(&escrow_program, &onchain_id),
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

    fn pubkey_hash(bytes: &[u8; 33]) -> [u8; 20] {
        match TransparentAddress::from_pubkey(&PublicKey::from_slice(bytes).expect("public key")) {
            TransparentAddress::PublicKeyHash(hash) => hash,
            TransparentAddress::ScriptHash(_) => unreachable!("public keys produce P2PKH"),
        }
    }

    #[tokio::test]
    async fn refund_preparation_is_role_and_policy_bound_in_both_directions() {
        for direction in [
            SwapDirection::TakerSellsForeign,
            SwapDirection::TakerSellsLez,
        ] {
            let (fixture, rpc, port, prepared) = prepared_refund_fixture(direction).await;
            assert_eq!(prepared.step(), RefundStepV1::Zcash);
            assert!(
                port.refund_validate_prepared(&fixture.agreement, &fixture.context, &prepared,)
                    .is_ok()
            );

            let wrong_role = ZebraRpcRefundPort::new(
                rpc.clone(),
                CanonicalRefundSigner {
                    participant: fixture.owner.other(),
                    key: refund_key(direction),
                },
                ZebraChainIdentity::deterministic_regtest_nu6_2(),
            );
            assert!(matches!(
                wrong_role
                    .prepare_refund(&fixture.agreement, &fixture.context)
                    .await,
                Err(ZebraClaimError::WrongRefundSignerRole { .. })
            ));

            let wrong_key_bytes = match direction {
                SwapDirection::TakerSellsForeign => [1; 32],
                SwapDirection::TakerSellsLez => [2; 32],
            };
            let wrong_key = ZebraRpcRefundPort::new(
                rpc,
                CanonicalRefundSigner {
                    participant: fixture.owner,
                    key: SecretKey::from_slice(&wrong_key_bytes).expect("wrong branch key"),
                },
                ZebraChainIdentity::deterministic_regtest_nu6_2(),
            );
            assert!(matches!(
                wrong_key
                    .prepare_refund(&fixture.agreement, &fixture.context)
                    .await,
                Err(ZebraClaimError::RefundSigner(_))
            ));
        }
    }

    #[tokio::test]
    async fn prepared_refund_observation_requires_exact_bytes_and_canonical_evidence() {
        let (fixture, rpc, port, prepared) =
            prepared_refund_fixture(SwapDirection::TakerSellsForeign).await;
        set_refund_absent(&rpc);
        assert_eq!(
            port.observe_prepared_refund(&fixture.agreement, &fixture.context, &prepared)
                .await
                .expect("stable absence"),
            RefundObservationV1::Absent,
        );

        rpc.edit(|state| {
            state.transaction = Some(ZebraTransactionState::Mempool {
                raw_transaction: prepared.exact_submission().to_vec(),
            });
        });
        assert_eq!(
            port.observe_prepared_refund(&fixture.agreement, &fixture.context, &prepared)
                .await
                .expect("mempool is not canonical evidence"),
            RefundObservationV1::Unstable,
        );

        set_confirmed_refund(&rpc, &prepared);
        let observed = port
            .observe_prepared_refund(&fixture.agreement, &fixture.context, &prepared)
            .await
            .expect("canonical refund observation");
        let RefundObservationV1::Confirmed(evidence) = observed else {
            panic!("confirmed exact refund must produce evidence")
        };
        assert_eq!(evidence.step(), RefundStepV1::Zcash);
        assert_eq!(
            evidence.observed_submission_id(),
            prepared.expected_submission_id()
        );
        assert_eq!(
            evidence.position(),
            ChainPosition::block_height(Chain::Zcash, u64::from(REFUND_TIP_HEIGHT)),
        );

        rpc.edit(|state| {
            let mut mutated = prepared.exact_submission().to_vec();
            mutated.push(0);
            state.transaction = Some(ZebraTransactionState::Mempool {
                raw_transaction: mutated,
            });
        });
        assert!(matches!(
            port.observe_prepared_refund(&fixture.agreement, &fixture.context, &prepared)
                .await,
            Err(ZebraClaimError::ObservedRefundBytesMismatch)
        ));
    }

    #[tokio::test]
    async fn refund_submission_replays_exact_bytes_and_classifies_every_outcome() {
        for (submission, expected) in [
            (
                Err(FakeError::RpcCode(-26)),
                RefundSubmitOutcomeV1::DefinitivelyRejected,
            ),
            (Err(FakeError::Transport), RefundSubmitOutcomeV1::Unknown),
        ] {
            let (fixture, rpc, port, prepared) =
                prepared_refund_fixture(SwapDirection::TakerSellsForeign).await;
            set_refund_absent(&rpc);
            rpc.edit(|state| state.submissions.push_back(submission));
            assert_eq!(
                port.submit_refund(&fixture.agreement, &fixture.context, &prepared)
                    .await
                    .expect("typed submission outcome"),
                expected,
            );
            assert_eq!(rpc.submitted(), [prepared.exact_submission().to_vec()]);
        }

        let (fixture, rpc, port, prepared) =
            prepared_refund_fixture(SwapDirection::TakerSellsForeign).await;
        set_refund_absent(&rpc);
        assert_eq!(
            port.observe_prepared_refund(&fixture.agreement, &fixture.context, &prepared)
                .await
                .expect("exact durable identity is absent before broadcast"),
            RefundObservationV1::Absent,
        );
        assert!(matches!(
            port.observe_refund_eligibility(&fixture.agreement, &fixture.context)
                .await
                .expect("fresh stable funding immediately before broadcast"),
            RefundEligibilityObservationV1::Canonical(_)
        ));
        let transaction_id = TxId::from_bytes(*prepared.expected_submission_id());
        rpc.edit(|state| state.submissions.push_back(Ok(transaction_id)));
        assert_eq!(
            port.submit_refund(&fixture.agreement, &fixture.context, &prepared)
                .await
                .expect("stable acceptance"),
            RefundSubmitOutcomeV1::Accepted,
        );
        assert_eq!(rpc.submitted(), [prepared.exact_submission().to_vec()]);
        let calls = rpc.calls();
        let observed_at = calls
            .iter()
            .position(|call| call == "transaction_state")
            .expect("prepared identity observation");
        let sent_at = calls
            .iter()
            .position(|call| call == "send_raw_transaction")
            .expect("exact broadcast");
        assert!(observed_at < sent_at, "observation must precede broadcast");

        let (fixture, rpc, port, prepared) =
            prepared_refund_fixture(SwapDirection::TakerSellsForeign).await;
        set_refund_absent(&rpc);
        let transaction_id = TxId::from_bytes(*prepared.expected_submission_id());
        rpc.edit(|state| {
            state.submissions.push_back(Ok(transaction_id));
            let before = ZebraChainInfo::new(
                ZebraRpcChain::Test,
                BlockHeight::from_u32(REFUND_TIP_HEIGHT),
                BlockHash([REFUND_TIP_HASH_BYTE; 32]),
                BranchId::Nu6_2,
            );
            let after = ZebraChainInfo::new(
                ZebraRpcChain::Test,
                BlockHeight::from_u32(REFUND_TIP_HEIGHT + 1),
                BlockHash([NEXT_REFUND_TIP_HASH_BYTE; 32]),
                BranchId::Nu6_2,
            );
            state.chain_info_overrides = VecDeque::from([Ok(before), Ok(after)]);
        });
        assert_eq!(
            port.submit_refund(&fixture.agreement, &fixture.context, &prepared)
                .await
                .expect("moving post-submit tip is unknown"),
            RefundSubmitOutcomeV1::Unknown,
        );
        assert_eq!(rpc.submitted(), [prepared.exact_submission().to_vec()]);
    }

    #[tokio::test]
    async fn failed_refund_broadcast_reconciles_exact_known_transaction() {
        for confirmed in [false, true] {
            let (fixture, rpc, port, prepared) =
                prepared_refund_fixture(SwapDirection::TakerSellsForeign).await;
            if confirmed {
                set_confirmed_refund(&rpc, &prepared);
            } else {
                rpc.edit(|state| {
                    state.calls.clear();
                    state.transaction = Some(ZebraTransactionState::Mempool {
                        raw_transaction: prepared.exact_submission().to_vec(),
                    });
                });
            }
            rpc.edit(|state| {
                state.submissions.push_back(Err(FakeError::RpcCode(-27)));
            });

            assert_eq!(
                port.submit_refund(&fixture.agreement, &fixture.context, &prepared)
                    .await
                    .expect("typed reconciled submission outcome"),
                RefundSubmitOutcomeV1::Accepted,
            );
            assert_eq!(rpc.submitted(), [prepared.exact_submission().to_vec()]);
            assert_eq!(
                rpc.calls()
                    .iter()
                    .filter(|call| call.as_str() == "transaction_state")
                    .count(),
                1,
                "one read-only reconciliation follows the failed send"
            );
        }
    }

    #[tokio::test]
    async fn failed_refund_broadcast_does_not_reconcile_a_moving_tip() {
        let (fixture, rpc, port, prepared) =
            prepared_refund_fixture(SwapDirection::TakerSellsForeign).await;
        rpc.edit(|state| {
            state.calls.clear();
            state.transaction = Some(ZebraTransactionState::Mempool {
                raw_transaction: prepared.exact_submission().to_vec(),
            });
            state.submissions.push_back(Err(FakeError::RpcCode(-27)));
            let before = ZebraChainInfo::new(
                ZebraRpcChain::Test,
                BlockHeight::from_u32(REFUND_TIP_HEIGHT),
                BlockHash([REFUND_TIP_HASH_BYTE; 32]),
                BranchId::Nu6_2,
            );
            let after = ZebraChainInfo::new(
                ZebraRpcChain::Test,
                BlockHeight::from_u32(REFUND_TIP_HEIGHT + 1),
                BlockHash([NEXT_REFUND_TIP_HASH_BYTE; 32]),
                BranchId::Nu6_2,
            );
            state.chain_info_overrides = VecDeque::from([Ok(before), Ok(after)]);
        });

        assert_eq!(
            port.submit_refund(&fixture.agreement, &fixture.context, &prepared)
                .await
                .expect("unstable duplicate remains a typed unknown outcome"),
            RefundSubmitOutcomeV1::Unknown,
        );
        assert_eq!(rpc.submitted(), [prepared.exact_submission().to_vec()]);
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn refund_eligibility_and_policy_mutations_fail_closed() {
        let (fixture, rpc, port, prepared) =
            prepared_refund_fixture(SwapDirection::TakerSellsForeign).await;
        for mutation in [
            RefundPolicyMutation::Outpoint,
            RefundPolicyMutation::Destination,
            RefundPolicyMutation::Fee,
            RefundPolicyMutation::Expiry,
        ] {
            let mutated = refund_policy_mutated_prepared(
                &fixture,
                SwapDirection::TakerSellsForeign,
                mutation,
            );
            assert!(
                port.observe_prepared_refund(&fixture.agreement, &fixture.context, &mutated)
                    .await
                    .is_err()
            );
        }
        let wrong_step = PreparedRefundSubmissionV1::new(
            RefundStepV1::Lez,
            *prepared.expected_submission_id(),
            prepared.exact_submission().to_vec(),
        )
        .expect("well-formed wrong-step refund");
        assert!(matches!(
            port.observe_prepared_refund(&fixture.agreement, &fixture.context, &wrong_step)
                .await,
            Err(ZebraClaimError::WrongRefundStep(RefundStepV1::Lez))
        ));

        let funding_output = fixture.funding.transparent_bundle().expect("funding").vout[0].clone();
        let claim_request = fixture
            .agreement
            .claim_spend_request(
                fixture.context.funding_outpoint().clone(),
                funding_output,
                BlockHeight::from_u32(REFUND_TIP_HEIGHT),
            )
            .expect("canonical opposite-branch request");
        let claim = build_claim_transaction(
            fixture.agreement.binding().expected_output().contract(),
            &claim_request,
            &claimant_key(),
            &[0x91; 32],
        )
        .expect("canonical opposite-branch spend");
        let mut exact_claim = Vec::new();
        claim.write(&mut exact_claim).expect("claim bytes");
        let wrong_branch = PreparedRefundSubmissionV1::new(
            RefundStepV1::Zcash,
            *claim.txid().as_ref(),
            exact_claim,
        )
        .expect("well-formed opposite-branch submission");
        assert!(matches!(
            port.observe_prepared_refund(&fixture.agreement, &fixture.context, &wrong_branch,)
                .await,
            Err(ZebraClaimError::WrongRefundSpendBranch)
        ));

        rpc.edit(|state| {
            state.discovery_enabled = false;
            state.transaction = None;
        });
        assert_eq!(
            port.observe_refund_eligibility(&fixture.agreement, &fixture.context)
                .await
                .expect("durable funding disappearance"),
            RefundEligibilityObservationV1::FundingUnavailable(RefundFundingWaitReasonV1::Reorged,),
        );

        let (fixture, rpc, port, _) =
            prepared_refund_fixture(SwapDirection::TakerSellsForeign).await;
        rpc.edit(|state| state.unspent = None);
        assert_eq!(
            port.observe_refund_eligibility(&fixture.agreement, &fixture.context)
                .await
                .expect("stable spent funding"),
            RefundEligibilityObservationV1::FundingUnavailable(RefundFundingWaitReasonV1::Spent),
        );

        let (fixture, rpc, port, _) =
            prepared_refund_fixture(SwapDirection::TakerSellsForeign).await;
        rpc.edit(|state| {
            let before = state.chain_infos.front().copied().expect("mature tip");
            let after = ZebraChainInfo::new(
                ZebraRpcChain::Test,
                BlockHeight::from_u32(REFUND_TIP_HEIGHT + 1),
                BlockHash([NEXT_REFUND_TIP_HASH_BYTE; 32]),
                BranchId::Nu6_2,
            );
            state.chain_info_overrides = VecDeque::from([Ok(before), Ok(after)]);
        });
        assert_eq!(
            port.observe_refund_eligibility(&fixture.agreement, &fixture.context)
                .await
                .expect("moving tip"),
            RefundEligibilityObservationV1::Unstable,
        );

        let (fixture, rpc, port, _) =
            prepared_refund_fixture(SwapDirection::TakerSellsForeign).await;
        rpc.edit(|state| {
            state
                .chain_info_overrides
                .push_back(Err(FakeError::Transport));
        });
        assert!(matches!(
            port.observe_refund_eligibility(&fixture.agreement, &fixture.context)
                .await,
            Err(ZebraClaimError::Rpc(FakeError::Transport))
        ));
    }

    #[tokio::test]
    async fn counterparty_refund_discovery_is_bounded_and_never_infers_scan_miss_as_absence() {
        let (fixture, rpc, _, prepared) =
            prepared_refund_fixture(SwapDirection::TakerSellsLez).await;
        let observer = ZebraRpcRefundPort::new(
            rpc.clone(),
            NeverRefundSigner,
            ZebraChainIdentity::deterministic_regtest_nu6_2(),
        );
        assert_eq!(
            observer
                .observe_counterparty_refund(&fixture.agreement, &fixture.context)
                .await
                .expect("stable unspent funding"),
            RefundObservationV1::Absent,
        );

        rpc.edit(|state| {
            state.discovery_enabled = true;
            state.unspent = None;
            state.transaction = None;
        });
        assert_eq!(
            observer
                .observe_counterparty_refund(&fixture.agreement, &fixture.context)
                .await
                .expect("bounded scan miss"),
            RefundObservationV1::Unstable,
        );

        rpc.edit(|state| {
            state.transaction = Some(ZebraTransactionState::Mempool {
                raw_transaction: prepared.exact_submission().to_vec(),
            });
        });
        assert_eq!(
            observer
                .observe_counterparty_refund(&fixture.agreement, &fixture.context)
                .await
                .expect("mempool refund"),
            RefundObservationV1::Unstable,
        );

        set_confirmed_refund(&rpc, &prepared);
        let bounded = ZebraRpcRefundPort::new(
            rpc.clone(),
            NeverRefundSigner,
            ZebraChainIdentity::deterministic_regtest_nu6_2(),
        )
        .with_counterparty_scan_blocks(1);
        assert_eq!(
            bounded
                .observe_counterparty_refund(&fixture.agreement, &fixture.context)
                .await
                .expect("exhausted historical horizon"),
            RefundObservationV1::Unstable,
        );
        assert!(matches!(
            observer
                .observe_counterparty_refund(&fixture.agreement, &fixture.context)
                .await
                .expect("canonical counterparty refund"),
            RefundObservationV1::Confirmed(_)
        ));

        set_refund_absent(&rpc);
        rpc.edit(|state| {
            state.unspent = None;
            state.mempool_overrides.push_back(Err(FakeError::Transport));
        });
        assert!(matches!(
            observer
                .observe_counterparty_refund(&fixture.agreement, &fixture.context)
                .await,
            Err(ZebraClaimError::Rpc(FakeError::Transport))
        ));
    }

    #[test]
    fn zebra_rpc_port_exposes_production_refund_boundary() {
        fn assert_refund_port<T: ZcashRefundPort>() {}

        assert_refund_port::<ZebraRpcRefundPort<FakeRpc, NeverRefundSigner>>();
    }
}
