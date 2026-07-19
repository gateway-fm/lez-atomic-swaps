use std::sync::Arc;

use borsh::{BorshDeserialize as _, to_vec};
use common::transaction::LeeTransaction;
use indexer_service_protocol::{
    BlockHeader, PublicTransaction as IndexedPublicTransaction, Transaction as IndexedTransaction,
};
use lez_bridge_protocol::{
    AccountIds, AggregateBip340Signature, ChainPosition,
    ClassifyFinalizedWitnessedAssetClaimV2Request, ClassifyFinalizedWitnessedAssetClaimV2Result,
    ClassifyFinalizedWitnessedAssetCustodyCreationV2Request,
    ClassifyFinalizedWitnessedAssetCustodyCreationV2Result,
    ClassifyFinalizedWitnessedAssetFundingV2Request,
    ClassifyFinalizedWitnessedAssetFundingV2Result,
    ClassifyFinalizedWitnessedAssetInitializationV2Request,
    ClassifyFinalizedWitnessedAssetInitializationV2Result, DiscoveryWindow, EscrowState,
    FinalizedBlockIdentity, FinalizedWitnessedAssetClaimFactsV2,
    FinalizedWitnessedAssetCustodyCreationFactsV2, FinalizedWitnessedAssetFundingFactsV2,
    FinalizedWitnessedAssetInitializationFactsV2, FinalizedWitnessedAssetTransactionTargetV2,
    FinalizedWitnessedAssetUnavailableReasonV2, Hex32, NativeRefundObservationTarget,
    ObserveWitnessedAssetRefundV2Request, ObserveWitnessedAssetRefundV2Result,
    ObservedTransactionFacts, PreparedTransaction, PreparedWitnessedClaim, RuntimeDescriptor,
    TokenHoldingFactsV2, WitnessedAssetClaimInstructionFactsV2, WitnessedAssetCustodyFactsV2,
    WitnessedAssetEffectInstructionFactsV2, WitnessedAssetInitializationCustodyFactsV2,
    WitnessedAssetPrepareStepV2, WitnessedAssetRefundFoundFactsV2,
    WitnessedAssetRefundInstructionFactsV2, WitnessedAssetRefundObservationV2,
    WitnessedEscrowMetadataFacts, WitnessedLezAssetTermsV2, WitnessedLezAssetV2,
};
use lez_zec_escrow_v02::{ClaimAuthority, EscrowMetadata, EscrowStatus};
use nssa::{AccountId, PublicKey, PublicTransaction};
use token_core::{TokenDefinition, TokenHolding};

use crate::{
    BridgeRuntimeError, FinalizedIndexerApi, HistoricalAccount, ZecEscrowInstruction,
    compute_custody_pda, compute_metadata_pda,
    finalized_claim_observation::{
        StableFinalizedWindow, decode_indexed_public, read_fixed_finalized_window,
    },
    prepared_from_transaction, program_id_from_hex, program_id_to_hex,
};

#[derive(Clone, Copy, Eq, PartialEq)]
enum EffectKind {
    Initialization,
    CustodyCreation,
    Funding,
    Claim,
    Refund,
}

struct Candidate {
    transaction: ObservedTransactionFacts,
    accounts: AccountIds,
    block: BlockHeader,
    public: PublicTransaction,
}

enum Scan {
    Found(Box<Candidate>, Box<StableFinalizedWindow>),
    Missing(Box<StableFinalizedWindow>),
    Unavailable(FinalizedWitnessedAssetUnavailableReasonV2),
}

enum MissingEffectClassification {
    Absent,
    Uncertain,
    Unavailable(FinalizedWitnessedAssetUnavailableReasonV2),
}

fn missing_state_error(
    error: BridgeRuntimeError,
) -> Result<MissingEffectClassification, BridgeRuntimeError> {
    match error {
        BridgeRuntimeError::MovingTip => Ok(MissingEffectClassification::Unavailable(
            FinalizedWitnessedAssetUnavailableReasonV2::MovingTip,
        )),
        BridgeRuntimeError::Unavailable => Ok(MissingEffectClassification::Unavailable(
            FinalizedWitnessedAssetUnavailableReasonV2::HistoryUnavailable,
        )),
        error => Err(error),
    }
}

/// Stable finalized native-or-token effect classifier for additive v2 routes.
pub(crate) struct FinalizedAssetObserver {
    runtime: RuntimeDescriptor,
    indexer: Arc<dyn FinalizedIndexerApi>,
}

impl FinalizedAssetObserver {
    pub(crate) fn new(runtime: RuntimeDescriptor, indexer: Arc<dyn FinalizedIndexerApi>) -> Self {
        Self { runtime, indexer }
    }

