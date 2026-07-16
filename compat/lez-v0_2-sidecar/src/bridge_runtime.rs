use std::sync::Arc;

use borsh::BorshDeserialize as _;
use common::{block::Block, transaction::LeeTransaction};
use lez_bridge_protocol::{
    AccountIds, ChainPosition, ChainTip, ClassifyFinalizedWitnessedClaimResult,
    CompleteWitnessedClaimRequest, CompleteWitnessedClaimResult, DiscoveryWindow,
    EscrowMetadataFacts, EscrowObservationTarget, EscrowState, FundingFoundFacts,
    FundingObservation, Hex32, InitializationFoundFacts, InitializationObservation,
    MAX_DISCOVERY_BLOCKS, NativeClaimInstructionFacts, NativeCustodyFacts,
    NativeFundInstructionFacts, NativeInitializeInstructionFacts, ObserveEscrowRequest,
    ObserveEscrowResult, ObserveFinalizedWitnessedClaimRequest,
    ObserveFinalizedWitnessedClaimResult, ObserveFinalizedWitnessedFundingRequest,
    ObserveFinalizedWitnessedFundingResult, ObserveRevealingClaimRequest,
    ObserveRevealingClaimResult, ObserveWitnessedEscrowRequest, ObserveWitnessedEscrowResult,
    ObservedTransactionFacts, PrepareWitnessedClaimRequest, PrepareWitnessedClaimResult,
    PreparedTransaction, RevealingClaimFoundFacts, RevealingClaimObservation,
    RevealingClaimObservationTarget, RevealingPreimage, RuntimeDescriptor, SubmissionOutcome,
    SubmitTransactionRequest, SubmitTransactionResult, TransactionId, WitnessedEscrowMetadataFacts,
    WitnessedFundingFoundFacts, WitnessedFundingObservation, WitnessedInitializationFoundFacts,
    WitnessedInitializationObservation, WitnessedNativeInitializeInstructionFacts,
};
use lez_zec_escrow_v02::{ClaimAuthority, EscrowMetadata, EscrowStatus};
use nssa::{AccountId, PublicKey, PublicTransaction};
use sha2::{Digest as _, Sha256};

use crate::{
    FinalizedIndexerApi, FinalizedWitnessedClaimObserver, FinalizedWitnessedFundingObserver,
    HealthProbe, NativeEscrowPlanner, NativePrepareError, OfficialNativeEscrowFacts,
    OfficialNodeRpc, RuntimeBoundaryError, ZecEscrowInstruction, compute_custody_pda,
    compute_metadata_pda, decode_prepared_for_signer, prepared_from_transaction,
    program_id_from_hex, program_id_to_hex,
};

/// Fail-closed failures at the `PoC` bridge observation and submission boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum BridgeRuntimeError {
    /// The planner rejected role, runtime, terms, key, nonce, or exact bytes.
    #[error("official v0.2 planner rejected the bridge operation")]
    Planner,
    /// A required official sequencer fact was unavailable or inconsistent.
    #[error("official v0.2 sequencer observation is unavailable")]
    Unavailable,
    /// The canonical tip moved while a multi-RPC observation was assembled.
    #[error("official v0.2 sequencer tip moved during observation")]
    MovingTip,
    /// Official block, transaction, instruction, or account facts were invalid.
    #[error("official v0.2 sequencer returned invalid observation facts")]
    InvalidObservation,
    /// More than one canonical transaction matched a terms-based slot.
    #[error("official v0.2 terms discovery was ambiguous")]
    AmbiguousDiscovery,
    /// One canonical terms match conflicted with the expected signed transcript.
    #[error("official v0.2 terms discovery conflicted with the signed transcript")]
    ConflictingDiscovery,
    /// The refund bridge slice has not been implemented.
    #[error("native refund is unavailable in the progressive PoC slice")]
    RefundUnavailable,
    /// Submission could have reached the sequencer but no acknowledgement is known.
    #[error("official v0.2 transaction submission outcome is unknown")]
    UnknownSubmissionOutcome,
}

impl From<NativePrepareError> for BridgeRuntimeError {
    fn from(_value: NativePrepareError) -> Self {
        Self::Planner
    }
}

impl From<RuntimeBoundaryError> for BridgeRuntimeError {
    fn from(value: RuntimeBoundaryError) -> Self {
        match value {
            RuntimeBoundaryError::InconsistentSnapshot => Self::MovingTip,
            RuntimeBoundaryError::NodeUnavailable => Self::Unavailable,
            RuntimeBoundaryError::WrongIncludedTransaction
            | RuntimeBoundaryError::WrongTransactionId
            | RuntimeBoundaryError::InvalidOfficialTransaction => Self::InvalidObservation,
            RuntimeBoundaryError::WrongCompatibility
            | RuntimeBoundaryError::WrongRole
            | RuntimeBoundaryError::WrongSigner
            | RuntimeBoundaryError::InvalidRuntimeIdentity
            | RuntimeBoundaryError::InvalidNodeEndpoint
            | RuntimeBoundaryError::WrongChannel => Self::Planner,
        }
    }
}

#[derive(Clone)]
struct FoundTransaction {
    facts: ObservedTransactionFacts,
    ordered_account_ids: AccountIds,
}

struct PairScan {
    tip: ChainTip,
    initialization: Option<FoundTransaction>,
    funding: Option<FoundTransaction>,
    fully_covered: bool,
}

struct ClaimScan {
    tip: ChainTip,
    claim: Option<(FoundTransaction, [u8; 32])>,
    fully_covered: bool,
}

/// One exact-v0.2 planner and official-node composition used by the bridge server.
pub struct BridgeRuntime {
    runtime: RuntimeDescriptor,
    planner: Arc<NativeEscrowPlanner>,
    node: Arc<OfficialNodeRpc>,
    finalized_claim_observer: FinalizedWitnessedClaimObserver,
    finalized_funding_observer: FinalizedWitnessedFundingObserver,
}

impl std::fmt::Debug for BridgeRuntime {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("BridgeRuntime")
            .field("runtime", &self.runtime)
            .finish_non_exhaustive()
    }
}

impl BridgeRuntime {
    /// Binds the immutable runtime descriptor to its planner and official node.
    #[must_use]
    pub fn new(
        runtime: RuntimeDescriptor,
        planner: Arc<NativeEscrowPlanner>,
        node: Arc<OfficialNodeRpc>,
        indexer: Arc<dyn FinalizedIndexerApi>,
    ) -> Self {
        let finalized_claim_observer =
            FinalizedWitnessedClaimObserver::new(runtime.clone(), Arc::clone(&indexer));
        let finalized_funding_observer =
            FinalizedWitnessedFundingObserver::new(runtime.clone(), indexer);
        Self {
            runtime,
            planner,
            node,
            finalized_claim_observer,
            finalized_funding_observer,
        }
    }

