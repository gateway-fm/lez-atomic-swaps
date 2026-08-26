use std::io::Cursor;

use async_trait::async_trait;
use lez_swap_core::Participant;
use lez_zec_swap_sdk::{
    CanonicalZcashOutputObservation, CanonicalZcashOutputRemoval, ExpectedBip199Output,
    FirstLockConfirmedEvidenceV1, FirstLockObservation, FirstLockStepV1, FirstLockTransitionError,
    MakerLockObservationV1, ObservationError, PreparedFirstLockSubmissionV1, ProfileError,
    TakerFirstLockObservationV1, ZcashFirstLockPort, ZcashMakerLockObservationPort,
    ZcashNodeRemovalSnapshot, ZcashNodeSnapshot, ZcashStableTip,
    ZcashTakerFirstLockObservationPort, ZecAgreementV1, ZecProfileId, ZecRefundProfile,
};
use zcash_primitives::transaction::{Transaction, TxVersion};
use zcash_protocol::{
    TxId,
    consensus::{BlockHeight, BranchId},
};

use crate::rpc::{ZebraChainIdentity, ZebraChainInfo, ZebraRpc, ZebraTransactionState};

/// Typed role-fixed Zebra funding owner and observer over a bounded RPC implementation.
#[derive(Clone, Debug)]
pub struct ZebraRpcSwapPort<R> {
    rpc: R,
    identity: ZebraChainIdentity,
    local_participant: Participant,
    counterparty_scan_blocks: u32,
}

impl<R> ZebraRpcSwapPort<R> {
    /// Binds one RPC implementation to an immutable chain identity and local role.
    #[must_use]
    pub const fn new(rpc: R, identity: ZebraChainIdentity, local_participant: Participant) -> Self {
        Self {
            rpc,
            identity,
            local_participant,
            counterparty_scan_blocks: ZecRefundProfile::for_id(ZecProfileId::PublicTestnetV1)
                .zcash_refund_blocks()
                .saturating_add(1),
        }
    }

    /// Replaces the complete signed-anchor scan bound used for remote funding discovery.
    ///
    /// Values below the agreement profile inclusive horizon are accepted as
    /// configuration but make discovery return Unstable; they can never turn
    /// an incomplete scan into absence.
    #[must_use]
    pub fn with_counterparty_scan_blocks(mut self, maximum: u32) -> Self {
        self.counterparty_scan_blocks = maximum.max(1);
        self
    }

    /// Configured immutable chain identity.
    #[must_use]
    pub const fn identity(&self) -> ZebraChainIdentity {
        self.identity
    }

    /// Typed transport used by this adapter.
    #[must_use]
    pub const fn rpc(&self) -> &R {
        &self.rpc
    }

    /// Role fixed to this adapter instance by the local actor.
    #[must_use]
    pub const fn local_participant(&self) -> Participant {
        self.local_participant
    }
}