    #[allow(
        clippy::too_many_lines,
        reason = "the four typed scan outcomes stay explicit at the initialization boundary"
    )]
    pub(crate) async fn classify_initialization(
        &self,
        request: &ClassifyFinalizedWitnessedAssetInitializationV2Request,
    ) -> Result<ClassifyFinalizedWitnessedAssetInitializationV2Result, BridgeRuntimeError> {
        self.validate_request(&request.runtime, &request.terms)?;
        let target = request.target.clone();
        Ok(
            match self
                .scan(
                    &request.terms,
                    &target,
                    request.window,
                    EffectKind::Initialization,
                    None,
                )
                .await?
            {
                Scan::Found(candidate, stable) => {
                    let (metadata, custody) = self
                        .read_initialization_state(&request.terms, candidate.block.block_id)
                        .await?;
                    if let Some(reason) = self
                        .post_candidate_state_unavailable(&stable, candidate.block.block_id)
                        .await?
                    {
                        return Ok(
                            ClassifyFinalizedWitnessedAssetInitializationV2Result::unavailable(
                                request.context.clone(),
                                request.terms.clone(),
                                target,
                                reason,
                            ),
                        );
                    }
                    let clock = stable.finalized_clock;
                    let facts = FinalizedWitnessedAssetInitializationFactsV2::new(
                        candidate.transaction,
                        WitnessedAssetEffectInstructionFactsV2::new(
                            WitnessedAssetPrepareStepV2::InitializeWitnessed,
                            self.runtime.escrow_program_id,
                            candidate.accounts,
                            swap_id(&request.terms),
                        ),
                        containing_block(&candidate.block),
                        metadata,
                        custody,
                    );
                    ClassifyFinalizedWitnessedAssetInitializationV2Result::found(
                        request.context.clone(),
                        request.terms.clone(),
                        target,
                        clock,
                        request.window,
                        facts,
                    )
                    .map_err(|_| BridgeRuntimeError::InvalidObservation)?
                }
                Scan::Missing(stable) => {
                    let clock = stable.requested_end_clock()?;
                    match self
                        .classify_missing_effect(
                            &request.terms,
                            EffectKind::Initialization,
                            &stable,
                        )
                        .await?
                    {
                        MissingEffectClassification::Absent => {
                            ClassifyFinalizedWitnessedAssetInitializationV2Result::absent(
                                request.context.clone(),
                                request.terms.clone(),
                                target,
                                clock,
                                request.window,
                            )
                            .map_err(|_| BridgeRuntimeError::InvalidObservation)?
                        }
                        MissingEffectClassification::Uncertain => {
                            ClassifyFinalizedWitnessedAssetInitializationV2Result::uncertain(
                                request.context.clone(),
                                request.terms.clone(),
                                target,
                                clock,
                                request.window,
                            )
                            .map_err(|_| BridgeRuntimeError::InvalidObservation)?
                        }
                        MissingEffectClassification::Unavailable(reason) => {
                            ClassifyFinalizedWitnessedAssetInitializationV2Result::unavailable(
                                request.context.clone(),
                                request.terms.clone(),
                                target,
                                reason,
                            )
                        }
                    }
                }
                Scan::Unavailable(reason) => {
                    ClassifyFinalizedWitnessedAssetInitializationV2Result::unavailable(
                        request.context.clone(),
                        request.terms.clone(),
                        target,
                        reason,
                    )
                }
            },
        )
    }

    #[allow(
        clippy::too_many_lines,
        reason = "the four typed scan outcomes stay explicit at the custody boundary"
    )]
    pub(crate) async fn classify_custody_creation(
        &self,
        request: &ClassifyFinalizedWitnessedAssetCustodyCreationV2Request,
    ) -> Result<ClassifyFinalizedWitnessedAssetCustodyCreationV2Result, BridgeRuntimeError> {
        self.validate_request(&request.runtime, &request.terms)?;
        if request.terms.asset().custom_token().is_none() {
            return Err(BridgeRuntimeError::Planner);
        }
        let target = request.target.clone();
        Ok(
            match self
                .scan(
                    &request.terms,
                    &target,
                    request.window,
                    EffectKind::CustodyCreation,
                    None,
                )
                .await?
            {
                Scan::Found(candidate, stable) => {
                    let (metadata, custody) = self
                        .read_asset_state(
                            &request.terms,
                            candidate.block.block_id,
                            EscrowState::Empty,
                            0,
                        )
                        .await?;
                    if let Some(reason) = self
                        .post_candidate_state_unavailable(&stable, candidate.block.block_id)
                        .await?
                    {
                        return ClassifyFinalizedWitnessedAssetCustodyCreationV2Result::unavailable(
                            request.context.clone(),
                            request.terms.clone(),
                            target,
                            reason,
                        )
                        .map_err(|_| BridgeRuntimeError::InvalidObservation);
                    }
                    let clock = stable.finalized_clock;
                    let WitnessedAssetCustodyFactsV2::CustomToken(custody) = custody else {
                        return Err(BridgeRuntimeError::InvalidObservation);
                    };
                    let facts = FinalizedWitnessedAssetCustodyCreationFactsV2::new(
                        candidate.transaction,
                        WitnessedAssetEffectInstructionFactsV2::new(
                            WitnessedAssetPrepareStepV2::CreateCustodyAta,
                            self.runtime.escrow_program_id,
                            candidate.accounts,
                            swap_id(&request.terms),
                        ),
                        containing_block(&candidate.block),
                        metadata,
                        custody,
                    );
                    ClassifyFinalizedWitnessedAssetCustodyCreationV2Result::found(
                        request.context.clone(),
                        request.terms.clone(),
                        target,
                        clock,
                        request.window,
                        facts,
                    )
                    .map_err(|_| BridgeRuntimeError::InvalidObservation)?
                }
                Scan::Missing(stable) => {
                    let clock = stable.requested_end_clock()?;
                    match self
                        .classify_missing_effect(
                            &request.terms,
                            EffectKind::CustodyCreation,
                            &stable,
                        )
                        .await?
                    {
                        MissingEffectClassification::Absent => {
                            ClassifyFinalizedWitnessedAssetCustodyCreationV2Result::absent(
                                request.context.clone(),
                                request.terms.clone(),
                                target,
                                clock,
                                request.window,
                            )
                            .map_err(|_| BridgeRuntimeError::InvalidObservation)?
                        }
                        MissingEffectClassification::Uncertain => {
                            ClassifyFinalizedWitnessedAssetCustodyCreationV2Result::uncertain(
                                request.context.clone(),
                                request.terms.clone(),
                                target,
                                clock,
                                request.window,
                            )
                            .map_err(|_| BridgeRuntimeError::InvalidObservation)?
                        }
                        MissingEffectClassification::Unavailable(reason) => {
                            ClassifyFinalizedWitnessedAssetCustodyCreationV2Result::unavailable(
                                request.context.clone(),
                                request.terms.clone(),
                                target,
                                reason,
                            )
                            .map_err(|_| BridgeRuntimeError::InvalidObservation)?
                        }
                    }
                }
                Scan::Unavailable(reason) => {
                    ClassifyFinalizedWitnessedAssetCustodyCreationV2Result::unavailable(
                        request.context.clone(),
                        request.terms.clone(),
                        target,
                        reason,
                    )
                    .map_err(|_| BridgeRuntimeError::InvalidObservation)?
                }
            },
        )
    }

    #[allow(
        clippy::too_many_lines,
        reason = "the four typed scan outcomes stay explicit at the funding boundary"
    )]
    pub(crate) async fn classify_funding(
        &self,
        request: &ClassifyFinalizedWitnessedAssetFundingV2Request,
    ) -> Result<ClassifyFinalizedWitnessedAssetFundingV2Result, BridgeRuntimeError> {
        self.validate_request(&request.runtime, &request.terms)?;
        let target = request.target.clone();
        Ok(
            match self
                .scan(
                    &request.terms,
                    &target,
                    request.window,
                    EffectKind::Funding,
                    None,
                )
                .await?
            {
                Scan::Found(candidate, stable) => {
                    let amount = amount(&request.terms);
                    let (metadata, custody) = self
                        .read_asset_state(
                            &request.terms,
                            candidate.block.block_id,
                            EscrowState::Funded,
                            amount,
                        )
                        .await?;
                    if let Some(reason) = self
                        .post_candidate_state_unavailable(&stable, candidate.block.block_id)
                        .await?
                    {
                        return Ok(ClassifyFinalizedWitnessedAssetFundingV2Result::unavailable(
                            request.context.clone(),
                            request.terms.clone(),
                            target,
                            reason,
                        ));
                    }
                    let clock = stable.finalized_clock;
                    let facts = FinalizedWitnessedAssetFundingFactsV2::new(
                        candidate.transaction,
                        WitnessedAssetEffectInstructionFactsV2::new(
                            WitnessedAssetPrepareStepV2::Fund,
                            self.runtime.escrow_program_id,
                            candidate.accounts,
                            swap_id(&request.terms),
                        ),
                        containing_block(&candidate.block),
                        metadata,
                        custody,
                    );
                    ClassifyFinalizedWitnessedAssetFundingV2Result::found(
                        request.context.clone(),
                        request.terms.clone(),
                        target,
                        clock,
                        request.window,
                        facts,
                    )
                    .map_err(|_| BridgeRuntimeError::InvalidObservation)?
                }
                Scan::Missing(stable) => {
                    let clock = stable.requested_end_clock()?;
                    match self
                        .classify_missing_effect(&request.terms, EffectKind::Funding, &stable)
                        .await?
                    {
                        MissingEffectClassification::Absent => {
                            ClassifyFinalizedWitnessedAssetFundingV2Result::absent(
                                request.context.clone(),
                                request.terms.clone(),
                                target,
                                clock,
                                request.window,
                            )
                            .map_err(|_| BridgeRuntimeError::InvalidObservation)?
                        }
                        MissingEffectClassification::Uncertain => {
                            ClassifyFinalizedWitnessedAssetFundingV2Result::uncertain(
                                request.context.clone(),
                                request.terms.clone(),
                                target,
                                clock,
                                request.window,
                            )
                            .map_err(|_| BridgeRuntimeError::InvalidObservation)?
                        }
                        MissingEffectClassification::Unavailable(reason) => {
                            ClassifyFinalizedWitnessedAssetFundingV2Result::unavailable(
                                request.context.clone(),
                                request.terms.clone(),
                                target,
                                reason,
                            )
                        }
                    }
                }
                Scan::Unavailable(reason) => {
                    ClassifyFinalizedWitnessedAssetFundingV2Result::unavailable(
                        request.context.clone(),
                        request.terms.clone(),
                        target,
                        reason,
                    )
                }
            },
        )
    }

    #[allow(
        clippy::too_many_lines,
        reason = "the four typed scan outcomes stay explicit at the claim boundary"
    )]
    pub(crate) async fn classify_claim(
        &self,
        request: &ClassifyFinalizedWitnessedAssetClaimV2Request,
    ) -> Result<ClassifyFinalizedWitnessedAssetClaimV2Result, BridgeRuntimeError> {
        self.validate_request(&request.runtime, &request.terms)?;
        validate_claim_transcript(&request.claim)?;
        let target = request.target.clone();
        Ok(
            match self
                .scan(
                    &request.terms,
                    &target,
                    request.window,
                    EffectKind::Claim,
                    Some(&request.claim),
                )
                .await?
            {
                Scan::Found(candidate, stable) => {
                    let (metadata, custody) = self
                        .read_asset_state(
                            &request.terms,
                            candidate.block.block_id,
                            EscrowState::Claimed,
                            0,
                        )
                        .await?;
                    if let Some(reason) = self
                        .post_candidate_state_unavailable(&stable, candidate.block.block_id)
                        .await?
                    {
                        return Ok(ClassifyFinalizedWitnessedAssetClaimV2Result::unavailable(
                            request.context.clone(),
                            request.terms.clone(),
                            request.claim.clone(),
                            target,
                            reason,
                        ));
                    }
                    let clock = stable.finalized_clock;
                    let [(signature, _)] =
                        candidate.public.witness_set().signatures_and_public_keys()
                    else {
                        return Err(BridgeRuntimeError::InvalidObservation);
                    };
                    let facts = FinalizedWitnessedAssetClaimFactsV2::new(
                        candidate.transaction,
                        WitnessedAssetClaimInstructionFactsV2::new(
                            self.runtime.escrow_program_id,
                            candidate.accounts,
                            swap_id(&request.terms),
                            request.claim.clone(),
                        ),
                        AggregateBip340Signature::from_bytes(signature.value),
                        containing_block(&candidate.block),
                        metadata,
                        custody,
                    );
                    ClassifyFinalizedWitnessedAssetClaimV2Result::found(
                        request.context.clone(),
                        request.terms.clone(),
                        request.claim.clone(),
                        target,
                        clock,
                        request.window,
                        facts,
                    )
                    .map_err(|_| BridgeRuntimeError::InvalidObservation)?
                }
                Scan::Missing(stable) => {
                    let clock = stable.requested_end_clock()?;
                    match self
                        .classify_missing_effect(&request.terms, EffectKind::Claim, &stable)
                        .await?
                    {
                        MissingEffectClassification::Absent => {
                            ClassifyFinalizedWitnessedAssetClaimV2Result::absent(
                                request.context.clone(),
                                request.terms.clone(),
                                request.claim.clone(),
                                target,
                                clock,
                                request.window,
                            )
                            .map_err(|_| BridgeRuntimeError::InvalidObservation)?
                        }
                        MissingEffectClassification::Uncertain => {
                            ClassifyFinalizedWitnessedAssetClaimV2Result::uncertain(
                                request.context.clone(),
                                request.terms.clone(),
                                request.claim.clone(),
                                target,
                                clock,
                                request.window,
                            )
                            .map_err(|_| BridgeRuntimeError::InvalidObservation)?
                        }
                        MissingEffectClassification::Unavailable(reason) => {
                            ClassifyFinalizedWitnessedAssetClaimV2Result::unavailable(
                                request.context.clone(),
                                request.terms.clone(),
                                request.claim.clone(),
                                target,
                                reason,
                            )
                        }
                    }
                }
                Scan::Unavailable(reason) => {
                    ClassifyFinalizedWitnessedAssetClaimV2Result::unavailable(
                        request.context.clone(),
                        request.terms.clone(),
                        request.claim.clone(),
                        target,
                        reason,
                    )
                }
            },
        )
    }

    pub(crate) async fn observe_refund(
        &self,
        request: &ObserveWitnessedAssetRefundV2Request,
    ) -> Result<ObserveWitnessedAssetRefundV2Result, BridgeRuntimeError> {
        self.validate_request(&request.runtime, &request.terms)?;
        let scanned = match request.target {
            NativeRefundObservationTarget::StateOnly => None,
            NativeRefundObservationTarget::Exact { window, .. }
            | NativeRefundObservationTarget::DiscoverByTerms { window } => {
                let discovery = FinalizedWitnessedAssetTransactionTargetV2::DiscoverByTerms {};
                Some(
                    self.scan(&request.terms, &discovery, window, EffectKind::Refund, None)
                        .await?,
                )
            }
        };
        let (refund, stable) = match scanned {
            None => (WitnessedAssetRefundObservationV2::NotRequested, None),
            Some(Scan::Missing(stable)) => {
                (WitnessedAssetRefundObservationV2::Absent, Some(stable))
            }
            Some(Scan::Unavailable(_)) => {
                (WitnessedAssetRefundObservationV2::UnknownOrPending, None)
            }
            Some(Scan::Found(candidate, stable)) => {
                if let NativeRefundObservationTarget::Exact {
                    refund_transaction_id,
                    ..
                } = request.target
                    && candidate.transaction.transaction_id != refund_transaction_id
                {
                    return Err(BridgeRuntimeError::ConflictingDiscovery);
                }
                (
                    WitnessedAssetRefundObservationV2::found(
                        WitnessedAssetRefundFoundFactsV2::new(
                            candidate.transaction,
                            WitnessedAssetRefundInstructionFactsV2::new(
                                self.runtime.escrow_program_id,
                                candidate.accounts,
                                swap_id(&request.terms),
                            ),
                        ),
                    ),
                    Some(stable),
                )
            }
        };
        let (clock, metadata, custody, refund) = if let Some(stable) = stable {
            match self
                .read_asset_state_at_stable_tip(&request.terms, &stable)
                .await
            {
                Ok((clock, metadata, custody)) => (clock, metadata, custody, refund),
                Err(BridgeRuntimeError::MovingTip | BridgeRuntimeError::Unavailable) => {
                    let (clock, metadata, custody) =
                        self.read_latest_asset_state(&request.terms).await?;
                    (
                        clock,
                        metadata,
                        custody,
                        WitnessedAssetRefundObservationV2::UnknownOrPending,
                    )
                }
                Err(error) => return Err(error),
            }
        } else {
            let (clock, metadata, custody) = self.read_latest_asset_state(&request.terms).await?;
            (clock, metadata, custody, refund)
        };
        ObserveWitnessedAssetRefundV2Result::new(
            request.context.clone(),
            request.terms.clone(),
            clock,
            metadata,
            custody,
            refund,
            clock,
        )
        .map_err(|_| BridgeRuntimeError::InvalidObservation)
    }

    async fn read_latest_asset_state(
        &self,
        terms: &WitnessedLezAssetTermsV2,
    ) -> Result<
        (
            lez_bridge_protocol::ChainClock,
            WitnessedEscrowMetadataFacts,
            WitnessedAssetCustodyFactsV2,
        ),
        BridgeRuntimeError,
    > {
        let tip = self
            .indexer
            .last_finalized_block_id()
            .await?
            .ok_or(BridgeRuntimeError::Unavailable)?;
        let window =
            DiscoveryWindow::new(tip, 1).map_err(|_| BridgeRuntimeError::InvalidObservation)?;
        let stable = read_fixed_finalized_window(self.indexer.as_ref(), window).await?;
        self.read_asset_state_at_stable_tip(terms, &stable).await
    }

    async fn read_asset_state_at_stable_tip(
        &self,
        terms: &WitnessedLezAssetTermsV2,
        stable: &StableFinalizedWindow,
    ) -> Result<
        (
            lez_bridge_protocol::ChainClock,
            WitnessedEscrowMetadataFacts,
            WitnessedAssetCustodyFactsV2,
        ),
        BridgeRuntimeError,
    > {
        let tip = stable.finalized_tip.header.block_id;
        let state = self.read_metadata_state(terms, tip).await?;
        let balance = if state == EscrowState::Funded {
            amount(terms)
        } else {
            0
        };
        let (metadata, custody) = self.read_asset_state(terms, tip, state, balance).await?;
        stable
            .confirm_pinned_snapshot(self.indexer.as_ref())
            .await?;
        Ok((stable.finalized_clock, metadata, custody))
    }

    async fn post_candidate_state_unavailable(
        &self,
        stable: &StableFinalizedWindow,
        candidate_block_id: u64,
    ) -> Result<Option<FinalizedWitnessedAssetUnavailableReasonV2>, BridgeRuntimeError> {
        match stable
            .confirm_block(self.indexer.as_ref(), candidate_block_id)
            .await
        {
            Ok(()) => Ok(None),
            Err(BridgeRuntimeError::MovingTip) => {
                Ok(Some(FinalizedWitnessedAssetUnavailableReasonV2::MovingTip))
            }
            Err(BridgeRuntimeError::Unavailable) => Ok(Some(
                FinalizedWitnessedAssetUnavailableReasonV2::HistoryUnavailable,
            )),
            Err(error) => Err(error),
        }
    }

    async fn classify_missing_effect(
        &self,
        terms: &WitnessedLezAssetTermsV2,
        kind: EffectKind,
        stable: &StableFinalizedWindow,
    ) -> Result<MissingEffectClassification, BridgeRuntimeError> {
        let tip = stable.requested_end;
        let state_proves_absence = match kind {
            EffectKind::Initialization => {
                let metadata_id = compute_metadata_pda(
                    &program_id_from_hex(self.runtime.escrow_program_id),
                    swap_id(terms).as_bytes(),
                );
                match self
                    .indexer
                    .account_at_block(metadata_id.into_value(), tip)
                    .await
                {
                    Ok(HistoricalAccount::Absent) => true,
                    Ok(HistoricalAccount::Present(_)) => false,
                    Err(error) => return missing_state_error(error),
                }
            }
            EffectKind::CustodyCreation => match self.read_initialization_state(terms, tip).await {
                Ok(_) => true,
                Err(BridgeRuntimeError::InvalidObservation) => false,
                Err(error) => return missing_state_error(error),
            },
            EffectKind::Funding => {
                match self
                    .read_asset_state(terms, tip, EscrowState::Empty, 0)
                    .await
                {
                    Ok(_) => true,
                    Err(BridgeRuntimeError::InvalidObservation) => false,
                    Err(error) => return missing_state_error(error),
                }
            }
            EffectKind::Claim => {
                match self
                    .read_asset_state(terms, tip, EscrowState::Funded, amount(terms))
                    .await
                {
                    Ok(_) => true,
                    Err(BridgeRuntimeError::InvalidObservation) => false,
                    Err(error) => return missing_state_error(error),
                }
            }
            EffectKind::Refund => false,
        };
        if !state_proves_absence {
            return Ok(MissingEffectClassification::Uncertain);
        }
        match stable.confirm_requested_end(self.indexer.as_ref()).await {
            Ok(()) => Ok(MissingEffectClassification::Absent),
            Err(error) => missing_state_error(error),
        }
    }

    async fn scan(
        &self,
        terms: &WitnessedLezAssetTermsV2,
        target: &FinalizedWitnessedAssetTransactionTargetV2,
        window: DiscoveryWindow,
        kind: EffectKind,
        claim: Option<&PreparedWitnessedClaim>,
    ) -> Result<Scan, BridgeRuntimeError> {
        let stable = match read_fixed_finalized_window(self.indexer.as_ref(), window).await {
            Ok(stable) => stable,
            Err(BridgeRuntimeError::MovingTip) => {
                return Ok(Scan::Unavailable(
                    FinalizedWitnessedAssetUnavailableReasonV2::MovingTip,
                ));
            }
            Err(BridgeRuntimeError::Unavailable) => {
                return Ok(Scan::Unavailable(
                    FinalizedWitnessedAssetUnavailableReasonV2::HistoryUnavailable,
                ));
            }
            Err(error) => return Err(error),
        };
        let mut found = None;
        for block in &stable.blocks {
            if block.header.block_id > stable.requested_end {
                continue;
            }
            for (index, transaction) in block.body.transactions.iter().enumerate() {
                let exact = match target {
                    FinalizedWitnessedAssetTransactionTargetV2::Exact { transaction: exact } => {
                        if transaction.hash().0 != *exact.transaction_id.as_bytes() {
                            continue;
                        }
                        Some(exact)
                    }
                    FinalizedWitnessedAssetTransactionTargetV2::DiscoverByTerms {} => None,
                };
                let IndexedTransaction::Public(indexed) = transaction else {
                    if exact.is_some() {
                        return Err(BridgeRuntimeError::InvalidObservation);
                    }
                    continue;
                };
                let candidate =
                    self.candidate(terms, indexed, &block.header, index, kind, claim, exact)?;
                let Some(candidate) = candidate else {
                    continue;
                };
                if found.replace(candidate).is_some() {
                    return Ok(Scan::Unavailable(
                        FinalizedWitnessedAssetUnavailableReasonV2::ConflictingMatches,
                    ));
                }
            }
        }
        Ok(match found {
            Some(candidate) => {
                if let Err(error) = stable
                    .confirm_block(self.indexer.as_ref(), candidate.block.block_id)
                    .await
                {
                    return Ok(Scan::Unavailable(match error {
                        BridgeRuntimeError::MovingTip => {
                            FinalizedWitnessedAssetUnavailableReasonV2::MovingTip
                        }
                        _ => FinalizedWitnessedAssetUnavailableReasonV2::HistoryUnavailable,
                    }));
                }
                Scan::Found(Box::new(candidate), Box::new(stable))
            }
            None => Scan::Missing(Box::new(stable)),
        })
    }

    #[allow(clippy::too_many_arguments)]
    #[allow(
        clippy::too_many_lines,
        reason = "canonical bytes, instruction, accounts, signers, and claim transcript are one fail-closed candidate check"
    )]
    fn candidate(
        &self,
        terms: &WitnessedLezAssetTermsV2,
        indexed: &IndexedPublicTransaction,
        block: &BlockHeader,
        index: usize,
        kind: EffectKind,
        claim: Option<&PreparedWitnessedClaim>,
        exact: Option<&PreparedTransaction>,
    ) -> Result<Option<Candidate>, BridgeRuntimeError> {
        let escrow_program = program_id_from_hex(self.runtime.escrow_program_id);
        if indexed.message.program_id.0 != escrow_program {
            return if exact.is_some() {
                Err(BridgeRuntimeError::InvalidObservation)
            } else {
                Ok(None)
            };
        }
        let public = decode_indexed_public(indexed)?;
        if public.hash() != indexed.hash.0 {
            return Err(BridgeRuntimeError::InvalidObservation);
        }
        LeeTransaction::Public(public.clone())
            .transaction_stateless_check()
            .map_err(|_| BridgeRuntimeError::InvalidObservation)?;
        let prepared = prepared_from_transaction(&public)?;
        if exact.is_some_and(|expected| expected != &prepared) {
            return Err(BridgeRuntimeError::InvalidObservation);
        }
        let instruction = risc0_zkvm::serde::from_slice::<ZecEscrowInstruction, u32>(
            &public.message().instruction_data,
        )
        .map_err(|_| {
            if exact.is_some() {
                BridgeRuntimeError::InvalidObservation
            } else {
                BridgeRuntimeError::ConflictingDiscovery
            }
        })?;
        let observed_kind = if instruction_matches(terms, kind, &instruction) {
            kind
        } else if exact.is_some() {
            return Err(BridgeRuntimeError::InvalidObservation);
        } else if instruction_swap(&instruction) != Some(swap_id(terms)) {
            return Ok(None);
        } else {
            let observed_kind =
                instruction_kind(&instruction).ok_or(BridgeRuntimeError::ConflictingDiscovery)?;
            if observed_kind == kind || !instruction_matches(terms, observed_kind, &instruction) {
                return Err(BridgeRuntimeError::ConflictingDiscovery);
            }
            observed_kind
        };
        let expected_accounts =
            expected_accounts(terms, self.runtime.escrow_program_id, observed_kind);
        let observed_accounts = AccountIds::new(
            public
                .message()
                .account_ids
                .iter()
                .map(|account| Hex32::from_bytes(account.into_value()))
                .collect(),
        )
        .map_err(|_| BridgeRuntimeError::InvalidObservation)?;
        if observed_accounts != expected_accounts {
            return Err(if exact.is_some() {
                BridgeRuntimeError::InvalidObservation
            } else {
                BridgeRuntimeError::ConflictingDiscovery
            });
        }
        let signer_ids = AccountIds::new(
            public
                .witness_set()
                .signatures_and_public_keys()
                .iter()
                .map(|(_, key)| Hex32::from_bytes(AccountId::from(key).into_value()))
                .collect(),
        )
        .map_err(|_| BridgeRuntimeError::InvalidObservation)?;
        if signer_ids != expected_signers(terms, observed_kind) {
            return Err(if exact.is_some() {
                BridgeRuntimeError::InvalidObservation
            } else {
                BridgeRuntimeError::ConflictingDiscovery
            });
        }
        if observed_kind != kind {
            return Ok(None);
        }
        if let Some(claim) = claim {
            let exact_message =
                to_vec(public.message()).map_err(|_| BridgeRuntimeError::InvalidObservation)?;
            if exact_message != claim.exact_message_bytes.as_slice()
                || public.message().hash() != *claim.message_hash.as_bytes()
            {
                return Err(if exact.is_some() {
                    BridgeRuntimeError::InvalidObservation
                } else {
                    BridgeRuntimeError::ConflictingDiscovery
                });
            }
        }
        Ok(Some(Candidate {
            transaction: ObservedTransactionFacts::new(
                prepared.transaction_id,
                prepared.exact_bytes,
                ChainPosition::new(
                    Hex32::from_bytes(block.hash.0),
                    block.block_id,
                    u32::try_from(index).map_err(|_| BridgeRuntimeError::InvalidObservation)?,
                ),
                signer_ids,
                true,
            ),
            accounts: observed_accounts,
            block: block.clone(),
            public,
        }))
    }

    fn validate_request(
        &self,
        runtime: &RuntimeDescriptor,
        terms: &WitnessedLezAssetTermsV2,
    ) -> Result<(), BridgeRuntimeError> {
        if runtime != &self.runtime || runtime.escrow_program_id != self.runtime.escrow_program_id {
            return Err(BridgeRuntimeError::Planner);
        }
        let signer = match terms.asset() {
            WitnessedLezAssetV2::Native(terms) => {
                if self.runtime.sidecar_role == terms.depositor() {
                    terms.depositor_account_id()
                } else if self.runtime.sidecar_role == terms.claimant() {
                    terms.claimant_account_id()
                } else {
                    return Err(BridgeRuntimeError::Planner);
                }
            }
            WitnessedLezAssetV2::CustomToken(terms) => {
                validate_token_terms(terms, self.runtime.escrow_program_id)?;
                if self.runtime.sidecar_role == terms.depositor() {
                    terms.depositor_owner_account_id()
                } else if self.runtime.sidecar_role == terms.claimant() {
                    terms.claimant_owner_account_id()
                } else {
                    return Err(BridgeRuntimeError::Planner);
                }
            }
        };
        if signer != self.runtime.signer_account_id {
            return Err(BridgeRuntimeError::Planner);
        }
        Ok(())
    }

    async fn read_initialization_state(
        &self,
        terms: &WitnessedLezAssetTermsV2,
        block_id: u64,
    ) -> Result<
        (
            WitnessedEscrowMetadataFacts,
            WitnessedAssetInitializationCustodyFactsV2,
        ),
        BridgeRuntimeError,
    > {
        match terms.asset() {
            WitnessedLezAssetV2::Native(_) => {
                let (metadata, custody) = self
                    .read_asset_state(terms, block_id, EscrowState::Empty, 0)
                    .await?;
                let WitnessedAssetCustodyFactsV2::Native(custody) = custody else {
                    return Err(BridgeRuntimeError::InvalidObservation);
                };
                Ok((
                    metadata,
                    WitnessedAssetInitializationCustodyFactsV2::native(custody),
                ))
            }
            WitnessedLezAssetV2::CustomToken(token) => {
                let metadata = self.read_metadata(terms, block_id, EscrowState::Empty);
                let definition = self.read_token_definition(token, block_id);
                let custody = self
                    .indexer
                    .account_at_block(*token.custody_ata_account_id().as_bytes(), block_id);
                let ((metadata, _), (), custody) = tokio::try_join!(metadata, definition, custody)?;
                match custody {
                    HistoricalAccount::Absent => Ok((
                        metadata,
                        WitnessedAssetInitializationCustodyFactsV2::custom_token_ata_absent(
                            token.custody_ata_account_id(),
                        ),
                    )),
                    HistoricalAccount::Present(_) => Err(BridgeRuntimeError::InvalidObservation),
                }
            }
        }
    }

    async fn read_asset_state(
        &self,
        terms: &WitnessedLezAssetTermsV2,
        block_id: u64,
        expected_state: EscrowState,
        expected_balance: u128,
    ) -> Result<(WitnessedEscrowMetadataFacts, WitnessedAssetCustodyFactsV2), BridgeRuntimeError>
    {
        match terms.asset() {
            WitnessedLezAssetV2::Native(native) => {
                let (metadata, custody_id) =
                    self.read_metadata(terms, block_id, expected_state).await?;
                let account = require_present(
                    self.indexer
                        .account_at_block(*custody_id.as_bytes(), block_id)
                        .await?,
                )?;
                let expected_program =
                    program_id_from_hex(native.authenticated_transfer_program_id());
                if account.program_owner.0 != expected_program
                    || account.balance != expected_balance
                {
                    return Err(BridgeRuntimeError::InvalidObservation);
                }
                Ok((
                    metadata,
                    WitnessedAssetCustodyFactsV2::Native(
                        lez_bridge_protocol::NativeCustodyFacts::new(
                            custody_id,
                            program_id_to_hex(account.program_owner.0),
                            account.balance,
                        ),
                    ),
                ))
            }
            WitnessedLezAssetV2::CustomToken(token) => {
                let metadata = self.read_metadata(terms, block_id, expected_state);
                let definition = self.read_token_definition(token, block_id);
                let custody = self
                    .indexer
                    .account_at_block(*token.custody_ata_account_id().as_bytes(), block_id);
                let ((metadata, _), (), custody) = tokio::try_join!(metadata, definition, custody)?;
                let account = require_present(custody)?;
                if account.program_owner.0 != programs::token().id() {
                    return Err(BridgeRuntimeError::InvalidObservation);
                }
                let holding = TokenHolding::try_from_slice(account.data.0.as_ref())
                    .map_err(|_| BridgeRuntimeError::InvalidObservation)?;
                let TokenHolding::Fungible {
                    definition_id,
                    balance,
                } = holding
                else {
                    return Err(BridgeRuntimeError::InvalidObservation);
                };
                if definition_id.into_value() != *token.token_definition_account_id().as_bytes()
                    || balance != expected_balance
                {
                    return Err(BridgeRuntimeError::InvalidObservation);
                }
                Ok((
                    metadata,
                    WitnessedAssetCustodyFactsV2::CustomToken(TokenHoldingFactsV2::new(
                        token.custody_ata_account_id(),
                        token.token_program_id(),
                        token.token_definition_account_id(),
                        balance,
                    )),
                ))
            }
        }
    }

    async fn read_metadata_state(
        &self,
        terms: &WitnessedLezAssetTermsV2,
        block_id: u64,
    ) -> Result<EscrowState, BridgeRuntimeError> {
        let metadata_id = compute_metadata_pda(
            &program_id_from_hex(self.runtime.escrow_program_id),
            swap_id(terms).as_bytes(),
        );
        let account = require_present(
            self.indexer
                .account_at_block(metadata_id.into_value(), block_id)
                .await?,
        )?;
        let metadata = EscrowMetadata::try_from_slice(account.data.0.as_ref())
            .map_err(|_| BridgeRuntimeError::InvalidObservation)?;
        escrow_state(metadata.status)
    }

    #[allow(
        clippy::too_many_lines,
        reason = "native and token metadata fields are exhaustively compared at one historical-state boundary"
    )]
    async fn read_metadata(
        &self,
        terms: &WitnessedLezAssetTermsV2,
        block_id: u64,
        expected_state: EscrowState,
    ) -> Result<(WitnessedEscrowMetadataFacts, Hex32), BridgeRuntimeError> {
        let escrow_program = program_id_from_hex(self.runtime.escrow_program_id);
        let swap = swap_id(terms);
        let metadata_id = compute_metadata_pda(&escrow_program, swap.as_bytes());
        let account = require_present(
            self.indexer
                .account_at_block(metadata_id.into_value(), block_id)
                .await?,
        )?;
        let metadata = EscrowMetadata::try_from_slice(account.data.0.as_ref())
            .map_err(|_| BridgeRuntimeError::InvalidObservation)?;
        let state = escrow_state(metadata.status)?;
        if account.program_owner.0 != escrow_program
            || metadata.version != 2
            || state != expected_state
        {
            return Err(BridgeRuntimeError::InvalidObservation);
        }
        let metadata_hex = Hex32::from_bytes(metadata_id.into_value());
        match terms.asset() {
            WitnessedLezAssetV2::Native(native) => {
                let custody = compute_custody_pda(&escrow_program, native.swap_id().as_bytes());
                let ClaimAuthority::AggregateWitness {
                    x_only_public_key,
                    account_id,
                } = metadata.claim_authority
                else {
                    return Err(BridgeRuntimeError::InvalidObservation);
                };
                let transfer = program_id_from_hex(native.authenticated_transfer_program_id());
                if metadata.swap_id != *native.swap_id().as_bytes()
                    || metadata.terms_hash != *native.terms_hash().as_bytes()
                    || x_only_public_key != *native.aggregate_x_only_public_key().as_bytes()
                    || account_id.into_value()
                        != *native.aggregate_authority_account_id().as_bytes()
                    || metadata.depositor.into_value() != *native.depositor_account_id().as_bytes()
                    || metadata.depositor_asset != metadata.depositor
                    || metadata.claimant.into_value() != *native.claimant_account_id().as_bytes()
                    || metadata.claimant_asset != metadata.claimant
                    || metadata.custody != custody
                    || metadata.asset_program != transfer
                    || metadata.custody_program != transfer
                    || metadata.asset_definition != [0; 32]
                    || metadata.amount != native.amount().as_u128()
                    || metadata.refund_at != native.refund_at_ms()
                {
                    return Err(BridgeRuntimeError::InvalidObservation);
                }
                let custody_hex = Hex32::from_bytes(custody.into_value());
                Ok((
                    WitnessedEscrowMetadataFacts::from_witnessed_native_terms(
                        metadata_hex,
                        self.runtime.escrow_program_id,
                        custody_hex,
                        native,
                        state,
                    ),
                    custody_hex,
                ))
            }
            WitnessedLezAssetV2::CustomToken(token) => {
                let ClaimAuthority::AggregateWitness {
                    x_only_public_key,
                    account_id,
                } = metadata.claim_authority
                else {
                    return Err(BridgeRuntimeError::InvalidObservation);
                };
                if metadata.swap_id != *token.swap_id().as_bytes()
                    || metadata.terms_hash != *token.terms_hash().as_bytes()
                    || x_only_public_key != *token.aggregate_x_only_public_key().as_bytes()
                    || account_id.into_value() != *token.aggregate_authority_account_id().as_bytes()
                    || metadata.depositor.into_value()
                        != *token.depositor_owner_account_id().as_bytes()
                    || metadata.depositor_asset.into_value()
                        != *token.depositor_ata_account_id().as_bytes()
                    || metadata.claimant.into_value()
                        != *token.claimant_owner_account_id().as_bytes()
                    || metadata.claimant_asset.into_value()
                        != *token.claimant_ata_account_id().as_bytes()
                    || metadata.custody.into_value() != *token.custody_ata_account_id().as_bytes()
                    || metadata.asset_program != programs::token().id()
                    || metadata.custody_program != programs::ata().id()
                    || metadata.asset_definition != *token.token_definition_account_id().as_bytes()
                    || metadata.amount != token.amount().as_u128()
                    || metadata.refund_at != token.refund_at_ms()
                {
                    return Err(BridgeRuntimeError::InvalidObservation);
                }
                Ok((
                    WitnessedEscrowMetadataFacts::from_witnessed_token_terms(
                        metadata_hex,
                        self.runtime.escrow_program_id,
                        token,
                        state,
                    ),
                    token.custody_ata_account_id(),
                ))
            }
        }
    }

    async fn read_token_definition(
        &self,
        terms: &lez_bridge_protocol::WitnessedTokenEscrowTermsV2,
        block_id: u64,
    ) -> Result<(), BridgeRuntimeError> {
        let account = require_present(
            self.indexer
                .account_at_block(*terms.token_definition_account_id().as_bytes(), block_id)
                .await?,
        )?;
        if account.program_owner.0 != programs::token().id()
            || !matches!(
                TokenDefinition::try_from_slice(account.data.0.as_ref()),
                Ok(TokenDefinition::Fungible { .. })
            )
        {
            return Err(BridgeRuntimeError::InvalidObservation);
        }
        Ok(())
    }
}