    /// Returns the immutable descriptor used by every request.
    pub const fn descriptor(&self) -> &RuntimeDescriptor {
        &self.runtime
    }

    /// Proves official sequencer health and the configured channel before bind.
    ///
    /// # Errors
    ///
    /// Fails closed when health or channel identity is unavailable or differs.
    pub async fn verify_health(&self) -> Result<(), BridgeRuntimeError> {
        let health = self.node.check_health().await?;
        if health.channel_id() != self.runtime.channel_id.as_bytes() {
            return Err(BridgeRuntimeError::InvalidObservation);
        }
        Ok(())
    }

    /// Prepares one exact native initialization/funding pair.
    ///
    /// # Errors
    ///
    /// Preserves every exact planner validation and durable-reservation error.
    pub async fn prepare_native_escrow(
        &self,
        request: lez_bridge_protocol::PrepareNativeEscrowRequest,
    ) -> Result<lez_bridge_protocol::PrepareNativeEscrowResult, BridgeRuntimeError> {
        self.planner.prepare(request).await.map_err(Into::into)
    }

    /// Prepares one exact witnessed initialization/funding pair.
    ///
    /// # Errors
    ///
    /// Preserves every exact planner validation and durable-reservation error.
    pub async fn prepare_witnessed_escrow(
        &self,
        request: &lez_bridge_protocol::PrepareWitnessedEscrowRequest,
    ) -> Result<lez_bridge_protocol::PrepareWitnessedEscrowResult, BridgeRuntimeError> {
        self.planner
            .prepare_witnessed_escrow(request)
            .await
            .map_err(Into::into)
    }

    /// Prepares one exact native revealing claim.
    ///
    /// # Errors
    ///
    /// Preserves every exact planner validation and durable-reservation error.
    pub async fn prepare_revealing_claim(
        &self,
        request: &lez_bridge_protocol::PrepareRevealingClaimRequest,
    ) -> Result<lez_bridge_protocol::PrepareRevealingClaimResult, BridgeRuntimeError> {
        self.planner
            .prepare_revealing_claim(request)
            .await
            .map_err(Into::into)
    }

    /// Prepares one exact permissionless native refund without submitting it.
    ///
    /// # Errors
    ///
    /// Preserves every exact planner validation and durable-reservation error.
    pub async fn prepare_native_refund(
        &self,
        request: &lez_bridge_protocol::PrepareNativeRefundRequest,
    ) -> Result<lez_bridge_protocol::PrepareNativeRefundResult, BridgeRuntimeError> {
        self.planner
            .prepare_native_refund(request)
            .await
            .map_err(Into::into)
    }

    /// Reserves one exact unsigned witnessed-claim message.
    ///
    /// # Errors
    ///
    /// Returns the planner's typed validation, nonce, encoding, or durable-state error.
    pub async fn prepare_witnessed_claim(
        &self,
        request: &PrepareWitnessedClaimRequest,
    ) -> Result<PrepareWitnessedClaimResult, BridgeRuntimeError> {
        self.planner
            .prepare_witnessed_claim(request)
            .await
            .map_err(Into::into)
    }

    /// Completes one exact witnessed reservation without submitting it.
    ///
    /// # Errors
    ///
    /// Returns a typed error for transcript drift, invalid signatures, encoding,
    /// or durable-state conflicts.
    pub async fn complete_witnessed_claim(
        &self,
        request: &CompleteWitnessedClaimRequest,
    ) -> Result<CompleteWitnessedClaimResult, BridgeRuntimeError> {
        self.planner
            .complete_witnessed_claim(request)
            .await
            .map_err(Into::into)
    }

    /// Observes one exact witnessed claim in a stable, fully finalized indexer window.
    ///
    /// # Errors
    ///
    /// Fails closed on role/runtime/message/terms drift, incomplete finality,
    /// missing or contradictory indexer facts, duplicate occurrence, or tip movement.
    pub async fn observe_finalized_witnessed_claim(
        &self,
        request: &ObserveFinalizedWitnessedClaimRequest,
    ) -> Result<ObserveFinalizedWitnessedClaimResult, BridgeRuntimeError> {
        self.finalized_claim_observer.observe(request).await
    }

    /// Classifies exact witnessed-claim presence in a stable finalized window.
    ///
    /// # Errors
    ///
    /// Fails closed on role/runtime/message/terms drift, incomplete finality,
    /// missing or contradictory indexer facts, duplicate occurrence, or tip
    /// movement. Only a complete stable scan may return definitive absence.
    pub async fn classify_finalized_witnessed_claim(
        &self,
        request: &ObserveFinalizedWitnessedClaimRequest,
    ) -> Result<ClassifyFinalizedWitnessedClaimResult, BridgeRuntimeError> {
        self.finalized_claim_observer.classify(request).await
    }

    /// Observes one witnessed funding effect in a stable, fully finalized indexer window.
    ///
    /// # Errors
    ///
    /// Fails closed on role/runtime/terms drift, incomplete finality, missing or
    /// contradictory indexer facts, duplicate occurrence, invalid historical funded
    /// state, or tip movement.
    pub async fn observe_finalized_witnessed_funding(
        &self,
        request: &ObserveFinalizedWitnessedFundingRequest,
    ) -> Result<ObserveFinalizedWitnessedFundingResult, BridgeRuntimeError> {
        self.finalized_funding_observer.observe(request).await
    }

    /// Submits only exact bytes owned by this actor's active durable planner.
    ///
    /// An exact canonical lookup is performed before submission. A found byte-
    /// identical transaction returns `already_known`; a miss is not called
    /// rejection and is followed by one official submission attempt.
    ///
    /// # Errors
    ///
    /// Rejects unowned bytes, malformed official transactions, node failure,
    /// or an unknown one-attempt submission outcome.
    pub async fn submit_transaction(
        &self,
        request: &SubmitTransactionRequest,
    ) -> Result<SubmitTransactionResult, BridgeRuntimeError> {
        self.planner
            .validate_owned_submission(&request.transaction)
            .await?;
        if self
            .node
            .prepared_transaction_is_included(&request.transaction)
            .await?
        {
            return Ok(SubmitTransactionResult::new(
                request.context.clone(),
                request.transaction.transaction_id,
                SubmissionOutcome::AlreadyKnown,
            ));
        }
        self.node
            .submit_prepared_transaction(&request.transaction)
            .await
            .map_err(|_| BridgeRuntimeError::UnknownSubmissionOutcome)?;
        Ok(SubmitTransactionResult::new(
            request.context.clone(),
            request.transaction.transaction_id,
            SubmissionOutcome::Accepted,
        ))
    }