/// A rejected prepared transaction, node identity, snapshot, or submission result.
#[derive(Debug, thiserror::Error)]
pub enum ZebraFirstLockError<E: std::error::Error + Send + Sync + 'static> {
    /// This adapter accepts only the Zcash funding step.
    #[error("Zebra first-lock adapter received wrong step {0:?}")]
    WrongStep(FirstLockStepV1),
    /// The role fixed to this adapter is not the local role required by the operation.
    #[error(
        "Zebra funding operation requires local role {expected:?}; configured role is {actual:?}"
    )]
    WrongRole {
        /// Local role required by the owner or observer operation.
        expected: Participant,
        /// Role fixed to this adapter instance.
        actual: Participant,
    },
    /// The observation trait does not match the agreement-assigned Zcash funder.
    #[error("Zebra funding observer expected funder {expected:?}; agreement assigns {actual:?}")]
    WrongObservedFunder {
        /// Funder required by this observation boundary.
        expected: Participant,
        /// Funder assigned by the accepted agreement.
        actual: Participant,
    },
    /// The configured network differs from the accepted agreement.
    #[error("configured Zebra network differs from the accepted agreement")]
    ConfiguredNetworkMismatch,
    /// The configured branch differs from the accepted agreement.
    #[error("configured Zebra branch differs from the accepted agreement")]
    ConfiguredConsensusBranchMismatch,
    /// The accepted profile rejected its configured network/branch.
    #[error("configured Zebra identity violates the accepted profile: {0}")]
    Profile(#[source] ProfileError),
    /// Canonical V5 transaction decoding failed.
    #[error("prepared Zcash first-lock transaction is malformed: {0}")]
    MalformedSubmission(#[source] std::io::Error),
    /// Bytes remained after decoding exactly one transaction.
    #[error("prepared Zcash first-lock transaction contains trailing bytes")]
    TrailingSubmissionBytes,
    /// Canonical transaction bytes from a block or mempool inventory were malformed.
    #[error("discovered Zcash funding transaction is malformed: {0}")]
    MalformedDiscoveredTransaction(#[source] std::io::Error),
    /// Bytes remained after decoding one inventoried transaction.
    #[error("discovered Zcash funding transaction contains trailing bytes")]
    TrailingDiscoveredTransactionBytes,
    /// Inventoried transaction bytes differ from the inventory identity.
    #[error("discovered Zcash funding transaction ID differs from block or mempool inventory")]
    DiscoveredTransactionIdMismatch,
    /// A hash-addressed block or listed transaction did not preserve its inventory context.
    #[error("Zebra canonical block transaction inventory is inconsistent")]
    BlockInventoryMismatch,
    /// A listed mempool transaction was not retrievable by its exact identity.
    #[error("Zebra mempool transaction inventory is inconsistent")]
    MempoolInventoryMismatch,
    /// More than one canonical transaction contains the exact agreement-bound output.
    #[error("multiple canonical Zcash transactions match the agreement-bound funding output")]
    AmbiguousFundingCandidates,
    /// A durable predecessor observation lies outside the signed funding scan window.
    #[error("durable Zcash funding observation lies outside the signed scan window")]
    PreviousObservationOutsideFundingWindow,
    /// Signed funding-anchor arithmetic was inconsistent after agreement validation.
    #[error("accepted Zcash agreement has an invalid funding anchor")]
    InvalidFundingAnchor,
    /// A transaction version other than V5 was supplied.
    #[error("prepared Zcash first-lock transaction is not V5")]
    WrongTransactionVersion,
    /// Canonical V5 identity differs from the durable expected identity.
    #[error("prepared Zcash first-lock transaction ID differs from durable identity")]
    ExpectedTransactionIdMismatch,
    /// The expected BIP-199 output is absent at vout zero.
    #[error("prepared Zcash first-lock transaction has no output at index zero")]
    MissingExpectedOutput,
    /// Vout zero has the wrong agreement-bound amount.
    #[error("prepared Zcash first-lock output value differs from the agreement")]
    OutputValueMismatch,
    /// Vout zero has the wrong agreement-bound P2SH script.
    #[error("prepared Zcash first-lock output script differs from the agreement")]
    OutputScriptMismatch,
    /// A typed Zebra RPC operation failed.
    #[error("typed Zebra RPC operation failed: {0}")]
    Rpc(#[source] E),
    /// The RPC chain spelling differs from the configured identity.
    #[error("Zebra RPC chain differs from configured identity")]
    RpcChainMismatch,
    /// The RPC tip branch differs from the configured identity.
    #[error("Zebra RPC consensus branch differs from configured identity")]
    RpcConsensusBranchMismatch,
    /// Height zero differs from the configured immutable genesis hash.
    #[error("Zebra genesis block differs from configured identity")]
    GenesisMismatch,
    /// Raw bytes returned by Zebra differ from the exact durable submission.
    #[error("Zebra transaction bytes differ from the exact durable submission")]
    ObservedRawTransactionMismatch,
    /// The SDK canonical output validator rejected the RPC snapshot.
    #[error("Zebra canonical output validation failed: {0}")]
    Observation(#[source] ObservationError),
    /// The SDK rejected the primitive envelope derived from the canonical snapshot.
    #[error("Zebra canonical first-lock evidence was invalid: {0}")]
    Evidence(#[source] FirstLockTransitionError),
    /// Zebra returned a different ID after accepting exact bytes.
    #[error("Zebra returned a transaction ID different from the submitted V5 identity")]
    SubmittedTransactionIdMismatch,
    /// A confirmed transport state was missing an internally required field.
    #[error("typed Zebra transaction state was internally incomplete")]
    IncompleteRpcSnapshot,
    /// The best-chain tip moved while submission identity was bracketed.
    #[error("Zebra best-chain tip changed during first-lock submission")]
    UnstableTipDuringSubmission,
}

struct ValidatedSubmission {
    transaction_id: TxId,
}

struct DiscoveredFunding {
    transaction_id: TxId,
    raw_transaction: Vec<u8>,
    block_hash: zcash_primitives::block::BlockHash,
    block_height: BlockHeight,
}

struct FundingScan {
    matching_mempool_transaction: bool,
    candidates: Vec<DiscoveredFunding>,
    previous_block_hash: Option<zcash_primitives::block::BlockHash>,
}

enum FundingDiscovery {
    Absent,
    Unstable,
    Canonical(Box<CanonicalZcashOutputObservation>),
    Removed(Box<CanonicalZcashOutputRemoval>),
    Replaced {
        removed: Box<CanonicalZcashOutputRemoval>,
        canonical: Box<CanonicalZcashOutputObservation>,
    },
}

#[async_trait]
impl<R> ZcashFirstLockPort for ZebraRpcSwapPort<R>
where
    R: ZebraRpc,
{
    type Error = ZebraFirstLockError<R::Error>;

    async fn observe_first_lock(
        &self,
        agreement: &ZecAgreementV1,
        submission: &PreparedFirstLockSubmissionV1,
    ) -> Result<FirstLockObservation, Self::Error> {
        let validated = self.validate_submission(agreement, submission)?;
        let before = self
            .rpc
            .chain_info()
            .await
            .map_err(ZebraFirstLockError::Rpc)?;
        self.validate_rpc_identity(before)?;
        self.validate_genesis().await?;

        let raw = self
            .rpc
            .raw_transaction(validated.transaction_id)
            .await
            .map_err(ZebraFirstLockError::Rpc)?;
        let state = match raw.as_ref() {
            Some(_) => self
                .rpc
                .transaction_state(validated.transaction_id)
                .await
                .map_err(ZebraFirstLockError::Rpc)?,
            None => None,
        };
        let canonical_block_hash = match &state {
            Some(ZebraTransactionState::Confirmed { block_height, .. }) => Some(
                self.rpc
                    .block_hash(*block_height)
                    .await
                    .map_err(ZebraFirstLockError::Rpc)?,
            ),
            Some(ZebraTransactionState::Mempool { .. }) | None => None,
        };
        let after = self
            .rpc
            .chain_info()
            .await
            .map_err(ZebraFirstLockError::Rpc)?;
        self.validate_rpc_identity(after)?;
        if before.tip_hash() != after.tip_hash() || before.tip_height() != after.tip_height() {
            return Ok(FirstLockObservation::Unstable);
        }

        let Some(raw) = raw else {
            return Ok(FirstLockObservation::Absent);
        };
        let Some(state) = state else {
            return Ok(FirstLockObservation::Unstable);
        };
        match state {
            ZebraTransactionState::Mempool { raw_transaction } => {
                require_exact_raw::<R::Error>(&raw_transaction, submission.exact_submission())?;
                require_exact_raw::<R::Error>(&raw_transaction, &raw)?;
                Ok(FirstLockObservation::Unstable)
            }
            ZebraTransactionState::Confirmed {
                raw_transaction,
                block_hash,
                block_height,
                confirmations,
                in_active_chain,
            } => {
                require_exact_raw::<R::Error>(&raw_transaction, submission.exact_submission())?;
                require_exact_raw::<R::Error>(&raw_transaction, &raw)?;
                let snapshot = ZcashNodeSnapshot::new(
                    self.identity.network(),
                    self.identity.consensus_branch_id(),
                    in_active_chain,
                    block_hash,
                    canonical_block_hash.ok_or(ZebraFirstLockError::IncompleteRpcSnapshot)?,
                    block_height,
                    ZcashStableTip::new(
                        before.tip_hash(),
                        before.tip_height(),
                        after.tip_hash(),
                        after.tip_height(),
                    ),
                    validated.transaction_id,
                    raw_transaction,
                    0,
                    confirmations,
                );
                let canonical = CanonicalZcashOutputObservation::validate(
                    agreement.binding().expected_output(),
                    &snapshot,
                )
                .map_err(ZebraFirstLockError::Observation)?;
                let required = ZecRefundProfile::for_id(agreement.binding().profile_id())
                    .zcash_confirmations();
                if canonical.confirmations().get() < required {
                    Ok(FirstLockObservation::Unstable)
                } else {
                    let evidence = FirstLockConfirmedEvidenceV1::new(
                        submission.step(),
                        *submission.expected_submission_id(),
                        canonical.transaction_id().to_string(),
                        canonical.confirmations().get(),
                    )
                    .map_err(ZebraFirstLockError::Evidence)?;
                    Ok(FirstLockObservation::Confirmed(evidence))
                }
            }
        }
    }

    async fn submit_first_lock(
        &self,
        agreement: &ZecAgreementV1,
        submission: &PreparedFirstLockSubmissionV1,
    ) -> Result<(), Self::Error> {
        let validated = self.validate_submission(agreement, submission)?;
        let before = self
            .rpc
            .chain_info()
            .await
            .map_err(ZebraFirstLockError::Rpc)?;
        self.validate_rpc_identity(before)?;
        self.validate_genesis().await?;
        let submitted = self
            .rpc
            .send_raw_transaction(submission.exact_submission())
            .await
            .map_err(ZebraFirstLockError::Rpc)?;
        let after = self
            .rpc
            .chain_info()
            .await
            .map_err(ZebraFirstLockError::Rpc)?;
        self.validate_rpc_identity(after)?;
        self.validate_genesis().await?;
        if before.tip_hash() != after.tip_hash() || before.tip_height() != after.tip_height() {
            return Err(ZebraFirstLockError::UnstableTipDuringSubmission);
        }
        if submitted != validated.transaction_id {
            return Err(ZebraFirstLockError::SubmittedTransactionIdMismatch);
        }
        Ok(())
    }
}

#[async_trait]
impl<R> ZcashTakerFirstLockObservationPort for ZebraRpcSwapPort<R>
where
    R: ZebraRpc,
{
    type Error = ZebraFirstLockError<R::Error>;

    async fn observe_taker_first_lock(
        &self,
        agreement: &ZecAgreementV1,
        previous: Option<&CanonicalZcashOutputObservation>,
    ) -> Result<TakerFirstLockObservationV1, Self::Error> {
        self.validate_observer_context(agreement, Participant::Maker, Participant::Taker)?;
        Ok(match self.discover_funding(agreement, previous).await? {
            FundingDiscovery::Absent => TakerFirstLockObservationV1::Absent,
            FundingDiscovery::Unstable => TakerFirstLockObservationV1::Unstable,
            FundingDiscovery::Canonical(canonical) => {
                TakerFirstLockObservationV1::CanonicalZcash(canonical)
            }
            FundingDiscovery::Removed(removed) => {
                TakerFirstLockObservationV1::ZcashRemoved(removed)
            }
            FundingDiscovery::Replaced { removed, canonical } => {
                TakerFirstLockObservationV1::ZcashReplaced { removed, canonical }
            }
        })
    }
}

#[async_trait]
impl<R> ZcashMakerLockObservationPort for ZebraRpcSwapPort<R>
where
    R: ZebraRpc,
{
    type Error = ZebraFirstLockError<R::Error>;

    async fn observe_maker_lock(
        &self,
        agreement: &ZecAgreementV1,
    ) -> Result<MakerLockObservationV1, Self::Error> {
        self.validate_observer_context(agreement, Participant::Taker, Participant::Maker)?;
        Ok(match self.discover_funding(agreement, None).await? {
            FundingDiscovery::Absent => MakerLockObservationV1::Absent,
            FundingDiscovery::Unstable => MakerLockObservationV1::Unstable,
            FundingDiscovery::Canonical(canonical) => {
                let transaction_id = canonical.transaction_id();
                let evidence = FirstLockConfirmedEvidenceV1::new(
                    FirstLockStepV1::ZcashFund,
                    *transaction_id.as_ref(),
                    transaction_id.to_string(),
                    canonical.confirmations().get(),
                )
                .map_err(ZebraFirstLockError::Evidence)?;
                MakerLockObservationV1::Confirmed(evidence)
            }
            FundingDiscovery::Removed(_) | FundingDiscovery::Replaced { .. } => {
                unreachable!("maker-lock discovery has no predecessor observation")
            }
        })
    }
}

impl<R> ZebraRpcSwapPort<R>
where
    R: ZebraRpc,
{
    async fn discover_funding(
        &self,
        agreement: &ZecAgreementV1,
        previous: Option<&CanonicalZcashOutputObservation>,
    ) -> Result<FundingDiscovery, ZebraFirstLockError<R::Error>> {
        self.validate_agreement_identity(agreement)?;
        let before = self
            .rpc
            .chain_info()
            .await
            .map_err(ZebraFirstLockError::Rpc)?;
        self.validate_rpc_identity(before)?;
        self.validate_genesis().await?;

        let profile = ZecRefundProfile::for_id(agreement.binding().profile_id());
        let anchor = agreement
            .zcash_refund_at_height()
            .checked_sub(profile.zcash_refund_blocks())
            .ok_or(ZebraFirstLockError::InvalidFundingAnchor)?;
        let tip_height = u32::from(before.tip_height());
        if let Some(previous) = previous {
            let previous_height = u32::from(previous.block_height());
            if previous_height < anchor {
                return Err(ZebraFirstLockError::PreviousObservationOutsideFundingWindow);
            }
            if previous_height > tip_height {
                let _ = self.sample_validated_tip().await?;
                return Ok(FundingDiscovery::Unstable);
            }
        }
        let required_profile_horizon = profile
            .zcash_refund_blocks()
            .checked_add(1)
            .ok_or(ZebraFirstLockError::InvalidFundingAnchor)?;
        if self.counterparty_scan_blocks < required_profile_horizon {
            let _ = self.sample_validated_tip().await?;
            return Ok(FundingDiscovery::Unstable);
        }
        let Some(scan_blocks) = tip_height
            .checked_sub(anchor)
            .and_then(|distance| distance.checked_add(1))
        else {
            let _ = self.sample_validated_tip().await?;
            return Ok(FundingDiscovery::Unstable);
        };
        if scan_blocks > self.counterparty_scan_blocks {
            let _ = self.sample_validated_tip().await?;
            return Ok(FundingDiscovery::Unstable);
        }

        let mut scan = self
            .scan_funding_range(agreement, previous, anchor, tip_height)
            .await?;
        let after = self.sample_validated_tip().await?;
        if !same_tip(before, after) || scan.matching_mempool_transaction {
            return Ok(FundingDiscovery::Unstable);
        }
        if scan.candidates.len() > 1 {
            return Err(ZebraFirstLockError::AmbiguousFundingCandidates);
        }
        let canonical = scan
            .candidates
            .pop()
            .map(|candidate| self.validate_discovered_funding(agreement, candidate, before, after))
            .transpose()?;
        let required = agreement
            .coordinator()
            .required_confirmations(agreement.lez_claimant());
        if canonical.as_ref().is_some_and(|value| {
            !meets_required_confirmation_depth(value.confirmations().get(), required)
        }) {
            return Ok(FundingDiscovery::Unstable);
        }
        self.reconcile_discovered_funding(
            previous,
            scan.previous_block_hash,
            canonical,
            before,
            after,
        )
    }

    async fn scan_funding_range(
        &self,
        agreement: &ZecAgreementV1,
        previous: Option<&CanonicalZcashOutputObservation>,
        anchor: u32,
        tip_height: u32,
    ) -> Result<FundingScan, ZebraFirstLockError<R::Error>> {
        let expected = agreement.binding().expected_output();
        let mut matching_mempool_transaction = false;
        for transaction_id in self
            .rpc
            .mempool_transaction_ids()
            .await
            .map_err(ZebraFirstLockError::Rpc)?
        {
            let raw_transaction = self
                .rpc
                .raw_transaction(transaction_id)
                .await
                .map_err(ZebraFirstLockError::Rpc)?
                .ok_or(ZebraFirstLockError::MempoolInventoryMismatch)?;
            matching_mempool_transaction |= transaction_matches_expected_output::<R::Error>(
                &raw_transaction,
                transaction_id,
                self.identity.consensus_branch_id(),
                expected,
            )?;
        }

        let mut candidates = Vec::new();
        let mut previous_block_hash = None;
        for height in anchor..=tip_height {
            let height = BlockHeight::from_u32(height);
            let block_hash = self
                .rpc
                .block_hash(height)
                .await
                .map_err(ZebraFirstLockError::Rpc)?;
            if previous.is_some_and(|value| value.block_height() == height) {
                previous_block_hash = Some(block_hash);
            }
            let block = self
                .rpc
                .canonical_block(block_hash)
                .await
                .map_err(ZebraFirstLockError::Rpc)?;
            if block.block_hash() != block_hash || block.block_height() != height {
                return Err(ZebraFirstLockError::BlockInventoryMismatch);
            }
            for transaction_id in block.transaction_ids() {
                let raw_transaction = self
                    .rpc
                    .block_transaction(*transaction_id, block_hash)
                    .await
                    .map_err(ZebraFirstLockError::Rpc)?
                    .ok_or(ZebraFirstLockError::BlockInventoryMismatch)?;
                if transaction_matches_expected_output::<R::Error>(
                    &raw_transaction,
                    *transaction_id,
                    self.identity.consensus_branch_id(),
                    expected,
                )? {
                    candidates.push(DiscoveredFunding {
                        transaction_id: *transaction_id,
                        raw_transaction,
                        block_hash,
                        block_height: height,
                    });
                }
            }
        }
        Ok(FundingScan {
            matching_mempool_transaction,
            candidates,
            previous_block_hash,
        })
    }

    fn reconcile_discovered_funding(
        &self,
        previous: Option<&CanonicalZcashOutputObservation>,
        previous_block_hash: Option<zcash_primitives::block::BlockHash>,
        canonical: Option<CanonicalZcashOutputObservation>,
        before: ZebraChainInfo,
        after: ZebraChainInfo,
    ) -> Result<FundingDiscovery, ZebraFirstLockError<R::Error>> {
        let Some(previous) = previous else {
            return Ok(canonical.map_or(FundingDiscovery::Absent, |value| {
                FundingDiscovery::Canonical(Box::new(value))
            }));
        };
        let current_previous_hash = previous_block_hash
            .ok_or(ZebraFirstLockError::PreviousObservationOutsideFundingWindow)?;
        if current_previous_hash == previous.block_hash() {
            return match canonical {
                Some(current)
                    if current.transaction_id() == previous.transaction_id()
                        && current.block_hash() == previous.block_hash()
                        && current.block_height() == previous.block_height() =>
                {
                    Ok(FundingDiscovery::Canonical(Box::new(current)))
                }
                Some(_) | None => Err(ZebraFirstLockError::BlockInventoryMismatch),
            };
        }

        let removal = CanonicalZcashOutputRemoval::validate(
            previous,
            &ZcashNodeRemovalSnapshot::new(
                self.identity.network(),
                self.identity.consensus_branch_id(),
                current_previous_hash,
                stable_tip(before, after),
            ),
        )
        .map_err(ZebraFirstLockError::Observation)?;
        Ok(match canonical {
            Some(canonical) => FundingDiscovery::Replaced {
                removed: Box::new(removal),
                canonical: Box::new(canonical),
            },
            None => FundingDiscovery::Removed(Box::new(removal)),
        })
    }

    fn validate_discovered_funding(
        &self,
        agreement: &ZecAgreementV1,
        candidate: DiscoveredFunding,
        before: ZebraChainInfo,
        after: ZebraChainInfo,
    ) -> Result<CanonicalZcashOutputObservation, ZebraFirstLockError<R::Error>> {
        let confirmations = u32::from(before.tip_height())
            .checked_sub(u32::from(candidate.block_height))
            .and_then(|distance| distance.checked_add(1))
            .ok_or(ZebraFirstLockError::BlockInventoryMismatch)?;
        CanonicalZcashOutputObservation::validate(
            agreement.binding().expected_output(),
            &ZcashNodeSnapshot::new(
                self.identity.network(),
                self.identity.consensus_branch_id(),
                true,
                candidate.block_hash,
                candidate.block_hash,
                candidate.block_height,
                stable_tip(before, after),
                candidate.transaction_id,
                candidate.raw_transaction,
                0,
                confirmations,
            ),
        )
        .map_err(ZebraFirstLockError::Observation)
    }

    fn validate_observer_context(
        &self,
        agreement: &ZecAgreementV1,
        expected_local: Participant,
        expected_funder: Participant,
    ) -> Result<(), ZebraFirstLockError<R::Error>> {
        if self.local_participant != expected_local {
            return Err(ZebraFirstLockError::WrongRole {
                expected: expected_local,
                actual: self.local_participant,
            });
        }
        let actual_funder = agreement.lez_claimant();
        if actual_funder != expected_funder {
            return Err(ZebraFirstLockError::WrongObservedFunder {
                expected: expected_funder,
                actual: actual_funder,
            });
        }
        self.validate_agreement_identity(agreement)
    }

    fn validate_agreement_identity(
        &self,
        agreement: &ZecAgreementV1,
    ) -> Result<(), ZebraFirstLockError<R::Error>> {
        let expected = agreement.binding().expected_output();
        if self.identity.network() != expected.network() {
            return Err(ZebraFirstLockError::ConfiguredNetworkMismatch);
        }
        if self.identity.consensus_branch_id() != expected.consensus_branch_id() {
            return Err(ZebraFirstLockError::ConfiguredConsensusBranchMismatch);
        }
        ZecRefundProfile::for_id(agreement.binding().profile_id())
            .validate_consensus(self.identity.network(), self.identity.consensus_branch_id())
            .map_err(ZebraFirstLockError::Profile)
    }

    async fn sample_validated_tip(&self) -> Result<ZebraChainInfo, ZebraFirstLockError<R::Error>> {
        let info = self
            .rpc
            .chain_info()
            .await
            .map_err(ZebraFirstLockError::Rpc)?;
        self.validate_rpc_identity(info)?;
        Ok(info)
    }

    fn validate_submission(
        &self,
        agreement: &ZecAgreementV1,
        submission: &PreparedFirstLockSubmissionV1,
    ) -> Result<ValidatedSubmission, ZebraFirstLockError<R::Error>> {
        if submission.step() != FirstLockStepV1::ZcashFund {
            return Err(ZebraFirstLockError::WrongStep(submission.step()));
        }
        let expected_funder = agreement.lez_claimant();
        if self.local_participant != expected_funder {
            return Err(ZebraFirstLockError::WrongRole {
                expected: expected_funder,
                actual: self.local_participant,
            });
        }
        self.validate_agreement_identity(agreement)?;

        let expected = agreement.binding().expected_output();
        let mut cursor = Cursor::new(submission.exact_submission());
        let transaction = Transaction::read(&mut cursor, expected.consensus_branch_id())
            .map_err(ZebraFirstLockError::MalformedSubmission)?;
        let exact_length = u64::try_from(submission.exact_submission().len())
            .map_err(|_| ZebraFirstLockError::TrailingSubmissionBytes)?;
        if cursor.position() != exact_length {
            return Err(ZebraFirstLockError::TrailingSubmissionBytes);
        }
        if transaction.version() != TxVersion::V5 {
            return Err(ZebraFirstLockError::WrongTransactionVersion);
        }
        let transaction_id = transaction.txid();
        if transaction_id.as_ref() != submission.expected_submission_id() {
            return Err(ZebraFirstLockError::ExpectedTransactionIdMismatch);
        }
        let output = transaction
            .transparent_bundle()
            .and_then(|bundle| bundle.vout.first())
            .ok_or(ZebraFirstLockError::MissingExpectedOutput)?;
        if output.value() != expected.value() {
            return Err(ZebraFirstLockError::OutputValueMismatch);
        }
        if output.script_pubkey().0.0 != expected.contract().p2sh_script_pubkey() {
            return Err(ZebraFirstLockError::OutputScriptMismatch);
        }
        Ok(ValidatedSubmission { transaction_id })
    }

    fn validate_rpc_identity(
        &self,
        info: ZebraChainInfo,
    ) -> Result<(), ZebraFirstLockError<R::Error>> {
        if info.rpc_chain() != self.identity.rpc_chain() {
            return Err(ZebraFirstLockError::RpcChainMismatch);
        }
        if info.consensus_branch_id() != self.identity.consensus_branch_id() {
            return Err(ZebraFirstLockError::RpcConsensusBranchMismatch);
        }
        Ok(())
    }

    async fn validate_genesis(&self) -> Result<(), ZebraFirstLockError<R::Error>> {
        let genesis = self
            .rpc
            .block_hash(BlockHeight::from_u32(0))
            .await
            .map_err(ZebraFirstLockError::Rpc)?;
        if genesis != self.identity.genesis_hash() {
            return Err(ZebraFirstLockError::GenesisMismatch);
        }
        Ok(())
    }
}

fn transaction_matches_expected_output<E: std::error::Error + Send + Sync + 'static>(
    raw_transaction: &[u8],
    expected_transaction_id: TxId,
    consensus_branch_id: BranchId,
    expected: &ExpectedBip199Output,
) -> Result<bool, ZebraFirstLockError<E>> {
    let mut cursor = Cursor::new(raw_transaction);
    let transaction = Transaction::read(&mut cursor, consensus_branch_id)
        .map_err(ZebraFirstLockError::MalformedDiscoveredTransaction)?;
    let exact_length = u64::try_from(raw_transaction.len())
        .map_err(|_| ZebraFirstLockError::TrailingDiscoveredTransactionBytes)?;
    if cursor.position() != exact_length {
        return Err(ZebraFirstLockError::TrailingDiscoveredTransactionBytes);
    }
    if transaction.txid() != expected_transaction_id {
        return Err(ZebraFirstLockError::DiscoveredTransactionIdMismatch);
    }
    Ok(transaction
        .transparent_bundle()
        .and_then(|bundle| bundle.vout.first())
        .is_some_and(|output| {
            output.value() == expected.value()
                && output.script_pubkey().0.0 == expected.contract().p2sh_script_pubkey()
        }))
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

const fn meets_required_confirmation_depth(actual: u32, required: u32) -> bool {
    actual >= required
}

fn require_exact_raw<E: std::error::Error + Send + Sync + 'static>(
    actual: &[u8],
    expected: &[u8],
) -> Result<(), ZebraFirstLockError<E>> {
    if actual == expected {
        Ok(())
    } else {
        Err(ZebraFirstLockError::ObservedRawTransactionMismatch)
    }
}

#[cfg(test)]
mod tests {
    use super::meets_required_confirmation_depth;

    #[test]
    fn signed_confirmation_depth_rejects_under_depth_funding() {
        assert!(!meets_required_confirmation_depth(9, 10));
        assert!(meets_required_confirmation_depth(10, 10));
        assert!(meets_required_confirmation_depth(11, 10));
    }
}