fn validate_token_terms(
    terms: &lez_bridge_protocol::WitnessedTokenEscrowTermsV2,
    escrow_program: Hex32,
) -> Result<(), BridgeRuntimeError> {
    if terms.token_program_id() != program_id_to_hex(programs::token().id())
        || terms.ata_program_id() != program_id_to_hex(programs::ata().id())
    {
        return Err(BridgeRuntimeError::Planner);
    }
    let definition = AccountId::new(*terms.token_definition_account_id().as_bytes());
    let depositor = AccountId::new(*terms.depositor_owner_account_id().as_bytes());
    let claimant = AccountId::new(*terms.claimant_owner_account_id().as_bytes());
    let metadata = compute_metadata_pda(
        &program_id_from_hex(escrow_program),
        terms.swap_id().as_bytes(),
    );
    if official_ata(depositor, definition) != terms.depositor_ata_account_id()
        || official_ata(claimant, definition) != terms.claimant_ata_account_id()
        || official_ata(metadata, definition) != terms.custody_ata_account_id()
    {
        return Err(BridgeRuntimeError::Planner);
    }
    let key = PublicKey::try_new(*terms.aggregate_x_only_public_key().as_bytes())
        .map_err(|_| BridgeRuntimeError::Planner)?;
    if AccountId::from(&key).into_value() != *terms.aggregate_authority_account_id().as_bytes() {
        return Err(BridgeRuntimeError::Planner);
    }
    Ok(())
}