    /// Observes an exact owned pair or discovers a counterparty pair by terms.
    ///
    /// # Errors
    ///
    /// Fails closed on role/runtime drift, moving tips, unavailable node facts,
    /// invalid canonical facts, or ambiguous terms discovery.
    #[allow(
        clippy::too_many_lines,
        reason = "the complete pair and snapshot are validated under one auditable tip bracket"
    )]
    pub async fn observe_escrow(
        &self,
        request: &ObserveEscrowRequest,
    ) -> Result<ObserveEscrowResult, BridgeRuntimeError> {
        let signer = self.runtime.signer_account_id;
        let (window, expected, exact) = match request.target {
            EscrowObservationTarget::Exact {
                initialization_transaction_id,
                funding_transaction_id,
            } => {
                if request.terms.depositor() != self.runtime.sidecar_role
                    || request.terms.depositor_account_id() != signer
                {
                    return Err(BridgeRuntimeError::Planner);
                }
                let expected = self
                    .planner
                    .owned_native_pair(
                        request,
                        initialization_transaction_id,
                        funding_transaction_id,
                    )
                    .await?;
                let tip = self.read_tip().await?;
                let start = tip
                    .height
                    .saturating_sub(u64::from(MAX_DISCOVERY_BLOCKS - 1))
                    .max(nssa::GENESIS_BLOCK_ID);
                (
                    DiscoveryWindow::new(start, MAX_DISCOVERY_BLOCKS)
                        .map_err(|_| BridgeRuntimeError::InvalidObservation)?,
                    Some(expected),
                    true,
                )
            }
            EscrowObservationTarget::DiscoverByTerms { window } => {
                if request.terms.claimant() != self.runtime.sidecar_role
                    || request.terms.claimant_account_id() != signer
                    || request.terms.depositor() == self.runtime.sidecar_role
                {
                    return Err(BridgeRuntimeError::Planner);
                }
                (window, None, false)
            }
        };
        let scan = self.scan_pair(request, window, expected.as_ref()).await?;
        if let (Some(initialization), Some(funding)) =
            (scan.initialization.as_ref(), scan.funding.as_ref())
            && position_key(&initialization.facts.position) >= position_key(&funding.facts.position)
        {
            return Err(BridgeRuntimeError::InvalidObservation);
        }
        let snapshot = if scan.initialization.is_some() || scan.funding.is_some() {
            Some(self.read_snapshot(&request.terms).await?)
        } else {
            None
        };
        let tip_after = self.read_tip().await?;
        if tip_after != scan.tip
            || snapshot
                .as_ref()
                .is_some_and(|(_, _, tip)| *tip != scan.tip)
        {
            return Err(BridgeRuntimeError::MovingTip);
        }
        let missing_is_absent = !exact && scan.fully_covered;
        let initialization = match scan.initialization {
            Some(found) => {
                let (metadata, _, _) = snapshot
                    .as_ref()
                    .ok_or(BridgeRuntimeError::InvalidObservation)?;
                InitializationObservation::found(InitializationFoundFacts::new(
                    found.facts,
                    NativeInitializeInstructionFacts::new(
                        self.runtime.escrow_program_id,
                        found.ordered_account_ids,
                        request.terms.clone(),
                    ),
                    metadata.clone(),
                ))
            }
            None if missing_is_absent => InitializationObservation::Absent,
            None => InitializationObservation::UnknownOrPending,
        };
        let funding = match scan.funding {
            Some(found) => {
                let (metadata, custody, _) = snapshot
                    .as_ref()
                    .ok_or(BridgeRuntimeError::InvalidObservation)?;
                FundingObservation::found(FundingFoundFacts::new(
                    found.facts,
                    NativeFundInstructionFacts::new(
                        self.runtime.escrow_program_id,
                        found.ordered_account_ids,
                        request.terms.swap_id(),
                    ),
                    metadata.clone(),
                    custody.clone(),
                ))
            }
            None if missing_is_absent => FundingObservation::Absent,
            None => FundingObservation::UnknownOrPending,
        };
        Ok(ObserveEscrowResult::new(
            request.context.clone(),
            scan.tip,
            initialization,
            funding,
            tip_after,
        ))
    }

    /// Observes an exact owned witnessed pair or discovers a counterparty pair by terms.
    ///
    /// Canonical transaction bytes, generated instruction accounts, aggregate
    /// authority metadata, custody effects, and all account reads are bound to
    /// one unchanged upstream tip. Inclusion does not overstate Bedrock finality.
    ///
    /// # Errors
    ///
    /// Fails closed on role/runtime/authority drift, moving tips, unavailable
    /// node facts, invalid canonical facts, or ambiguous terms discovery.
    #[allow(
        clippy::too_many_lines,
        reason = "the witnessed pair and same-tip account effects remain auditable together"
    )]
    pub async fn observe_witnessed_escrow(
        &self,
        request: &ObserveWitnessedEscrowRequest,
    ) -> Result<ObserveWitnessedEscrowResult, BridgeRuntimeError> {
        validate_witnessed_authority(&request.terms)?;
        let signer = self.runtime.signer_account_id;
        let (window, expected, exact) = match request.target {
            EscrowObservationTarget::Exact {
                initialization_transaction_id,
                funding_transaction_id,
            } => {
                if request.terms.depositor() != self.runtime.sidecar_role
                    || request.terms.depositor_account_id() != signer
                {
                    return Err(BridgeRuntimeError::Planner);
                }
                let expected = self
                    .planner
                    .owned_witnessed_pair(
                        request,
                        initialization_transaction_id,
                        funding_transaction_id,
                    )
                    .await?;
                let tip = self.read_tip().await?;
                let start = tip
                    .height
                    .saturating_sub(u64::from(MAX_DISCOVERY_BLOCKS - 1))
                    .max(nssa::GENESIS_BLOCK_ID);
                (
                    DiscoveryWindow::new(start, MAX_DISCOVERY_BLOCKS)
                        .map_err(|_| BridgeRuntimeError::InvalidObservation)?,
                    Some(expected),
                    true,
                )
            }
            EscrowObservationTarget::DiscoverByTerms { window } => {
                if request.terms.claimant() != self.runtime.sidecar_role
                    || request.terms.claimant_account_id() != signer
                    || request.terms.depositor() == self.runtime.sidecar_role
                {
                    return Err(BridgeRuntimeError::Planner);
                }
                (window, None, false)
            }
        };
        let scan = self
            .scan_witnessed_pair(request, window, expected.as_ref())
            .await?;
        if let (Some(initialization), Some(funding)) =
            (scan.initialization.as_ref(), scan.funding.as_ref())
            && position_key(&initialization.facts.position) >= position_key(&funding.facts.position)
        {
            return Err(BridgeRuntimeError::InvalidObservation);
        }
        let snapshot = if scan.initialization.is_some() || scan.funding.is_some() {
            Some(self.read_witnessed_snapshot(&request.terms).await?)
        } else {
            None
        };
        let tip_after = self.read_tip().await?;
        if tip_after != scan.tip
            || snapshot
                .as_ref()
                .is_some_and(|(_, _, tip)| *tip != scan.tip)
        {
            return Err(BridgeRuntimeError::MovingTip);
        }
        let missing_is_absent = !exact && scan.fully_covered;
        let initialization = match scan.initialization {
            Some(found) => {
                let (metadata, _, _) = snapshot
                    .as_ref()
                    .ok_or(BridgeRuntimeError::InvalidObservation)?;
                WitnessedInitializationObservation::found(WitnessedInitializationFoundFacts::new(
                    found.facts,
                    WitnessedNativeInitializeInstructionFacts::new(
                        self.runtime.escrow_program_id,
                        found.ordered_account_ids,
                        request.terms.clone(),
                    ),
                    metadata.clone(),
                ))
            }
            None if missing_is_absent => WitnessedInitializationObservation::Absent,
            None => WitnessedInitializationObservation::UnknownOrPending,
        };
        let funding = match scan.funding {
            Some(found) => {
                let (metadata, custody, _) = snapshot
                    .as_ref()
                    .ok_or(BridgeRuntimeError::InvalidObservation)?;
                WitnessedFundingObservation::found(WitnessedFundingFoundFacts::new(
                    found.facts,
                    NativeFundInstructionFacts::new(
                        self.runtime.escrow_program_id,
                        found.ordered_account_ids,
                        request.terms.swap_id(),
                    ),
                    metadata.clone(),
                    custody.clone(),
                ))
            }
            None if missing_is_absent => WitnessedFundingObservation::Absent,
            None => WitnessedFundingObservation::UnknownOrPending,
        };
        Ok(ObserveWitnessedEscrowResult::new(
            request.context.clone(),
            scan.tip,
            initialization,
            funding,
            tip_after,
        ))
    }

    /// Observes an exact owned revealing claim or discovers one by terms.
    ///
    /// # Errors
    ///
    /// Fails closed on role/runtime drift, moving tips, unavailable node facts,
    /// invalid canonical facts, or ambiguous terms discovery.
    pub async fn observe_revealing_claim(
        &self,
        request: &ObserveRevealingClaimRequest,
    ) -> Result<ObserveRevealingClaimResult, BridgeRuntimeError> {
        let signer = self.runtime.signer_account_id;
        let (window, expected) = match request.target {
            RevealingClaimObservationTarget::Exact {
                claim_transaction_id,
            } => {
                if request.terms.claimant() != self.runtime.sidecar_role
                    || request.terms.claimant_account_id() != signer
                {
                    return Err(BridgeRuntimeError::Planner);
                }
                let (prepared, preimage) = self
                    .planner
                    .owned_revealing_claim(request, claim_transaction_id)
                    .await?;
                let tip = self.read_tip().await?;
                let start = tip
                    .height
                    .saturating_sub(u64::from(MAX_DISCOVERY_BLOCKS - 1))
                    .max(nssa::GENESIS_BLOCK_ID);
                (
                    DiscoveryWindow::new(start, MAX_DISCOVERY_BLOCKS)
                        .map_err(|_| BridgeRuntimeError::InvalidObservation)?,
                    Some((prepared, preimage)),
                )
            }
            RevealingClaimObservationTarget::DiscoverByTerms { window } => {
                if request.terms.depositor() != self.runtime.sidecar_role
                    || request.terms.depositor_account_id() != signer
                    || request.terms.claimant() == self.runtime.sidecar_role
                {
                    return Err(BridgeRuntimeError::Planner);
                }
                (window, None)
            }
        };
        let exact = expected.is_some();
        let scan = self.scan_claim(request, window, expected.as_ref()).await?;
        let claim = match scan.claim {
            Some((found, preimage)) => {
                let (metadata, custody, snapshot_tip) = self.read_snapshot(&request.terms).await?;
                if snapshot_tip != scan.tip || metadata.status != EscrowState::Claimed {
                    return Err(BridgeRuntimeError::MovingTip);
                }
                RevealingClaimObservation::found(RevealingClaimFoundFacts::new(
                    found.facts,
                    NativeClaimInstructionFacts::new(
                        self.runtime.escrow_program_id,
                        found.ordered_account_ids,
                        request.terms.swap_id(),
                        RevealingPreimage::new(preimage),
                    ),
                    metadata,
                    custody,
                ))
            }
            None if exact || scan.fully_covered => RevealingClaimObservation::Absent,
            None => RevealingClaimObservation::UnknownOrPending,
        };
        let tip_after = self.read_tip().await?;
        if tip_after != scan.tip {
            return Err(BridgeRuntimeError::MovingTip);
        }
        Ok(ObserveRevealingClaimResult::new(
            request.context.clone(),
            scan.tip,
            claim,
            tip_after,
        ))
    }

    #[allow(
        clippy::too_many_lines,
        reason = "both ordered generated instructions are decoded in one bounded canonical scan"
    )]
    async fn scan_pair(
        &self,
        request: &ObserveEscrowRequest,
        window: DiscoveryWindow,
        expected: Option<&lez_bridge_protocol::PrepareNativeEscrowResult>,
    ) -> Result<PairScan, BridgeRuntimeError> {
        let (tip, blocks, fully_covered) = self.blocks_in_window(window).await?;
        let mut initialization = None;
        let mut funding = None;
        for block in &blocks {
            for (index, transaction) in block.body.transactions.iter().enumerate() {
                let LeeTransaction::Public(public) = transaction else {
                    continue;
                };
                if public.message().program_id
                    != program_id_from_hex(self.runtime.escrow_program_id)
                {
                    continue;
                }
                let transaction_id = TransactionId::from_bytes(public.hash());
                let exact_initialization = expected
                    .filter(|pair| pair.initialization.transaction_id == transaction_id)
                    .map(|pair| &pair.initialization);
                let exact_funding = expected
                    .filter(|pair| pair.funding.transaction_id == transaction_id)
                    .map(|pair| &pair.funding);
                if expected.is_some() && exact_initialization.is_none() && exact_funding.is_none() {
                    continue;
                }
                let instruction = decode_instruction(public).map_err(|_| {
                    if expected.is_some() {
                        BridgeRuntimeError::InvalidObservation
                    } else {
                        BridgeRuntimeError::Unavailable
                    }
                })?;
                match instruction {
                    ZecEscrowInstruction::InitializeNative {
                        swap_id,
                        terms_hash,
                        secret_digest,
                        amount,
                        refund_at,
                        authenticated_transfer_program,
                    } if swap_id == *request.terms.swap_id().as_bytes()
                        && terms_hash == *request.terms.terms_hash().as_bytes()
                        && secret_digest == *request.terms.secret_digest().as_bytes()
                        && amount == request.terms.amount().as_u128()
                        && refund_at == request.terms.refund_at_ms()
                        && authenticated_transfer_program
                            == program_id_from_hex(
                                request.terms.authenticated_transfer_program_id(),
                            ) =>
                    {
                        let found = Self::found_transaction(
                            public,
                            block,
                            index,
                            request.terms.depositor_account_id(),
                            expected.map(|pair| &pair.initialization),
                            expected.is_some(),
                        )?;
                        let expected_accounts = native_account_ids(
                            &request.terms,
                            self.runtime.escrow_program_id,
                            true,
                        );
                        if found.ordered_account_ids != expected_accounts {
                            return Err(BridgeRuntimeError::InvalidObservation);
                        }
                        replace_unique(&mut initialization, found, expected.is_some())?;
                    }
                    ZecEscrowInstruction::FundNative { swap_id }
                        if swap_id == *request.terms.swap_id().as_bytes() =>
                    {
                        let found = Self::found_transaction(
                            public,
                            block,
                            index,
                            request.terms.depositor_account_id(),
                            expected.map(|pair| &pair.funding),
                            expected.is_some(),
                        )?;
                        let expected_accounts = native_account_ids(
                            &request.terms,
                            self.runtime.escrow_program_id,
                            false,
                        );
                        if found.ordered_account_ids != expected_accounts {
                            return Err(BridgeRuntimeError::InvalidObservation);
                        }
                        replace_unique(&mut funding, found, expected.is_some())?;
                    }
                    _ if expected.is_some() => {
                        return Err(BridgeRuntimeError::InvalidObservation);
                    }
                    _ => {}
                }
            }
        }
        Ok(PairScan {
            tip,
            initialization,
            funding,
            fully_covered,
        })
    }

    #[allow(
        clippy::too_many_lines,
        reason = "both exact witnessed instructions are decoded in one bounded canonical scan"
    )]
    async fn scan_witnessed_pair(
        &self,
        request: &ObserveWitnessedEscrowRequest,
        window: DiscoveryWindow,
        expected: Option<&lez_bridge_protocol::PrepareWitnessedEscrowResult>,
    ) -> Result<PairScan, BridgeRuntimeError> {
        let (tip, blocks, fully_covered) = self.blocks_in_window(window).await?;
        let mut initialization = None;
        let mut funding = None;
        for block in &blocks {
            for (index, transaction) in block.body.transactions.iter().enumerate() {
                let LeeTransaction::Public(public) = transaction else {
                    continue;
                };
                if public.message().program_id
                    != program_id_from_hex(self.runtime.escrow_program_id)
                {
                    continue;
                }
                let transaction_id = TransactionId::from_bytes(public.hash());
                let exact_initialization = expected
                    .filter(|pair| pair.initialization.transaction_id == transaction_id)
                    .map(|pair| &pair.initialization);
                let exact_funding = expected
                    .filter(|pair| pair.funding.transaction_id == transaction_id)
                    .map(|pair| &pair.funding);
                if expected.is_some() && exact_initialization.is_none() && exact_funding.is_none() {
                    continue;
                }
                let instruction = decode_instruction(public).map_err(|_| {
                    if expected.is_some() {
                        BridgeRuntimeError::InvalidObservation
                    } else {
                        BridgeRuntimeError::Unavailable
                    }
                })?;
                match instruction {
                    ZecEscrowInstruction::InitializeNativeWitnessed {
                        swap_id,
                        terms_hash,
                        aggregate_x_only_public_key,
                        amount,
                        refund_at,
                        authenticated_transfer_program,
                    } if swap_id == *request.terms.swap_id().as_bytes()
                        && terms_hash == *request.terms.terms_hash().as_bytes()
                        && aggregate_x_only_public_key
                            == *request.terms.aggregate_x_only_public_key().as_bytes()
                        && amount == request.terms.amount().as_u128()
                        && refund_at == request.terms.refund_at_ms()
                        && authenticated_transfer_program
                            == program_id_from_hex(
                                request.terms.authenticated_transfer_program_id(),
                            ) =>
                    {
                        let found = Self::found_transaction(
                            public,
                            block,
                            index,
                            request.terms.depositor_account_id(),
                            expected.map(|pair| &pair.initialization),
                            expected.is_some(),
                        )?;
                        let expected_accounts = witnessed_account_ids(
                            &request.terms,
                            self.runtime.escrow_program_id,
                            true,
                        );
                        if found.ordered_account_ids != expected_accounts {
                            return Err(BridgeRuntimeError::InvalidObservation);
                        }
                        replace_unique(&mut initialization, found, expected.is_some())?;
                    }
                    ZecEscrowInstruction::FundNative { swap_id }
                        if swap_id == *request.terms.swap_id().as_bytes() =>
                    {
                        let found = Self::found_transaction(
                            public,
                            block,
                            index,
                            request.terms.depositor_account_id(),
                            expected.map(|pair| &pair.funding),
                            expected.is_some(),
                        )?;
                        let expected_accounts = witnessed_account_ids(
                            &request.terms,
                            self.runtime.escrow_program_id,
                            false,
                        );
                        if found.ordered_account_ids != expected_accounts {
                            return Err(BridgeRuntimeError::InvalidObservation);
                        }
                        replace_unique(&mut funding, found, expected.is_some())?;
                    }
                    _ if expected.is_some() => {
                        return Err(BridgeRuntimeError::InvalidObservation);
                    }
                    _ => {}
                }
            }
        }
        Ok(PairScan {
            tip,
            initialization,
            funding,
            fully_covered,
        })
    }

    async fn scan_claim(
        &self,
        request: &ObserveRevealingClaimRequest,
        window: DiscoveryWindow,
        expected: Option<&(PreparedTransaction, [u8; 32])>,
    ) -> Result<ClaimScan, BridgeRuntimeError> {
        let (tip, blocks, fully_covered) = self.blocks_in_window(window).await?;
        let mut claim = None;
        for block in &blocks {
            for (index, transaction) in block.body.transactions.iter().enumerate() {
                let LeeTransaction::Public(public) = transaction else {
                    continue;
                };
                if public.message().program_id
                    != program_id_from_hex(self.runtime.escrow_program_id)
                {
                    continue;
                }
                let transaction_id = TransactionId::from_bytes(public.hash());
                if expected.is_some_and(|(prepared, _)| prepared.transaction_id != transaction_id) {
                    continue;
                }
                let instruction = decode_instruction(public).map_err(|_| {
                    if expected.is_some() {
                        BridgeRuntimeError::InvalidObservation
                    } else {
                        BridgeRuntimeError::Unavailable
                    }
                })?;
                let ZecEscrowInstruction::ClaimNative { swap_id, preimage } = instruction else {
                    if expected.is_some() {
                        return Err(BridgeRuntimeError::InvalidObservation);
                    }
                    continue;
                };
                if swap_id != *request.terms.swap_id().as_bytes()
                    || <[u8; 32]>::from(Sha256::digest(preimage))
                        != *request.terms.secret_digest().as_bytes()
                {
                    if expected.is_some() {
                        return Err(BridgeRuntimeError::InvalidObservation);
                    }
                    continue;
                }
                if expected.is_some_and(|(_, exact_preimage)| *exact_preimage != preimage) {
                    return Err(BridgeRuntimeError::InvalidObservation);
                }
                let found = Self::found_transaction(
                    public,
                    block,
                    index,
                    request.terms.claimant_account_id(),
                    expected.map(|(prepared, _)| prepared),
                    expected.is_some(),
                )?;
                let expected_accounts =
                    claim_account_ids(&request.terms, self.runtime.escrow_program_id);
                if found.ordered_account_ids != expected_accounts {
                    return Err(BridgeRuntimeError::InvalidObservation);
                }
                if claim.replace((found, preimage)).is_some() {
                    return Err(if expected.is_some() {
                        BridgeRuntimeError::InvalidObservation
                    } else {
                        BridgeRuntimeError::AmbiguousDiscovery
                    });
                }
            }
        }
        Ok(ClaimScan {
            tip,
            claim,
            fully_covered,
        })
    }

    fn found_transaction(
        public: &PublicTransaction,
        block: &Block,
        index: usize,
        expected_signer: Hex32,
        expected: Option<&PreparedTransaction>,
        exact: bool,
    ) -> Result<FoundTransaction, BridgeRuntimeError> {
        let expected_signer = AccountId::new(*expected_signer.as_bytes());
        let prepared = prepared_from_transaction(public)?;
        decode_prepared_for_signer(&prepared, expected_signer)?;
        if expected.is_some_and(|expected| expected != &prepared) {
            return Err(BridgeRuntimeError::InvalidObservation);
        }
        if exact && expected.is_none() {
            return Err(BridgeRuntimeError::InvalidObservation);
        }
        let signer_ids = public
            .witness_set()
            .signatures_and_public_keys()
            .iter()
            .map(|(_, key)| Hex32::from_bytes(AccountId::from(key).into_value()))
            .collect::<Vec<_>>();
        let ordered_account_ids = AccountIds::new(
            public
                .message()
                .account_ids
                .iter()
                .map(|account_id| Hex32::from_bytes(account_id.into_value()))
                .collect(),
        )
        .map_err(|_| BridgeRuntimeError::InvalidObservation)?;
        Ok(FoundTransaction {
            facts: ObservedTransactionFacts::new(
                prepared.transaction_id,
                prepared.exact_bytes,
                ChainPosition::new(
                    Hex32::from_bytes(block.header.hash.0),
                    block.header.block_id,
                    u32::try_from(index).map_err(|_| BridgeRuntimeError::InvalidObservation)?,
                ),
                AccountIds::new(signer_ids).map_err(|_| BridgeRuntimeError::InvalidObservation)?,
                true,
            ),
            ordered_account_ids,
        })
    }

    async fn blocks_in_window(
        &self,
        window: DiscoveryWindow,
    ) -> Result<(ChainTip, Vec<Block>, bool), BridgeRuntimeError> {
        let tip = self.read_tip().await?;
        let declared_end = window
            .start_height()
            .checked_add(u64::from(window.max_blocks() - 1))
            .ok_or(BridgeRuntimeError::InvalidObservation)?;
        if window.start_height() > tip.height || declared_end < nssa::GENESIS_BLOCK_ID {
            return Ok((tip, Vec::new(), tip.height >= declared_end));
        }
        let start = window.start_height().max(nssa::GENESIS_BLOCK_ID);
        let end = declared_end.min(tip.height);
        let anchor = start.saturating_sub(1).max(nssa::GENESIS_BLOCK_ID);
        let blocks = self.node.block_range(anchor, end).await?;
        validate_block_range(&blocks, anchor, end, self.runtime.genesis_block_hash)?;
        let blocks = blocks
            .into_iter()
            .skip(usize::from(anchor < start))
            .collect();
        let tip_after = self.read_tip().await?;
        if tip_after != tip {
            return Err(BridgeRuntimeError::MovingTip);
        }
        Ok((tip, blocks, tip.height >= declared_end))
    }

    async fn read_tip(&self) -> Result<ChainTip, BridgeRuntimeError> {
        let block = self.node.tip_block().await?;
        let height = block.header.block_id;
        if block.header.block_id != height
            || (height == nssa::GENESIS_BLOCK_ID
                && block.header.hash.0 != *self.runtime.genesis_block_hash.as_bytes())
        {
            return Err(BridgeRuntimeError::InvalidObservation);
        }
        Ok(ChainTip::new(
            Hex32::from_bytes(block.header.hash.0),
            height,
        ))
    }

    async fn read_snapshot(
        &self,
        terms: &lez_bridge_protocol::NativeEscrowTerms,
    ) -> Result<(EscrowMetadataFacts, NativeCustodyFacts, ChainTip), BridgeRuntimeError> {
        let escrow_program = program_id_from_hex(self.runtime.escrow_program_id);
        let swap_id = *terms.swap_id().as_bytes();
        let metadata_id = compute_metadata_pda(&escrow_program, &swap_id);
        let custody_id = compute_custody_pda(&escrow_program, &swap_id);
        let depositor = AccountId::new(*terms.depositor_account_id().as_bytes());
        let claimant = AccountId::new(*terms.claimant_account_id().as_bytes());
        let facts = self
            .node
            .native_escrow_facts(metadata_id, custody_id, depositor, claimant)
            .await?;
        self.snapshot_facts(terms, metadata_id, custody_id, &facts)
    }

    async fn read_witnessed_snapshot(
        &self,
        terms: &lez_bridge_protocol::WitnessedNativeEscrowTerms,
    ) -> Result<(WitnessedEscrowMetadataFacts, NativeCustodyFacts, ChainTip), BridgeRuntimeError>
    {
        let escrow_program = program_id_from_hex(self.runtime.escrow_program_id);
        let swap_id = *terms.swap_id().as_bytes();
        let metadata_id = compute_metadata_pda(&escrow_program, &swap_id);
        let custody_id = compute_custody_pda(&escrow_program, &swap_id);
        let depositor = AccountId::new(*terms.depositor_account_id().as_bytes());
        let claimant = AccountId::new(*terms.claimant_account_id().as_bytes());
        let facts = self
            .node
            .native_escrow_facts(metadata_id, custody_id, depositor, claimant)
            .await?;
        self.witnessed_snapshot_facts(terms, metadata_id, custody_id, &facts)
    }

    fn witnessed_snapshot_facts(
        &self,
        terms: &lez_bridge_protocol::WitnessedNativeEscrowTerms,
        metadata_id: AccountId,
        custody_id: AccountId,
        facts: &OfficialNativeEscrowFacts,
    ) -> Result<(WitnessedEscrowMetadataFacts, NativeCustodyFacts, ChainTip), BridgeRuntimeError>
    {
        if facts.channel_id() != *self.runtime.channel_id.as_bytes()
            || facts.genesis_block_hash() != *self.runtime.genesis_block_hash.as_bytes()
        {
            return Err(BridgeRuntimeError::InvalidObservation);
        }
        let metadata = EscrowMetadata::try_from_slice(facts.metadata_account().data.as_ref())
            .map_err(|_| BridgeRuntimeError::InvalidObservation)?;
        let state = match metadata.status {
            EscrowStatus::Empty => EscrowState::Empty,
            EscrowStatus::Funded => EscrowState::Funded,
            EscrowStatus::Claimed => EscrowState::Claimed,
            EscrowStatus::Refunded => EscrowState::Refunded,
        };
        let expected_transfer = program_id_from_hex(terms.authenticated_transfer_program_id());
        let ClaimAuthority::AggregateWitness {
            x_only_public_key,
            account_id,
        } = metadata.claim_authority
        else {
            return Err(BridgeRuntimeError::InvalidObservation);
        };
        if facts.metadata_account().program_owner
            != program_id_from_hex(self.runtime.escrow_program_id)
            || metadata.version != 2
            || metadata.swap_id != *terms.swap_id().as_bytes()
            || metadata.terms_hash != *terms.terms_hash().as_bytes()
            || x_only_public_key != *terms.aggregate_x_only_public_key().as_bytes()
            || account_id.into_value() != *terms.aggregate_authority_account_id().as_bytes()
            || metadata.depositor.into_value() != *terms.depositor_account_id().as_bytes()
            || metadata.depositor_asset != metadata.depositor
            || metadata.claimant.into_value() != *terms.claimant_account_id().as_bytes()
            || metadata.claimant_asset != metadata.claimant
            || metadata.custody != custody_id
            || metadata.asset_program != expected_transfer
            || metadata.custody_program != expected_transfer
            || metadata.asset_definition != [0; 32]
            || metadata.amount != terms.amount().as_u128()
            || metadata.refund_at != terms.refund_at_ms()
            || facts.custody_account().program_owner != expected_transfer
            || facts.depositor_account().program_owner != expected_transfer
            || facts.claimant_account().program_owner != expected_transfer
        {
            return Err(BridgeRuntimeError::InvalidObservation);
        }
        let expected_balance = if state == EscrowState::Funded {
            terms.amount().as_u128()
        } else {
            0
        };
        if facts.custody_account().balance != expected_balance {
            return Err(BridgeRuntimeError::InvalidObservation);
        }
        let metadata_facts = WitnessedEscrowMetadataFacts::from_witnessed_native_terms(
            Hex32::from_bytes(metadata_id.into_value()),
            self.runtime.escrow_program_id,
            Hex32::from_bytes(custody_id.into_value()),
            terms,
            state,
        );
        let custody_facts = NativeCustodyFacts::new(
            Hex32::from_bytes(custody_id.into_value()),
            program_id_to_hex(facts.custody_account().program_owner),
            facts.custody_account().balance,
        );
        Ok((
            metadata_facts,
            custody_facts,
            ChainTip::new(
                Hex32::from_bytes(facts.tip_block_hash()),
                facts.sequencer_tip(),
            ),
        ))
    }

    fn snapshot_facts(
        &self,
        terms: &lez_bridge_protocol::NativeEscrowTerms,
        metadata_id: AccountId,
        custody_id: AccountId,
        facts: &OfficialNativeEscrowFacts,
    ) -> Result<(EscrowMetadataFacts, NativeCustodyFacts, ChainTip), BridgeRuntimeError> {
        if facts.channel_id() != *self.runtime.channel_id.as_bytes()
            || facts.genesis_block_hash() != *self.runtime.genesis_block_hash.as_bytes()
        {
            return Err(BridgeRuntimeError::InvalidObservation);
        }
        let metadata = EscrowMetadata::try_from_slice(facts.metadata_account().data.as_ref())
            .map_err(|_| BridgeRuntimeError::InvalidObservation)?;
        let state = match metadata.status {
            EscrowStatus::Empty => EscrowState::Empty,
            EscrowStatus::Funded => EscrowState::Funded,
            EscrowStatus::Claimed => EscrowState::Claimed,
            EscrowStatus::Refunded => EscrowState::Refunded,
        };
        let expected_transfer = program_id_from_hex(terms.authenticated_transfer_program_id());
        let ClaimAuthority::Sha256Preimage { secret_digest } = metadata.claim_authority else {
            return Err(BridgeRuntimeError::InvalidObservation);
        };
        if facts.metadata_account().program_owner
            != program_id_from_hex(self.runtime.escrow_program_id)
            || metadata.version != 2
            || metadata.swap_id != *terms.swap_id().as_bytes()
            || metadata.terms_hash != *terms.terms_hash().as_bytes()
            || secret_digest != *terms.secret_digest().as_bytes()
            || metadata.depositor.into_value() != *terms.depositor_account_id().as_bytes()
            || metadata.depositor_asset != metadata.depositor
            || metadata.claimant.into_value() != *terms.claimant_account_id().as_bytes()
            || metadata.claimant_asset != metadata.claimant
            || metadata.custody != custody_id
            || metadata.asset_program != expected_transfer
            || metadata.custody_program != expected_transfer
            || metadata.asset_definition != [0; 32]
            || metadata.amount != terms.amount().as_u128()
            || metadata.refund_at != terms.refund_at_ms()
            || facts.custody_account().program_owner != expected_transfer
        {
            return Err(BridgeRuntimeError::InvalidObservation);
        }
        let expected_balance = if state == EscrowState::Funded {
            terms.amount().as_u128()
        } else {
            0
        };
        if facts.custody_account().balance != expected_balance {
            return Err(BridgeRuntimeError::InvalidObservation);
        }
        let metadata_facts = EscrowMetadataFacts::from_lee_v0_2_native_terms(
            Hex32::from_bytes(metadata_id.into_value()),
            self.runtime.escrow_program_id,
            Hex32::from_bytes(custody_id.into_value()),
            terms,
            state,
        );
        let custody_facts = NativeCustodyFacts::new(
            Hex32::from_bytes(custody_id.into_value()),
            program_id_to_hex(facts.custody_account().program_owner),
            facts.custody_account().balance,
        );
        Ok((
            metadata_facts,
            custody_facts,
            ChainTip::new(
                Hex32::from_bytes(facts.tip_block_hash()),
                facts.sequencer_tip(),
            ),
        ))
    }
}

fn decode_instruction(
    transaction: &PublicTransaction,
) -> Result<ZecEscrowInstruction, BridgeRuntimeError> {
    risc0_zkvm::serde::from_slice::<ZecEscrowInstruction, u32>(
        &transaction.message().instruction_data,
    )
    .map_err(|_| BridgeRuntimeError::InvalidObservation)
}

fn native_account_ids(
    terms: &lez_bridge_protocol::NativeEscrowTerms,
    escrow_program_id: Hex32,
    initialization: bool,
) -> AccountIds {
    let escrow_program = program_id_from_hex(escrow_program_id);
    let swap_id = terms.swap_id();
    let swap_id = swap_id.as_bytes();
    let metadata = Hex32::from_bytes(compute_metadata_pda(&escrow_program, swap_id).into_value());
    let custody = Hex32::from_bytes(compute_custody_pda(&escrow_program, swap_id).into_value());
    AccountIds::new(if initialization {
        vec![
            metadata,
            custody,
            terms.depositor_account_id(),
            terms.claimant_account_id(),
        ]
    } else {
        vec![metadata, custody, terms.depositor_account_id()]
    })
    .expect("native account count is protocol bounded")
}

fn validate_witnessed_authority(
    terms: &lez_bridge_protocol::WitnessedNativeEscrowTerms,
) -> Result<(), BridgeRuntimeError> {
    let key = PublicKey::try_new(*terms.aggregate_x_only_public_key().as_bytes())
        .map_err(|_| BridgeRuntimeError::InvalidObservation)?;
    let authority = AccountId::from(&key);
    if authority.into_value() != *terms.aggregate_authority_account_id().as_bytes()
        || terms.aggregate_authority_account_id() == terms.claimant_account_id()
        || terms.aggregate_authority_account_id() == terms.depositor_account_id()
    {
        return Err(BridgeRuntimeError::InvalidObservation);
    }
    Ok(())
}