fn official_ata(owner: AccountId, definition: AccountId) -> Hex32 {
    let seed = ata_core::compute_ata_seed(owner, definition);
    Hex32::from_bytes(
        ata_core::get_associated_token_account_id(&programs::ata().id(), &seed).into_value(),
    )
}

fn instruction_matches(
    terms: &WitnessedLezAssetTermsV2,
    kind: EffectKind,
    instruction: &ZecEscrowInstruction,
) -> bool {
    match (terms.asset(), kind, instruction) {
        (
            WitnessedLezAssetV2::Native(terms),
            EffectKind::Initialization,
            ZecEscrowInstruction::InitializeNativeWitnessed {
                swap_id,
                terms_hash,
                aggregate_x_only_public_key,
                amount,
                refund_at,
                authenticated_transfer_program,
            },
        ) => {
            swap_id == terms.swap_id().as_bytes()
                && terms_hash == terms.terms_hash().as_bytes()
                && aggregate_x_only_public_key == terms.aggregate_x_only_public_key().as_bytes()
                && *amount == terms.amount().as_u128()
                && *refund_at == terms.refund_at_ms()
                && *authenticated_transfer_program
                    == program_id_from_hex(terms.authenticated_transfer_program_id())
        }
        (
            WitnessedLezAssetV2::CustomToken(terms),
            EffectKind::Initialization,
            ZecEscrowInstruction::InitializeTokenWitnessed {
                swap_id,
                terms_hash,
                aggregate_x_only_public_key,
                amount,
                refund_at,
                ata_program,
            },
        ) => {
            swap_id == terms.swap_id().as_bytes()
                && terms_hash == terms.terms_hash().as_bytes()
                && aggregate_x_only_public_key == terms.aggregate_x_only_public_key().as_bytes()
                && *amount == terms.amount().as_u128()
                && *refund_at == terms.refund_at_ms()
                && *ata_program == programs::ata().id()
        }
        (
            WitnessedLezAssetV2::CustomToken(terms),
            EffectKind::CustodyCreation,
            ZecEscrowInstruction::CreateTokenCustody { swap_id },
        )
        | (
            WitnessedLezAssetV2::CustomToken(terms),
            EffectKind::Funding,
            ZecEscrowInstruction::FundToken { swap_id },
        )
        | (
            WitnessedLezAssetV2::CustomToken(terms),
            EffectKind::Claim,
            ZecEscrowInstruction::ClaimTokenWitnessed { swap_id },
        )
        | (
            WitnessedLezAssetV2::CustomToken(terms),
            EffectKind::Refund,
            ZecEscrowInstruction::RefundToken { swap_id },
        ) => swap_id == terms.swap_id().as_bytes(),
        (
            WitnessedLezAssetV2::Native(terms),
            EffectKind::Funding,
            ZecEscrowInstruction::FundNative { swap_id },
        )
        | (
            WitnessedLezAssetV2::Native(terms),
            EffectKind::Claim,
            ZecEscrowInstruction::ClaimNativeWitnessed { swap_id },
        )
        | (
            WitnessedLezAssetV2::Native(terms),
            EffectKind::Refund,
            ZecEscrowInstruction::RefundNative { swap_id },
        ) => swap_id == terms.swap_id().as_bytes(),
        _ => false,
    }
}

fn instruction_swap(instruction: &ZecEscrowInstruction) -> Option<Hex32> {
    match instruction {
        ZecEscrowInstruction::InitializeNativeWitnessed { swap_id, .. }
        | ZecEscrowInstruction::InitializeTokenWitnessed { swap_id, .. }
        | ZecEscrowInstruction::CreateTokenCustody { swap_id }
        | ZecEscrowInstruction::FundNative { swap_id }
        | ZecEscrowInstruction::FundToken { swap_id }
        | ZecEscrowInstruction::ClaimNativeWitnessed { swap_id }
        | ZecEscrowInstruction::ClaimTokenWitnessed { swap_id }
        | ZecEscrowInstruction::RefundNative { swap_id }
        | ZecEscrowInstruction::RefundToken { swap_id } => Some(Hex32::from_bytes(*swap_id)),
        _ => None,
    }
}

fn instruction_kind(instruction: &ZecEscrowInstruction) -> Option<EffectKind> {
    match instruction {
        ZecEscrowInstruction::InitializeNativeWitnessed { .. }
        | ZecEscrowInstruction::InitializeTokenWitnessed { .. } => Some(EffectKind::Initialization),
        ZecEscrowInstruction::CreateTokenCustody { .. } => Some(EffectKind::CustodyCreation),
        ZecEscrowInstruction::FundNative { .. } | ZecEscrowInstruction::FundToken { .. } => {
            Some(EffectKind::Funding)
        }
        ZecEscrowInstruction::ClaimNativeWitnessed { .. }
        | ZecEscrowInstruction::ClaimTokenWitnessed { .. } => Some(EffectKind::Claim),
        ZecEscrowInstruction::RefundNative { .. } | ZecEscrowInstruction::RefundToken { .. } => {
            Some(EffectKind::Refund)
        }
        _ => None,
    }
}

fn expected_accounts(
    terms: &WitnessedLezAssetTermsV2,
    escrow_program: Hex32,
    kind: EffectKind,
) -> AccountIds {
    let metadata = Hex32::from_bytes(
        compute_metadata_pda(
            &program_id_from_hex(escrow_program),
            swap_id(terms).as_bytes(),
        )
        .into_value(),
    );
    let accounts = match (terms.asset(), kind) {
        (WitnessedLezAssetV2::Native(terms), EffectKind::Initialization) => vec![
            metadata,
            Hex32::from_bytes(
                compute_custody_pda(
                    &program_id_from_hex(escrow_program),
                    terms.swap_id().as_bytes(),
                )
                .into_value(),
            ),
            terms.depositor_account_id(),
            terms.claimant_account_id(),
            terms.aggregate_authority_account_id(),
        ],
        (WitnessedLezAssetV2::Native(terms), EffectKind::Funding | EffectKind::Refund) => vec![
            metadata,
            Hex32::from_bytes(
                compute_custody_pda(
                    &program_id_from_hex(escrow_program),
                    terms.swap_id().as_bytes(),
                )
                .into_value(),
            ),
            terms.depositor_account_id(),
        ],
        (WitnessedLezAssetV2::Native(terms), EffectKind::Claim) => vec![
            metadata,
            Hex32::from_bytes(
                compute_custody_pda(
                    &program_id_from_hex(escrow_program),
                    terms.swap_id().as_bytes(),
                )
                .into_value(),
            ),
            terms.claimant_account_id(),
            terms.aggregate_authority_account_id(),
        ],
        (WitnessedLezAssetV2::CustomToken(terms), EffectKind::Initialization) => vec![
            metadata,
            terms.depositor_owner_account_id(),
            terms.claimant_owner_account_id(),
            terms.token_definition_account_id(),
            terms.aggregate_authority_account_id(),
        ],
        (WitnessedLezAssetV2::CustomToken(terms), EffectKind::CustodyCreation) => vec![
            metadata,
            terms.token_definition_account_id(),
            terms.custody_ata_account_id(),
        ],
        (WitnessedLezAssetV2::CustomToken(terms), EffectKind::Funding) => vec![
            metadata,
            terms.depositor_owner_account_id(),
            terms.depositor_ata_account_id(),
            terms.custody_ata_account_id(),
        ],
        (WitnessedLezAssetV2::CustomToken(terms), EffectKind::Claim) => vec![
            metadata,
            terms.custody_ata_account_id(),
            terms.claimant_owner_account_id(),
            terms.claimant_ata_account_id(),
            terms.aggregate_authority_account_id(),
        ],
        (WitnessedLezAssetV2::CustomToken(terms), EffectKind::Refund) => vec![
            metadata,
            terms.custody_ata_account_id(),
            terms.depositor_ata_account_id(),
        ],
        _ => Vec::new(),
    };
    AccountIds::new(accounts).expect("official asset account sets are bounded")
}