fn witnessed_account_ids(
    terms: &lez_bridge_protocol::WitnessedNativeEscrowTerms,
    escrow_program_id: Hex32,
    initialization: bool,
) -> AccountIds {
    let escrow_program = program_id_from_hex(escrow_program_id);
    let swap_id = terms.swap_id();
    let swap_id = swap_id.as_bytes();
    let metadata = Hex32::from_bytes(compute_metadata_pda(&escrow_program, swap_id).into_value());
    let custody = Hex32::from_bytes(compute_custody_pda(&escrow_program, swap_id).into_value());
    AccountIds::new(if initialization {
        vec![
            metadata,
            custody,
            terms.depositor_account_id(),
            terms.claimant_account_id(),
            terms.aggregate_authority_account_id(),
        ]
    } else {
        vec![metadata, custody, terms.depositor_account_id()]
    })
    .expect("witnessed native account count is protocol bounded")
}

fn claim_account_ids(
    terms: &lez_bridge_protocol::NativeEscrowTerms,
    escrow_program_id: Hex32,
) -> AccountIds {
    let escrow_program = program_id_from_hex(escrow_program_id);
    let swap_id = terms.swap_id();
    let swap_id = swap_id.as_bytes();
    AccountIds::new(vec![
        Hex32::from_bytes(compute_metadata_pda(&escrow_program, swap_id).into_value()),
        Hex32::from_bytes(compute_custody_pda(&escrow_program, swap_id).into_value()),
        terms.claimant_account_id(),
    ])
    .expect("claim account count is protocol bounded")
}