fn expected_signers(terms: &WitnessedLezAssetTermsV2, kind: EffectKind) -> AccountIds {
    let signers = match (terms.asset(), kind) {
        (_, EffectKind::CustodyCreation | EffectKind::Refund) => Vec::new(),
        (WitnessedLezAssetV2::Native(terms), EffectKind::Claim) => {
            vec![terms.aggregate_authority_account_id()]
        }
        (WitnessedLezAssetV2::CustomToken(terms), EffectKind::Claim) => {
            vec![terms.aggregate_authority_account_id()]
        }
        (WitnessedLezAssetV2::Native(terms), _) => vec![terms.depositor_account_id()],
        (WitnessedLezAssetV2::CustomToken(terms), _) => {
            vec![terms.depositor_owner_account_id()]
        }
    };
    AccountIds::new(signers).expect("official asset signer sets are bounded")
}

fn validate_claim_transcript(claim: &PreparedWitnessedClaim) -> Result<(), BridgeRuntimeError> {
    let message =
        nssa::public_transaction::Message::try_from_slice(claim.exact_message_bytes.as_slice())
            .map_err(|_| BridgeRuntimeError::Planner)?;
    if to_vec(&message).map_err(|_| BridgeRuntimeError::Planner)?
        != claim.exact_message_bytes.as_slice()
        || message.hash() != *claim.message_hash.as_bytes()
    {
        return Err(BridgeRuntimeError::Planner);
    }
    Ok(())
}

fn require_present(
    account: HistoricalAccount,
) -> Result<indexer_service_protocol::Account, BridgeRuntimeError> {
    match account {
        HistoricalAccount::Present(account) => Ok(account),
        HistoricalAccount::Absent => Err(BridgeRuntimeError::InvalidObservation),
    }
}

fn containing_block(block: &BlockHeader) -> FinalizedBlockIdentity {
    FinalizedBlockIdentity::new(
        block.block_id,
        Hex32::from_bytes(block.hash.0),
        block.timestamp,
    )
}

fn swap_id(terms: &WitnessedLezAssetTermsV2) -> Hex32 {
    match terms.asset() {
        WitnessedLezAssetV2::Native(terms) => terms.swap_id(),
        WitnessedLezAssetV2::CustomToken(terms) => terms.swap_id(),
    }
}

fn amount(terms: &WitnessedLezAssetTermsV2) -> u128 {
    match terms.asset() {
        WitnessedLezAssetV2::Native(terms) => terms.amount().as_u128(),
        WitnessedLezAssetV2::CustomToken(terms) => terms.amount().as_u128(),
    }
}