fn replace_unique(
    slot: &mut Option<FoundTransaction>,
    value: FoundTransaction,
    exact: bool,
) -> Result<(), BridgeRuntimeError> {
    if slot.replace(value).is_some() {
        return Err(if exact {
            BridgeRuntimeError::InvalidObservation
        } else {
            BridgeRuntimeError::AmbiguousDiscovery
        });
    }
    Ok(())
}

fn position_key(position: &ChainPosition) -> (u64, u32) {
    (position.height, position.transaction_index)
}

fn validate_block_range(
    blocks: &[Block],
    start: u64,
    end: u64,
    genesis_hash: Hex32,
) -> Result<(), BridgeRuntimeError> {
    let expected_len = end
        .checked_sub(start)
        .and_then(|distance| distance.checked_add(1))
        .and_then(|length| usize::try_from(length).ok())
        .ok_or(BridgeRuntimeError::InvalidObservation)?;
    if blocks.len() != expected_len {
        return Err(BridgeRuntimeError::InvalidObservation);
    }
    for (offset, block) in blocks.iter().enumerate() {
        let height = start
            .checked_add(u64::try_from(offset).map_err(|_| BridgeRuntimeError::InvalidObservation)?)
            .ok_or(BridgeRuntimeError::InvalidObservation)?;
        if block.header.block_id != height
            || (height == nssa::GENESIS_BLOCK_ID && block.header.hash.0 != *genesis_hash.as_bytes())
        {
            return Err(BridgeRuntimeError::InvalidObservation);
        }
        if let Some(previous) = offset.checked_sub(1).and_then(|index| blocks.get(index))
            && block.header.prev_block_hash != previous.header.hash
        {
            return Err(BridgeRuntimeError::InvalidObservation);
        }
    }
    Ok(())
}