const fn escrow_state(status: EscrowStatus) -> Result<EscrowState, BridgeRuntimeError> {
    match status {
        EscrowStatus::Empty => Ok(EscrowState::Empty),
        EscrowStatus::Funded => Ok(EscrowState::Funded),
        EscrowStatus::Claimed => Ok(EscrowState::Claimed),
        EscrowStatus::Refunded => Ok(EscrowState::Refunded),
        EscrowStatus::XmrClaimAuthorized => Err(BridgeRuntimeError::InvalidObservation),
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeMap,
        sync::{
            Arc,
            atomic::{AtomicBool, AtomicUsize, Ordering},
        },
    };

    use async_trait::async_trait;
    use borsh::to_vec;
    use indexer_service_protocol::{
        Account as IndexedAccount, AccountId as IndexedAccountId, BedrockStatus, Block, BlockBody,
        BlockHeader, Data as IndexedData, HashType, ProgramId as IndexedProgramId,
        PublicKey as IndexedPublicKey, PublicMessage as IndexedPublicMessage,
        PublicTransaction as IndexedPublicTransaction, Signature as IndexedSignature, Transaction,
        WitnessSet as IndexedWitnessSet,
    };
    use lez_bridge_protocol::{
        ClassifyFinalizedWitnessedAssetCustodyCreationV2Request,
        ClassifyFinalizedWitnessedAssetFundingV2Request,
        ClassifyFinalizedWitnessedAssetInitializationV2Request, DiscoveryWindow,
        FinalizedWitnessedAssetScanOutcomeV2, Hex32, MessageContext, NativeRefundObservationTarget,
        ObserveWitnessedAssetRefundV2Request, Participant, PrepareWitnessedAssetEscrowV2Request,
        PrepareWitnessedAssetRefundV2Request, RequestId, RunId, RuntimeCompatibility,
        RuntimeDescriptor, WitnessedAssetRefundObservationV2, WitnessedLezAssetTermsV2,
        WitnessedTokenEscrowTermsV2, WitnessedTokenEscrowTermsV2Input,
    };
    use nssa::{AccountId, PrivateKey, PublicKey, PublicTransaction};
    use token_core::{TokenDefinition, TokenHolding};

    use super::*;
    use crate::{
        NativeEscrowPlanner, NativePrepareError, NonceSource, decode_official_public_transaction,
    };

    #[derive(Debug)]
    struct AccountIndexer {
        accounts: BTreeMap<([u8; 32], u64), HistoricalAccount>,
    }

    #[async_trait]
    impl FinalizedIndexerApi for AccountIndexer {
        async fn last_finalized_block_id(&self) -> Result<Option<u64>, BridgeRuntimeError> {
            Err(BridgeRuntimeError::Unavailable)
        }

        async fn block_by_id(&self, _block_id: u64) -> Result<Option<Block>, BridgeRuntimeError> {
            Err(BridgeRuntimeError::Unavailable)
        }

        async fn block_by_hash(
            &self,
            _block_hash: [u8; 32],
        ) -> Result<Option<Block>, BridgeRuntimeError> {
            Err(BridgeRuntimeError::Unavailable)
        }

        async fn account_at_block(
            &self,
            account_id: [u8; 32],
            block_id: u64,
        ) -> Result<HistoricalAccount, BridgeRuntimeError> {
            self.accounts
                .get(&(account_id, block_id))
                .cloned()
                .ok_or(BridgeRuntimeError::Unavailable)
        }
    }

    #[derive(Debug)]
    struct ConcurrentAccountIndexer {
        accounts: BTreeMap<([u8; 32], u64), HistoricalAccount>,
        entered: AtomicUsize,
        all_entered: tokio::sync::Notify,
        release: tokio::sync::Semaphore,
    }

    #[async_trait]
    impl FinalizedIndexerApi for ConcurrentAccountIndexer {
        async fn last_finalized_block_id(&self) -> Result<Option<u64>, BridgeRuntimeError> {
            Err(BridgeRuntimeError::Unavailable)
        }

        async fn block_by_id(&self, _block_id: u64) -> Result<Option<Block>, BridgeRuntimeError> {
            Err(BridgeRuntimeError::Unavailable)
        }

        async fn block_by_hash(
            &self,
            _block_hash: [u8; 32],
        ) -> Result<Option<Block>, BridgeRuntimeError> {
            Err(BridgeRuntimeError::Unavailable)
        }

        async fn account_at_block(
            &self,
            account_id: [u8; 32],
            block_id: u64,
        ) -> Result<HistoricalAccount, BridgeRuntimeError> {
            if self.entered.fetch_add(1, Ordering::SeqCst) + 1 == 3 {
                self.all_entered.notify_one();
            }
            self.release
                .acquire()
                .await
                .map_err(|_| BridgeRuntimeError::Unavailable)?
                .forget();
            self.accounts
                .get(&(account_id, block_id))
                .cloned()
                .ok_or(BridgeRuntimeError::Unavailable)
        }
    }

    #[derive(Debug)]
    struct ScanIndexer {
        tip: u64,
        by_id: BTreeMap<u64, Block>,
        by_hash: BTreeMap<[u8; 32], Block>,
        accounts: BTreeMap<([u8; 32], u64), HistoricalAccount>,
    }

    #[async_trait]
    impl FinalizedIndexerApi for ScanIndexer {
        async fn last_finalized_block_id(&self) -> Result<Option<u64>, BridgeRuntimeError> {
            Ok(Some(self.tip))
        }

        async fn block_by_id(&self, block_id: u64) -> Result<Option<Block>, BridgeRuntimeError> {
            Ok(self.by_id.get(&block_id).cloned())
        }

        async fn block_by_hash(
            &self,
            block_hash: [u8; 32],
        ) -> Result<Option<Block>, BridgeRuntimeError> {
            Ok(self.by_hash.get(&block_hash).cloned())
        }

        async fn account_at_block(
            &self,
            account_id: [u8; 32],
            block_id: u64,
        ) -> Result<HistoricalAccount, BridgeRuntimeError> {
            self.accounts
                .get(&(account_id, block_id))
                .cloned()
                .ok_or(BridgeRuntimeError::Unavailable)
        }
    }

    #[derive(Debug)]
    struct AdvancingTipAfterAccountIndexer {
        base: ScanIndexer,
        next_tip: Block,
        advanced: AtomicBool,
    }

    #[async_trait]
    impl FinalizedIndexerApi for AdvancingTipAfterAccountIndexer {
        async fn last_finalized_block_id(&self) -> Result<Option<u64>, BridgeRuntimeError> {
            Ok(Some(if self.advanced.load(Ordering::SeqCst) {
                self.next_tip.header.block_id
            } else {
                self.base.tip
            }))
        }

        async fn block_by_id(&self, block_id: u64) -> Result<Option<Block>, BridgeRuntimeError> {
            if block_id == self.next_tip.header.block_id {
                Ok(Some(self.next_tip.clone()))
            } else {
                self.base.block_by_id(block_id).await
            }
        }

        async fn block_by_hash(
            &self,
            block_hash: [u8; 32],
        ) -> Result<Option<Block>, BridgeRuntimeError> {
            if block_hash == self.next_tip.header.hash.0 {
                Ok(Some(self.next_tip.clone()))
            } else {
                self.base.block_by_hash(block_hash).await
            }
        }

        async fn account_at_block(
            &self,
            account_id: [u8; 32],
            block_id: u64,
        ) -> Result<HistoricalAccount, BridgeRuntimeError> {
            let account = self.base.account_at_block(account_id, block_id).await?;
            self.advanced.store(true, Ordering::SeqCst);
            Ok(account)
        }
    }

    #[derive(Debug)]
    struct RequestedEndDriftAfterAccountIndexer {
        base: ScanIndexer,
        replacement: Block,
        changed: AtomicBool,
    }

    #[derive(Debug)]
    struct CandidateDriftAfterAccountIndexer {
        base: ScanIndexer,
        candidate_height: u64,
        replacement: Option<Block>,
        changed: AtomicBool,
    }

    #[async_trait]
    impl FinalizedIndexerApi for CandidateDriftAfterAccountIndexer {
        async fn last_finalized_block_id(&self) -> Result<Option<u64>, BridgeRuntimeError> {
            Ok(Some(self.base.tip))
        }

        async fn block_by_id(&self, block_id: u64) -> Result<Option<Block>, BridgeRuntimeError> {
            if self.changed.load(Ordering::SeqCst) && block_id == self.candidate_height {
                Ok(self.replacement.clone())
            } else {
                self.base.block_by_id(block_id).await
            }
        }

        async fn block_by_hash(
            &self,
            block_hash: [u8; 32],
        ) -> Result<Option<Block>, BridgeRuntimeError> {
            if let Some(replacement) = &self.replacement
                && block_hash == replacement.header.hash.0
            {
                Ok(Some(replacement.clone()))
            } else {
                self.base.block_by_hash(block_hash).await
            }
        }

        async fn account_at_block(
            &self,
            account_id: [u8; 32],
            block_id: u64,
        ) -> Result<HistoricalAccount, BridgeRuntimeError> {
            let account = self.base.account_at_block(account_id, block_id).await?;
            self.changed.store(true, Ordering::SeqCst);
            Ok(account)
        }
    }

    #[async_trait]
    impl FinalizedIndexerApi for RequestedEndDriftAfterAccountIndexer {
        async fn last_finalized_block_id(&self) -> Result<Option<u64>, BridgeRuntimeError> {
            Ok(Some(self.base.tip))
        }

        async fn block_by_id(&self, block_id: u64) -> Result<Option<Block>, BridgeRuntimeError> {
            if self.changed.load(Ordering::SeqCst) && block_id == self.replacement.header.block_id {
                Ok(Some(self.replacement.clone()))
            } else {
                self.base.block_by_id(block_id).await
            }
        }

        async fn block_by_hash(
            &self,
            block_hash: [u8; 32],
        ) -> Result<Option<Block>, BridgeRuntimeError> {
            if block_hash == self.replacement.header.hash.0 {
                Ok(Some(self.replacement.clone()))
            } else {
                self.base.block_by_hash(block_hash).await
            }
        }

        async fn account_at_block(
            &self,
            account_id: [u8; 32],
            block_id: u64,
        ) -> Result<HistoricalAccount, BridgeRuntimeError> {
            let account = self.base.account_at_block(account_id, block_id).await?;
            self.changed.store(true, Ordering::SeqCst);
            Ok(account)
        }
    }

    #[derive(Debug)]
    struct ForkAfterScanIndexer {
        old: ScanIndexer,
        new_tip: Block,
        new_accounts: BTreeMap<([u8; 32], u64), HistoricalAccount>,
        tip_reads: AtomicUsize,
        changed: AtomicBool,
    }

    #[async_trait]
    impl FinalizedIndexerApi for ForkAfterScanIndexer {
        async fn last_finalized_block_id(&self) -> Result<Option<u64>, BridgeRuntimeError> {
            let read = self.tip_reads.fetch_add(1, Ordering::SeqCst);
            if read >= 1 {
                self.changed.store(true, Ordering::SeqCst);
            }
            Ok(Some(self.old.tip))
        }

        async fn block_by_id(&self, block_id: u64) -> Result<Option<Block>, BridgeRuntimeError> {
            if self.changed.load(Ordering::SeqCst) && block_id == self.new_tip.header.block_id {
                Ok(Some(self.new_tip.clone()))
            } else {
                self.old.block_by_id(block_id).await
            }
        }

        async fn block_by_hash(
            &self,
            block_hash: [u8; 32],
        ) -> Result<Option<Block>, BridgeRuntimeError> {
            if block_hash == self.new_tip.header.hash.0 {
                Ok(Some(self.new_tip.clone()))
            } else {
                self.old.block_by_hash(block_hash).await
            }
        }

        async fn account_at_block(
            &self,
            account_id: [u8; 32],
            block_id: u64,
        ) -> Result<HistoricalAccount, BridgeRuntimeError> {
            if self.changed.load(Ordering::SeqCst) {
                self.new_accounts
                    .get(&(account_id, block_id))
                    .cloned()
                    .ok_or(BridgeRuntimeError::Unavailable)
            } else {
                self.old.account_at_block(account_id, block_id).await
            }
        }
    }

    #[derive(Debug)]
    struct FixedNonce;

    #[async_trait]
    impl NonceSource for FixedNonce {
        async fn account_nonce(&self, _account_id: AccountId) -> Result<u128, NativePrepareError> {
            Ok(41)
        }
    }

    fn indexed_public(public: &PublicTransaction) -> IndexedPublicTransaction {
        IndexedPublicTransaction {
            hash: HashType(public.hash()),
            message: IndexedPublicMessage {
                program_id: IndexedProgramId(public.message().program_id),
                account_ids: public
                    .message()
                    .account_ids
                    .iter()
                    .map(|account| IndexedAccountId {
                        value: account.into_value(),
                    })
                    .collect(),
                nonces: public
                    .message()
                    .nonces
                    .iter()
                    .map(|nonce| u128::from(*nonce))
                    .collect(),
                instruction_data: public.message().instruction_data.clone(),
            },
            witness_set: IndexedWitnessSet {
                signatures_and_public_keys: public
                    .witness_set()
                    .signatures_and_public_keys()
                    .iter()
                    .map(|(signature, key)| {
                        (
                            IndexedSignature(signature.value),
                            IndexedPublicKey(*key.value()),
                        )
                    })
                    .collect(),
                proof: None,
            },
        }
    }

    fn finalized_block(block_id: u64, transactions: Vec<Transaction>) -> Block {
        let byte = u8::try_from(block_id).unwrap();
        let previous = u8::try_from(block_id.saturating_sub(1)).unwrap();
        Block {
            header: BlockHeader {
                block_id,
                prev_block_hash: HashType([previous; 32]),
                hash: HashType([byte; 32]),
                timestamp: 1_850_000_000_000 + block_id,
                signature: IndexedSignature([byte; 64]),
            },
            body: BlockBody { transactions },
            bedrock_status: BedrockStatus::Finalized,
        }
    }

    fn at_height(
        accounts: BTreeMap<([u8; 32], u64), HistoricalAccount>,
        height: u64,
    ) -> impl Iterator<Item = (([u8; 32], u64), HistoricalAccount)> {
        accounts
            .into_iter()
            .map(move |((account, _), state)| ((account, height), state))
    }

    struct TokenFixture {
        runtime: RuntimeDescriptor,
        terms: WitnessedLezAssetTermsV2,
        metadata_id: [u8; 32],
        custody_id: [u8; 32],
        definition_id: [u8; 32],
        accounts: BTreeMap<([u8; 32], u64), HistoricalAccount>,
    }

    #[allow(
        clippy::too_many_lines,
        reason = "the official token metadata, definition, holding, and ATA identities form one exact fixture"
    )]
    fn token_fixture(state: EscrowStatus, custody: HistoricalAccount) -> TokenFixture {
        let depositor_key = PrivateKey::try_new([71; 32]).unwrap();
        let depositor = AccountId::from(&PublicKey::new_from_private_key(&depositor_key));
        let claimant_key = PrivateKey::try_new([72; 32]).unwrap();
        let claimant = AccountId::from(&PublicKey::new_from_private_key(&claimant_key));
        let authority_key = PrivateKey::try_new([73; 32]).unwrap();
        let authority_public = PublicKey::new_from_private_key(&authority_key);
        let authority = AccountId::from(&authority_public);
        let definition = AccountId::new([74; 32]);
        let escrow_program = [0x1020_3040; 8];
        let escrow_program_hex = program_id_to_hex(escrow_program);
        let swap = Hex32::from_bytes([75; 32]);
        let metadata = compute_metadata_pda(&escrow_program, swap.as_bytes());
        let depositor_ata = official_ata(depositor, definition);
        let claimant_ata = official_ata(claimant, definition);
        let custody_ata = official_ata(metadata, definition);
        let token_terms = WitnessedTokenEscrowTermsV2::new(WitnessedTokenEscrowTermsV2Input {
            swap_id: swap,
            terms_hash: Hex32::from_bytes([76; 32]),
            depositor: Participant::Maker,
            depositor_owner_account_id: Hex32::from_bytes(depositor.into_value()),
            depositor_ata_account_id: depositor_ata,
            claimant: Participant::Taker,
            claimant_owner_account_id: Hex32::from_bytes(claimant.into_value()),
            claimant_ata_account_id: claimant_ata,
            custody_ata_account_id: custody_ata,
            token_program_id: program_id_to_hex(programs::token().id()),
            ata_program_id: program_id_to_hex(programs::ata().id()),
            token_definition_account_id: Hex32::from_bytes(definition.into_value()),
            aggregate_authority_account_id: Hex32::from_bytes(authority.into_value()),
            aggregate_x_only_public_key: Hex32::from_bytes(*authority_public.value()),
            amount: 75,
            refund_at_ms: 1_850_000_000_123,
        })
        .unwrap();
        let terms = WitnessedLezAssetTermsV2::custom_token(token_terms.clone());
        let metadata_state = EscrowMetadata {
            version: 2,
            swap_id: *token_terms.swap_id().as_bytes(),
            terms_hash: *token_terms.terms_hash().as_bytes(),
            claim_authority: ClaimAuthority::AggregateWitness {
                x_only_public_key: *token_terms.aggregate_x_only_public_key().as_bytes(),
                account_id: authority,
            },
            depositor,
            depositor_asset: AccountId::new(*depositor_ata.as_bytes()),
            claimant,
            claimant_asset: AccountId::new(*claimant_ata.as_bytes()),
            custody: AccountId::new(*custody_ata.as_bytes()),
            asset_program: programs::token().id(),
            custody_program: programs::ata().id(),
            asset_definition: definition.into_value(),
            amount: 75,
            refund_at: 1_850_000_000_123,
            status: state,
        };
        let metadata_id = metadata.into_value();
        let custody_id = *custody_ata.as_bytes();
        let definition_id = definition.into_value();
        let mut accounts = BTreeMap::from([
            (
                (metadata_id, 10),
                HistoricalAccount::Present(IndexedAccount {
                    program_owner: IndexedProgramId(escrow_program),
                    balance: 0,
                    data: IndexedData(to_vec(&metadata_state).unwrap()),
                    nonce: 0,
                }),
            ),
            (
                (definition_id, 10),
                HistoricalAccount::Present(IndexedAccount {
                    program_owner: IndexedProgramId(programs::token().id()),
                    balance: 0,
                    data: IndexedData(
                        to_vec(&TokenDefinition::Fungible {
                            name: "F7 token".to_owned(),
                            total_supply: 1_000,
                            metadata_id: None,
                        })
                        .unwrap(),
                    ),
                    nonce: 0,
                }),
            ),
        ]);
        accounts.insert((custody_id, 10), custody);
        TokenFixture {
            runtime: RuntimeDescriptor::new(
                Participant::Maker,
                RuntimeCompatibility::LeeV0_2_0,
                Hex32::from_bytes([1; 32]),
                Hex32::from_bytes([2; 32]),
                Hex32::from_bytes([3; 32]),
                escrow_program_hex,
                Hex32::from_bytes(depositor.into_value()),
            ),
            terms,
            metadata_id,
            custody_id,
            definition_id,
            accounts,
        }
    }

    fn token_terms_with_hash(
        terms: &WitnessedLezAssetTermsV2,
        terms_hash: Hex32,
    ) -> WitnessedLezAssetTermsV2 {
        let current = terms.asset().custom_token().expect("token fixture terms");
        WitnessedLezAssetTermsV2::custom_token(
            WitnessedTokenEscrowTermsV2::new(WitnessedTokenEscrowTermsV2Input {
                swap_id: current.swap_id(),
                terms_hash,
                depositor: current.depositor(),
                depositor_owner_account_id: current.depositor_owner_account_id(),
                depositor_ata_account_id: current.depositor_ata_account_id(),
                claimant: current.claimant(),
                claimant_owner_account_id: current.claimant_owner_account_id(),
                claimant_ata_account_id: current.claimant_ata_account_id(),
                custody_ata_account_id: current.custody_ata_account_id(),
                token_program_id: current.token_program_id(),
                ata_program_id: current.ata_program_id(),
                token_definition_account_id: current.token_definition_account_id(),
                aggregate_authority_account_id: current.aggregate_authority_account_id(),
                aggregate_x_only_public_key: current.aggregate_x_only_public_key(),
                amount: current.amount().as_u128(),
                refund_at_ms: current.refund_at_ms(),
            })
            .expect("distinct nonzero token terms hash"),
        )
    }

    fn holding(definition_id: [u8; 32], balance: u128) -> HistoricalAccount {
        HistoricalAccount::Present(IndexedAccount {
            program_owner: IndexedProgramId(programs::token().id()),
            balance: 0,
            data: IndexedData(
                to_vec(&TokenHolding::Fungible {
                    definition_id: AccountId::new(definition_id),
                    balance,
                })
                .unwrap(),
            ),
            nonce: 0,
        })
    }

    fn empty_scan_observer(fixture: &TokenFixture) -> FinalizedAssetObserver {
        let block = finalized_block(10, Vec::new());
        FinalizedAssetObserver::new(
            fixture.runtime.clone(),
            Arc::new(ScanIndexer {
                tip: 10,
                by_id: BTreeMap::from([(10, block.clone())]),
                by_hash: BTreeMap::from([(block.header.hash.0, block)]),
                accounts: fixture.accounts.clone(),
            }),
        )
    }

    #[tokio::test]
    async fn missing_asset_effect_requires_exact_stable_predecessor_state() {
        let window = DiscoveryWindow::new(10, 1).unwrap();

        let mut initialization = token_fixture(EscrowStatus::Empty, HistoricalAccount::Absent);
        initialization
            .accounts
            .insert((initialization.metadata_id, 10), HistoricalAccount::Absent);
        let observer = empty_scan_observer(&initialization);
        let stable = read_fixed_finalized_window(observer.indexer.as_ref(), window)
            .await
            .unwrap();
        assert!(matches!(
            observer
                .classify_missing_effect(
                    &initialization.terms,
                    EffectKind::Initialization,
                    &stable,
                )
                .await
                .unwrap(),
            MissingEffectClassification::Absent
        ));

        let custody = token_fixture(EscrowStatus::Empty, HistoricalAccount::Absent);
        let observer = empty_scan_observer(&custody);
        let stable = read_fixed_finalized_window(observer.indexer.as_ref(), window)
            .await
            .unwrap();
        assert!(matches!(
            observer
                .classify_missing_effect(&custody.terms, EffectKind::CustodyCreation, &stable,)
                .await
                .unwrap(),
            MissingEffectClassification::Absent
        ));

        let funding_seed = token_fixture(EscrowStatus::Empty, HistoricalAccount::Absent);
        let mut funding_accounts = funding_seed.accounts.clone();
        funding_accounts.insert(
            (funding_seed.custody_id, 10),
            holding(funding_seed.definition_id, 0),
        );
        let funding = TokenFixture {
            accounts: funding_accounts,
            ..funding_seed
        };
        let observer = empty_scan_observer(&funding);
        let stable = read_fixed_finalized_window(observer.indexer.as_ref(), window)
            .await
            .unwrap();
        assert!(matches!(
            observer
                .classify_missing_effect(&funding.terms, EffectKind::Funding, &stable)
                .await
                .unwrap(),
            MissingEffectClassification::Absent
        ));

        let claim_seed = token_fixture(EscrowStatus::Funded, HistoricalAccount::Absent);
        let mut claim_accounts = claim_seed.accounts.clone();
        claim_accounts.insert(
            (claim_seed.custody_id, 10),
            holding(claim_seed.definition_id, 75),
        );
        let claim = TokenFixture {
            accounts: claim_accounts,
            ..claim_seed
        };
        let observer = empty_scan_observer(&claim);
        let stable = read_fixed_finalized_window(observer.indexer.as_ref(), window)
            .await
            .unwrap();
        assert!(matches!(
            observer
                .classify_missing_effect(&claim.terms, EffectKind::Claim, &stable)
                .await
                .unwrap(),
            MissingEffectClassification::Absent
        ));

        let already_initialized = token_fixture(EscrowStatus::Empty, HistoricalAccount::Absent);
        let observer = empty_scan_observer(&already_initialized);
        let stable = read_fixed_finalized_window(observer.indexer.as_ref(), window)
            .await
            .unwrap();
        assert!(matches!(
            observer
                .classify_missing_effect(
                    &already_initialized.terms,
                    EffectKind::Initialization,
                    &stable,
                )
                .await
                .unwrap(),
            MissingEffectClassification::Uncertain
        ));
    }

    fn absent_funding_request(
        fixture: &TokenFixture,
    ) -> ClassifyFinalizedWitnessedAssetFundingV2Request {
        ClassifyFinalizedWitnessedAssetFundingV2Request::new(
            MessageContext::new(
                RunId::new("asset-snapshot-boundary-run-0001").unwrap(),
                RequestId::new("asset-snapshot-boundary-funding-0001").unwrap(),
                Participant::Maker,
            ),
            fixture.runtime.clone(),
            fixture.terms.clone(),
            PreparedTransaction::new(
                lez_bridge_protocol::TransactionId::from_bytes([201; 32]),
                lez_bridge_protocol::ExactTransactionBytes::new(vec![1]).unwrap(),
            ),
            DiscoveryWindow::new(10, 1).unwrap(),
        )
    }

    #[tokio::test]
    async fn missing_effect_is_anchored_to_requested_end_while_live_tip_advances() {
        let seed = token_fixture(EscrowStatus::Empty, HistoricalAccount::Absent);
        let mut accounts = seed.accounts.clone();
        accounts.insert((seed.custody_id, 10), holding(seed.definition_id, 0));
        let fixture = TokenFixture { accounts, ..seed };
        let requested_end = finalized_block(10, Vec::new());
        let next_tip = finalized_block(11, Vec::new());
        let indexer = Arc::new(AdvancingTipAfterAccountIndexer {
            base: ScanIndexer {
                tip: 10,
                by_id: BTreeMap::from([(10, requested_end.clone())]),
                by_hash: BTreeMap::from([(requested_end.header.hash.0, requested_end.clone())]),
                accounts: fixture.accounts.clone(),
            },
            next_tip,
            advanced: AtomicBool::new(false),
        });
        let observer = FinalizedAssetObserver::new(fixture.runtime.clone(), indexer.clone());
        let result = observer
            .classify_funding(&absent_funding_request(&fixture))
            .await
            .unwrap();

        let (finalized_clock, scanned_window) = match result.outcome {
            FinalizedWitnessedAssetScanOutcomeV2::Absent {
                finalized_clock,
                scanned_window,
            } => (finalized_clock, scanned_window),
            other => panic!("fixed finalized snapshot must prove bounded absence: {other:?}"),
        };
        assert_eq!(scanned_window, DiscoveryWindow::new(10, 1).unwrap());
        assert_eq!(finalized_clock.height, 10);
        assert_eq!(finalized_clock.block_hash.as_bytes(), &[10; 32]);
        assert!(indexer.advanced.load(Ordering::SeqCst));
        assert_eq!(indexer.last_finalized_block_id().await.unwrap(), Some(11));
    }

    #[tokio::test]
    async fn missing_effect_fails_closed_when_requested_end_identity_drifts() {
        let seed = token_fixture(EscrowStatus::Empty, HistoricalAccount::Absent);
        let mut accounts = seed.accounts.clone();
        accounts.insert((seed.custody_id, 10), holding(seed.definition_id, 0));
        let fixture = TokenFixture { accounts, ..seed };
        let requested_end = finalized_block(10, Vec::new());
        let mut replacement = requested_end.clone();
        replacement.header.hash = HashType([202; 32]);
        replacement.header.signature = IndexedSignature([202; 64]);
        let indexer = Arc::new(RequestedEndDriftAfterAccountIndexer {
            base: ScanIndexer {
                tip: 10,
                by_id: BTreeMap::from([(10, requested_end.clone())]),
                by_hash: BTreeMap::from([(requested_end.header.hash.0, requested_end)]),
                accounts: fixture.accounts.clone(),
            },
            replacement,
            changed: AtomicBool::new(false),
        });
        let observer = FinalizedAssetObserver::new(fixture.runtime.clone(), indexer);
        let result = observer
            .classify_funding(&absent_funding_request(&fixture))
            .await
            .unwrap();

        assert!(matches!(
            result.outcome,
            FinalizedWitnessedAssetScanOutcomeV2::Unavailable {
                reason: FinalizedWitnessedAssetUnavailableReasonV2::MovingTip
            }
        ));
    }

    #[tokio::test]
    async fn token_initialization_preserves_authoritative_absence_provenance() {
        let absent = token_fixture(EscrowStatus::Empty, HistoricalAccount::Absent);
        let observer = FinalizedAssetObserver::new(
            absent.runtime.clone(),
            Arc::new(AccountIndexer {
                accounts: absent.accounts.clone(),
            }),
        );
        let (_, custody) = observer
            .read_initialization_state(&absent.terms, 10)
            .await
            .unwrap();
        assert!(matches!(
            custody,
            WitnessedAssetInitializationCustodyFactsV2::CustomTokenAtaAbsent {
                expected_account_id
            } if expected_account_id.as_bytes() == &absent.custody_id
        ));

        let mut unavailable_accounts = absent.accounts.clone();
        unavailable_accounts.remove(&(absent.custody_id, 10));
        let unavailable = FinalizedAssetObserver::new(
            absent.runtime.clone(),
            Arc::new(AccountIndexer {
                accounts: unavailable_accounts,
            }),
        );
        assert!(matches!(
            unavailable
                .read_initialization_state(&absent.terms, 10)
                .await,
            Err(BridgeRuntimeError::Unavailable)
        ));

        let present = token_fixture(EscrowStatus::Empty, holding(absent.definition_id, 0));
        let observer = FinalizedAssetObserver::new(
            present.runtime.clone(),
            Arc::new(AccountIndexer {
                accounts: present.accounts.clone(),
            }),
        );
        assert!(matches!(
            observer.read_initialization_state(&present.terms, 10).await,
            Err(BridgeRuntimeError::InvalidObservation)
        ));
    }

    #[tokio::test]
    async fn token_funding_requires_exact_fungible_definition_and_holding() {
        let funded_seed = token_fixture(EscrowStatus::Funded, HistoricalAccount::Absent);
        let mut funded_accounts = funded_seed.accounts.clone();
        funded_accounts.insert(
            (funded_seed.custody_id, 10),
            holding(funded_seed.definition_id, 75),
        );
        let observer = FinalizedAssetObserver::new(
            funded_seed.runtime.clone(),
            Arc::new(AccountIndexer {
                accounts: funded_accounts.clone(),
            }),
        );
        let (_, custody) = observer
            .read_asset_state(&funded_seed.terms, 10, EscrowState::Funded, 75)
            .await
            .unwrap();
        assert!(matches!(
            custody,
            WitnessedAssetCustodyFactsV2::CustomToken(facts)
                if facts.balance.as_u128() == 75
                    && facts.token_definition_account_id.as_bytes()
                        == &funded_seed.definition_id
        ));

        let mut wrong_holding = funded_accounts.clone();
        wrong_holding.insert((funded_seed.custody_id, 10), holding([99; 32], 75));
        let observer = FinalizedAssetObserver::new(
            funded_seed.runtime.clone(),
            Arc::new(AccountIndexer {
                accounts: wrong_holding,
            }),
        );
        assert!(matches!(
            observer
                .read_asset_state(&funded_seed.terms, 10, EscrowState::Funded, 75)
                .await,
            Err(BridgeRuntimeError::InvalidObservation)
        ));

        let mut nft_definition = funded_accounts;
        nft_definition.insert(
            (funded_seed.definition_id, 10),
            HistoricalAccount::Present(IndexedAccount {
                program_owner: IndexedProgramId(programs::token().id()),
                balance: 0,
                data: IndexedData(
                    to_vec(&TokenDefinition::NonFungible {
                        name: "substitution".to_owned(),
                        printable_supply: 1,
                        metadata_id: AccountId::new([98; 32]),
                    })
                    .unwrap(),
                ),
                nonce: 0,
            }),
        );
        let observer = FinalizedAssetObserver::new(
            funded_seed.runtime,
            Arc::new(AccountIndexer {
                accounts: nft_definition,
            }),
        );
        assert!(matches!(
            observer
                .read_asset_state(&funded_seed.terms, 10, EscrowState::Funded, 75)
                .await,
            Err(BridgeRuntimeError::InvalidObservation)
        ));
        assert_ne!(funded_seed.metadata_id, funded_seed.custody_id);
    }

    #[tokio::test]
    async fn custom_token_state_reads_metadata_definition_and_custody_concurrently() {
        let fixture = token_fixture(EscrowStatus::Funded, HistoricalAccount::Absent);
        let mut accounts = fixture.accounts.clone();
        accounts.insert((fixture.custody_id, 10), holding(fixture.definition_id, 75));
        let indexer = Arc::new(ConcurrentAccountIndexer {
            accounts,
            entered: AtomicUsize::new(0),
            all_entered: tokio::sync::Notify::new(),
            release: tokio::sync::Semaphore::new(0),
        });
        let observer = FinalizedAssetObserver::new(fixture.runtime, indexer.clone());
        let read = observer.read_asset_state(&fixture.terms, 10, EscrowState::Funded, 75);
        let assert_concurrent = async {
            indexer.all_entered.notified().await;
            assert_eq!(indexer.entered.load(Ordering::SeqCst), 3);
            indexer.release.add_permits(3);
        };
        let (result, ()) = tokio::time::timeout(std::time::Duration::from_secs(1), async {
            tokio::join!(read, assert_concurrent)
        })
        .await
        .expect("three custom-token historical reads must enter concurrently");

        assert!(matches!(
            result,
            Ok((_, WitnessedAssetCustodyFactsV2::CustomToken(facts)))
                if facts.balance.as_u128() == 75
        ));
    }

    #[tokio::test]
    async fn asset_scan_ignores_unrequested_finalized_descendants() {
        let initialization = token_fixture(EscrowStatus::Empty, HistoricalAccount::Absent);
        let planner = NativeEscrowPlanner::new(
            Participant::Maker,
            PrivateKey::try_new([71; 32]).unwrap(),
            [0x1020_3040; 8],
            [0x5060_7080; 8],
            initialization.runtime.clone(),
            Arc::new(FixedNonce),
        )
        .unwrap();
        let prepared = planner
            .prepare_witnessed_asset_escrow_v2(&PrepareWitnessedAssetEscrowV2Request::new(
                MessageContext::new(
                    RunId::new("asset-fixed-window-run-0001").unwrap(),
                    RequestId::new("asset-fixed-window-prepare-0001").unwrap(),
                    Participant::Maker,
                ),
                initialization.runtime.clone(),
                initialization.terms.clone(),
            ))
            .await
            .unwrap();
        let public = decode_official_public_transaction(
            prepared.effects[0].transaction.exact_bytes.as_slice(),
        )
        .unwrap();
        let requested = finalized_block(10, vec![Transaction::Public(indexed_public(&public))]);
        let live_tip = finalized_block(13, Vec::new());
        let mut accounts = BTreeMap::new();
        accounts.extend(at_height(initialization.accounts.clone(), 10));
        let observer = FinalizedAssetObserver::new(
            initialization.runtime.clone(),
            Arc::new(ScanIndexer {
                tip: 13,
                by_id: BTreeMap::from([(10, requested.clone()), (13, live_tip.clone())]),
                by_hash: BTreeMap::from([
                    (requested.header.hash.0, requested),
                    (live_tip.header.hash.0, live_tip),
                ]),
                accounts,
            }),
        );

        let result = observer
            .classify_initialization(
                &ClassifyFinalizedWitnessedAssetInitializationV2Request::new(
                    MessageContext::new(
                        RunId::new("asset-fixed-window-run-0001").unwrap(),
                        RequestId::new("asset-fixed-window-observe-0001").unwrap(),
                        Participant::Maker,
                    ),
                    initialization.runtime.clone(),
                    initialization.terms.clone(),
                    prepared.effects[0].transaction.clone(),
                    DiscoveryWindow::new(10, 1).unwrap(),
                ),
            )
            .await
            .unwrap();

        assert!(matches!(
            result.outcome,
            FinalizedWitnessedAssetScanOutcomeV2::Found { .. }
        ));
    }

    #[tokio::test]
    #[allow(
        clippy::too_many_lines,
        reason = "one official planner-to-indexer journey keeps all three ordered effects and the post-state reorg in view"
    )]
    async fn official_token_plan_is_found_across_stable_finalized_blocks() {
        let initialization = token_fixture(EscrowStatus::Empty, HistoricalAccount::Absent);
        let custody = token_fixture(
            EscrowStatus::Empty,
            holding(initialization.definition_id, 0),
        );
        let funding = token_fixture(
            EscrowStatus::Funded,
            holding(initialization.definition_id, 75),
        );
        let planner = NativeEscrowPlanner::new(
            Participant::Maker,
            PrivateKey::try_new([71; 32]).unwrap(),
            [0x1020_3040; 8],
            [0x5060_7080; 8],
            initialization.runtime.clone(),
            Arc::new(FixedNonce),
        )
        .unwrap();
        let prepared = planner
            .prepare_witnessed_asset_escrow_v2(&PrepareWitnessedAssetEscrowV2Request::new(
                MessageContext::new(
                    RunId::new("asset-finalized-scan-run-0001").unwrap(),
                    RequestId::new("asset-finalized-scan-prepare-0001").unwrap(),
                    Participant::Maker,
                ),
                initialization.runtime.clone(),
                initialization.terms.clone(),
            ))
            .await
            .unwrap();
        assert_eq!(prepared.effects.len(), 3);

        let effect_blocks = prepared
            .effects
            .iter()
            .enumerate()
            .map(|(index, effect)| {
                let public =
                    decode_official_public_transaction(effect.transaction.exact_bytes.as_slice())
                        .unwrap();
                finalized_block(
                    10 + u64::try_from(index).unwrap(),
                    vec![Transaction::Public(indexed_public(&public))],
                )
            })
            .chain(std::iter::once(finalized_block(13, Vec::new())))
            .collect::<Vec<_>>();
        let mut accounts = BTreeMap::new();
        accounts.extend(at_height(initialization.accounts, 10));
        accounts.extend(at_height(custody.accounts, 11));
        accounts.extend(at_height(funding.accounts, 12));
        let by_id = effect_blocks
            .iter()
            .map(|block| (block.header.block_id, block.clone()))
            .collect::<BTreeMap<_, _>>();
        let by_hash = effect_blocks
            .iter()
            .map(|block| (block.header.hash.0, block.clone()))
            .collect::<BTreeMap<_, _>>();
        let indexer = Arc::new(ScanIndexer {
            tip: 13,
            by_id: by_id.clone(),
            by_hash: by_hash.clone(),
            accounts: accounts.clone(),
        });
        let observer = FinalizedAssetObserver::new(initialization.runtime.clone(), indexer);
        let window = DiscoveryWindow::new(10, 4).unwrap();
        let context = |request_id| {
            MessageContext::new(
                RunId::new("asset-finalized-scan-run-0001").unwrap(),
                RequestId::new(request_id).unwrap(),
                Participant::Maker,
            )
        };

        let initialization_result = observer
            .classify_initialization(
                &ClassifyFinalizedWitnessedAssetInitializationV2Request::new(
                    context("asset-finalized-scan-init-0001"),
                    initialization.runtime.clone(),
                    initialization.terms.clone(),
                    prepared.effects[0].transaction.clone(),
                    window,
                ),
            )
            .await
            .unwrap();
        assert!(matches!(
            initialization_result.outcome,
            FinalizedWitnessedAssetScanOutcomeV2::Found { .. }
        ));

        let custody_result = observer
            .classify_custody_creation(
                &ClassifyFinalizedWitnessedAssetCustodyCreationV2Request::new(
                    context("asset-finalized-scan-custody-0001"),
                    initialization.runtime.clone(),
                    initialization.terms.clone(),
                    prepared.effects[1].transaction.clone(),
                    window,
                )
                .unwrap(),
            )
            .await
            .unwrap();
        assert!(matches!(
            custody_result.outcome,
            FinalizedWitnessedAssetScanOutcomeV2::Found { .. }
        ));

        let funding_result = observer
            .classify_funding(&ClassifyFinalizedWitnessedAssetFundingV2Request::new(
                context("asset-finalized-scan-funding-0001"),
                initialization.runtime.clone(),
                initialization.terms.clone(),
                prepared.effects[2].transaction.clone(),
                window,
            ))
            .await
            .unwrap();
        assert!(matches!(
            funding_result.outcome,
            FinalizedWitnessedAssetScanOutcomeV2::Found { .. }
        ));

        let discovered_funding = observer
            .classify_funding(
                &ClassifyFinalizedWitnessedAssetFundingV2Request::discover_by_terms(
                    context("asset-finalized-scan-funding-discovery-0001"),
                    initialization.runtime.clone(),
                    initialization.terms.clone(),
                    window,
                ),
            )
            .await
            .expect("valid earlier steps for the same swap are not funding conflicts");
        assert!(matches!(
            discovered_funding.outcome,
            FinalizedWitnessedAssetScanOutcomeV2::Found { .. }
        ));

        let conflicting_terms =
            token_terms_with_hash(&initialization.terms, Hex32::from_bytes([204; 32]));
        assert!(matches!(
            observer
                .classify_funding(
                    &ClassifyFinalizedWitnessedAssetFundingV2Request::discover_by_terms(
                        context("asset-finalized-scan-funding-conflict-0001"),
                        initialization.runtime.clone(),
                        conflicting_terms,
                        window,
                    ),
                )
                .await,
            Err(BridgeRuntimeError::ConflictingDiscovery)
        ));

        let advancing_indexer = Arc::new(AdvancingTipAfterAccountIndexer {
            base: ScanIndexer {
                tip: 13,
                by_id: by_id.clone(),
                by_hash: by_hash.clone(),
                accounts: accounts.clone(),
            },
            next_tip: finalized_block(14, Vec::new()),
            advanced: AtomicBool::new(false),
        });
        let moving_observer =
            FinalizedAssetObserver::new(initialization.runtime.clone(), advancing_indexer.clone());
        let moved = moving_observer
            .classify_initialization(
                &ClassifyFinalizedWitnessedAssetInitializationV2Request::new(
                    context("asset-finalized-scan-moving-tip-0001"),
                    initialization.runtime.clone(),
                    initialization.terms.clone(),
                    prepared.effects[0].transaction.clone(),
                    window,
                ),
            )
            .await
            .unwrap();
        assert!(matches!(
            moved.outcome,
            FinalizedWitnessedAssetScanOutcomeV2::Found { .. }
        ));
        assert!(advancing_indexer.advanced.load(Ordering::SeqCst));
        assert_eq!(
            advancing_indexer.last_finalized_block_id().await.unwrap(),
            Some(14)
        );

        let mut replacement = by_id.get(&10).unwrap().clone();
        replacement.header.hash = HashType([203; 32]);
        replacement.header.signature = IndexedSignature([203; 64]);
        let changed_candidate = FinalizedAssetObserver::new(
            initialization.runtime.clone(),
            Arc::new(CandidateDriftAfterAccountIndexer {
                base: ScanIndexer {
                    tip: 13,
                    by_id: by_id.clone(),
                    by_hash: by_hash.clone(),
                    accounts: accounts.clone(),
                },
                candidate_height: 10,
                replacement: Some(replacement),
                changed: AtomicBool::new(false),
            }),
        );
        let changed = changed_candidate
            .classify_initialization(
                &ClassifyFinalizedWitnessedAssetInitializationV2Request::new(
                    context("asset-finalized-scan-candidate-drift-0001"),
                    initialization.runtime.clone(),
                    initialization.terms.clone(),
                    prepared.effects[0].transaction.clone(),
                    window,
                ),
            )
            .await
            .unwrap();
        assert!(matches!(
            changed.outcome,
            FinalizedWitnessedAssetScanOutcomeV2::Unavailable {
                reason: FinalizedWitnessedAssetUnavailableReasonV2::MovingTip
            }
        ));

        let missing_candidate = FinalizedAssetObserver::new(
            initialization.runtime.clone(),
            Arc::new(CandidateDriftAfterAccountIndexer {
                base: ScanIndexer {
                    tip: 13,
                    by_id,
                    by_hash,
                    accounts,
                },
                candidate_height: 10,
                replacement: None,
                changed: AtomicBool::new(false),
            }),
        );
        let missing = missing_candidate
            .classify_initialization(
                &ClassifyFinalizedWitnessedAssetInitializationV2Request::new(
                    context("asset-finalized-scan-candidate-missing-0001"),
                    initialization.runtime,
                    initialization.terms,
                    prepared.effects[0].transaction.clone(),
                    window,
                ),
            )
            .await
            .unwrap();
        assert!(matches!(
            missing.outcome,
            FinalizedWitnessedAssetScanOutcomeV2::Unavailable {
                reason: FinalizedWitnessedAssetUnavailableReasonV2::HistoryUnavailable
            }
        ));
    }

    #[tokio::test]
    async fn refund_fork_between_scan_and_state_is_never_reported_found() {
        let definition = token_fixture(EscrowStatus::Refunded, HistoricalAccount::Absent);
        let fixture = token_fixture(EscrowStatus::Refunded, holding(definition.definition_id, 0));
        let planner = NativeEscrowPlanner::new(
            Participant::Maker,
            PrivateKey::try_new([71; 32]).unwrap(),
            [0x1020_3040; 8],
            [0x5060_7080; 8],
            fixture.runtime.clone(),
            Arc::new(FixedNonce),
        )
        .unwrap();
        let prepared = planner
            .prepare_witnessed_asset_refund_v2(&PrepareWitnessedAssetRefundV2Request::new(
                MessageContext::new(
                    RunId::new("asset-refund-fork-run-0001").unwrap(),
                    RequestId::new("asset-refund-fork-prepare-0001").unwrap(),
                    Participant::Maker,
                ),
                fixture.runtime.clone(),
                fixture.terms.clone(),
            ))
            .await
            .unwrap();
        let public =
            decode_official_public_transaction(prepared.refund.exact_bytes.as_slice()).unwrap();
        let old_blocks = [
            finalized_block(10, vec![Transaction::Public(indexed_public(&public))]),
            finalized_block(11, Vec::new()),
        ];
        let mut new_tip = old_blocks[1].clone();
        new_tip.header.hash = HashType([111; 32]);
        new_tip.header.signature = IndexedSignature([111; 64]);
        let accounts = at_height(fixture.accounts, 11).collect::<BTreeMap<_, _>>();
        let indexer = Arc::new(ForkAfterScanIndexer {
            old: ScanIndexer {
                tip: 11,
                by_id: old_blocks
                    .iter()
                    .map(|block| (block.header.block_id, block.clone()))
                    .collect(),
                by_hash: old_blocks
                    .iter()
                    .map(|block| (block.header.hash.0, block.clone()))
                    .collect(),
                accounts: accounts.clone(),
            },
            new_tip,
            new_accounts: accounts,
            tip_reads: AtomicUsize::new(0),
            changed: AtomicBool::new(false),
        });
        let service = FinalizedAssetObserver::new(fixture.runtime.clone(), indexer);
        let outcome = service
            .observe_refund(&ObserveWitnessedAssetRefundV2Request::new(
                MessageContext::new(
                    RunId::new("asset-refund-fork-run-0001").unwrap(),
                    RequestId::new("asset-refund-fork-observe-0001").unwrap(),
                    Participant::Maker,
                ),
                fixture.runtime,
                fixture.terms,
                NativeRefundObservationTarget::DiscoverByTerms {
                    window: DiscoveryWindow::new(10, 2).unwrap(),
                },
            ))
            .await
            .unwrap();
        assert!(matches!(
            outcome.refund,
            WitnessedAssetRefundObservationV2::UnknownOrPending
        ));
    }

    #[tokio::test]
    async fn refund_state_only_remains_transaction_free_and_not_requested() {
        let definition = token_fixture(EscrowStatus::Refunded, HistoricalAccount::Absent);
        let fixture = token_fixture(EscrowStatus::Refunded, holding(definition.definition_id, 0));
        let tip = finalized_block(11, Vec::new());
        let indexer = Arc::new(ScanIndexer {
            tip: 11,
            by_id: BTreeMap::from([(11, tip.clone())]),
            by_hash: BTreeMap::from([(tip.header.hash.0, tip)]),
            accounts: at_height(fixture.accounts, 11).collect(),
        });
        let service = FinalizedAssetObserver::new(fixture.runtime.clone(), indexer);
        let outcome = service
            .observe_refund(&ObserveWitnessedAssetRefundV2Request::new(
                MessageContext::new(
                    RunId::new("asset-refund-state-run-0001").unwrap(),
                    RequestId::new("asset-refund-state-observe-0001").unwrap(),
                    Participant::Maker,
                ),
                fixture.runtime,
                fixture.terms,
                NativeRefundObservationTarget::StateOnly,
            ))
            .await
            .unwrap();
        assert!(matches!(
            outcome.refund,
            WitnessedAssetRefundObservationV2::NotRequested
        ));
    }
}
