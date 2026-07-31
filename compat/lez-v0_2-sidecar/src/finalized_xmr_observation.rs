use std::sync::Arc;

use borsh::BorshDeserialize as _;
use common::transaction::LeeTransaction;
use indexer_service_protocol::{
    BedrockStatus, Block, BlockHeader, PublicTransaction as IndexedPublicTransaction,
    Transaction as IndexedTransaction,
};
use lez_bridge_protocol::{
    AccountIds, AggregateBip340Signature, ChainClock, ChainPosition,
    ClassifyFinalizedNativeXmrEffectV3Request, ClassifyFinalizedNativeXmrEffectV3Result,
    FinalizedBlockIdentity, FinalizedNativeXmrEffectFactsV3, FinalizedNativeXmrScanOutcomeV3,
    FinalizedNativeXmrTransactionTargetV3, FinalizedNativeXmrUnavailableReasonV3, Hex32,
    NativeCustodyFacts, ObservedTransactionFacts, PreparedTransaction, XmrNativeEffectV3,
    XmrNativeEscrowMetadataFactsV3, XmrNativeEscrowStateV3, XmrNativeEscrowTermsV3,
    XmrNativeInstructionFactsV3,
};
use lez_zec_escrow_v02::{ClaimAuthority, EscrowMetadata, EscrowStatus};
use nssa::AccountId;

use crate::{
    BridgeRuntimeError, FinalizedIndexerApi, HistoricalAccount, NativeEscrowPlanner,
    ZecEscrowInstruction, finalized_claim_observation::decode_indexed_public,
    native_prepare::xmr_claim_partial_matches_terms, prepared_from_transaction,
    program_id_from_hex,
};

enum ClassifiedFailure {
    Outcome(FinalizedNativeXmrUnavailableReasonV3),
    Runtime(BridgeRuntimeError),
}

type Classified<T> = Result<T, ClassifiedFailure>;

fn finality_failure(error: BridgeRuntimeError) -> ClassifiedFailure {
    match error {
        BridgeRuntimeError::MovingTip => {
            ClassifiedFailure::Outcome(FinalizedNativeXmrUnavailableReasonV3::MovingTip)
        }
        BridgeRuntimeError::Unavailable => {
            ClassifiedFailure::Outcome(FinalizedNativeXmrUnavailableReasonV3::FinalityUnavailable)
        }
        error => ClassifiedFailure::Runtime(error),
    }
}

fn history_failure(error: BridgeRuntimeError) -> ClassifiedFailure {
    match error {
        BridgeRuntimeError::MovingTip => {
            ClassifiedFailure::Outcome(FinalizedNativeXmrUnavailableReasonV3::MovingTip)
        }
        BridgeRuntimeError::Unavailable => {
            ClassifiedFailure::Outcome(FinalizedNativeXmrUnavailableReasonV3::HistoryUnavailable)
        }
        error => ClassifiedFailure::Runtime(error),
    }
}

struct StableXmrWindow {
    blocks: Vec<Block>,
    requested_end: u64,
    finalized_clock: ChainClock,
}

impl StableXmrWindow {
    fn block(&self, block_id: u64) -> Result<&Block, BridgeRuntimeError> {
        self.blocks
            .iter()
            .find(|block| block.header.block_id == block_id)
            .ok_or(BridgeRuntimeError::InvalidObservation)
    }

    async fn confirm_block(
        &self,
        observer: &FinalizedNativeXmrEffectObserver,
        block_id: u64,
    ) -> Classified<()> {
        let expected = self.block(block_id).map_err(ClassifiedFailure::Runtime)?;
        let reread = observer.read_finalized_block(block_id).await?;
        if &reread != expected {
            return Err(ClassifiedFailure::Outcome(
                FinalizedNativeXmrUnavailableReasonV3::MovingTip,
            ));
        }
        Ok(())
    }

    async fn confirm_requested_end(
        &self,
        observer: &FinalizedNativeXmrEffectObserver,
    ) -> Classified<()> {
        self.confirm_block(observer, self.requested_end).await
    }

    async fn confirm_finalized_coverage(
        &self,
        observer: &FinalizedNativeXmrEffectObserver,
    ) -> Classified<()> {
        let finalized_tip = observer
            .indexer
            .last_finalized_block_id()
            .await
            .map_err(finality_failure)?
            .ok_or(ClassifiedFailure::Outcome(
                FinalizedNativeXmrUnavailableReasonV3::FinalityUnavailable,
            ))?;
        if finalized_tip < self.requested_end {
            return Err(ClassifiedFailure::Outcome(
                FinalizedNativeXmrUnavailableReasonV3::FinalityUnavailable,
            ));
        }
        self.confirm_requested_end(observer).await
    }
}

struct EffectCandidate {
    transaction: ObservedTransactionFacts,
    instruction: XmrNativeInstructionFactsV3,
    aggregate_signature: Option<AggregateBip340Signature>,
    block: BlockHeader,
}

enum Scan {
    Found(Box<EffectCandidate>, Box<StableXmrWindow>),
    Missing(Box<StableXmrWindow>),
}

/// Read-only finalized classifier for durable and counterparty native-XMR effects.
pub(crate) struct FinalizedNativeXmrEffectObserver {
    planner: Arc<NativeEscrowPlanner>,
    indexer: Arc<dyn FinalizedIndexerApi>,
}

impl FinalizedNativeXmrEffectObserver {
    pub(crate) fn new(
        planner: Arc<NativeEscrowPlanner>,
        indexer: Arc<dyn FinalizedIndexerApi>,
    ) -> Self {
        Self { planner, indexer }
    }

    #[allow(
        clippy::too_many_lines,
        reason = "the four typed outcomes remain explicit at the finalized authority boundary"
    )]
    pub(crate) async fn classify(
        &self,
        request: &ClassifyFinalizedNativeXmrEffectV3Request,
    ) -> Result<ClassifyFinalizedNativeXmrEffectV3Result, BridgeRuntimeError> {
        self.planner
            .validate_xmr_terms_v3_binding(&request.context, &request.runtime, &request.terms)
            .map_err(BridgeRuntimeError::from)?;
        match &request.target {
            FinalizedNativeXmrTransactionTargetV3::Exact { transaction } => self
                .planner
                .validate_owned_xmr_effect_v3(
                    &request.context,
                    &request.runtime,
                    &request.terms,
                    request.effect,
                    transaction,
                )
                .map_err(BridgeRuntimeError::from)?,
            FinalizedNativeXmrTransactionTargetV3::DiscoverByTerms {} => self
                .planner
                .validate_xmr_effect_discovery_v3(
                    &request.context,
                    &request.runtime,
                    &request.terms,
                    request.effect,
                )
                .map_err(BridgeRuntimeError::from)?,
        }

        let scan = match self.scan_effect(request).await {
            Ok(scan) => scan,
            Err(ClassifiedFailure::Outcome(reason)) => {
                return Self::result(
                    request,
                    FinalizedNativeXmrScanOutcomeV3::unavailable(reason),
                );
            }
            Err(ClassifiedFailure::Runtime(error)) => return Err(error),
        };
        match scan {
            Scan::Found(candidate, stable) => {
                let (metadata, custody) = match self
                    .read_effect_state(&request.terms, request.effect, candidate.block.block_id)
                    .await
                {
                    Ok(state) => state,
                    Err(ClassifiedFailure::Outcome(reason)) => {
                        return Self::result(
                            request,
                            FinalizedNativeXmrScanOutcomeV3::unavailable(reason),
                        );
                    }
                    Err(ClassifiedFailure::Runtime(error)) => return Err(error),
                };
                if let Err(failure) = stable.confirm_block(self, candidate.block.block_id).await {
                    return match failure {
                        ClassifiedFailure::Outcome(reason) => Self::result(
                            request,
                            FinalizedNativeXmrScanOutcomeV3::unavailable(reason),
                        ),
                        ClassifiedFailure::Runtime(error) => Err(error),
                    };
                }
                if let Err(failure) = stable.confirm_finalized_coverage(self).await {
                    return match failure {
                        ClassifiedFailure::Outcome(reason) => Self::result(
                            request,
                            FinalizedNativeXmrScanOutcomeV3::unavailable(reason),
                        ),
                        ClassifiedFailure::Runtime(error) => Err(error),
                    };
                }
                let facts = FinalizedNativeXmrEffectFactsV3::new(
                    candidate.transaction,
                    candidate.instruction,
                    candidate.aggregate_signature,
                    containing_block(&candidate.block),
                    metadata,
                    custody,
                );
                Self::result(
                    request,
                    FinalizedNativeXmrScanOutcomeV3::found(
                        stable.finalized_clock,
                        request.window,
                        facts,
                    ),
                )
            }
            Scan::Missing(stable) => {
                if let Err(failure) = self
                    .validate_missing_state(&request.terms, stable.requested_end)
                    .await
                {
                    return match failure {
                        ClassifiedFailure::Outcome(reason) => Self::result(
                            request,
                            FinalizedNativeXmrScanOutcomeV3::unavailable(reason),
                        ),
                        ClassifiedFailure::Runtime(error) => Err(error),
                    };
                }
                if let Err(failure) = stable.confirm_finalized_coverage(self).await {
                    return match failure {
                        ClassifiedFailure::Outcome(reason) => Self::result(
                            request,
                            FinalizedNativeXmrScanOutcomeV3::unavailable(reason),
                        ),
                        ClassifiedFailure::Runtime(error) => Err(error),
                    };
                }
                Self::result(
                    request,
                    FinalizedNativeXmrScanOutcomeV3::uncertain(
                        stable.finalized_clock,
                        request.window,
                    ),
                )
            }
        }
    }

    fn result(
        request: &ClassifyFinalizedNativeXmrEffectV3Request,
        outcome: FinalizedNativeXmrScanOutcomeV3,
    ) -> Result<ClassifyFinalizedNativeXmrEffectV3Result, BridgeRuntimeError> {
        ClassifyFinalizedNativeXmrEffectV3Result::new(
            request.context.clone(),
            request.terms,
            request.effect,
            request.target.clone(),
            outcome,
        )
        .map_err(|_| BridgeRuntimeError::InvalidObservation)
    }

    async fn scan_effect(
        &self,
        request: &ClassifyFinalizedNativeXmrEffectV3Request,
    ) -> Classified<Scan> {
        let stable = self.read_window(request.window).await?;
        let mut found = None;
        for block in &stable.blocks {
            for (index, indexed) in block.body.transactions.iter().enumerate() {
                let (public, expected) = match &request.target {
                    FinalizedNativeXmrTransactionTargetV3::Exact { transaction } => {
                        if indexed.hash().0 != *transaction.transaction_id.as_bytes() {
                            continue;
                        }
                        let IndexedTransaction::Public(public) = indexed else {
                            return Err(ClassifiedFailure::Runtime(
                                BridgeRuntimeError::InvalidObservation,
                            ));
                        };
                        (public, Some(transaction))
                    }
                    FinalizedNativeXmrTransactionTargetV3::DiscoverByTerms {} => {
                        let IndexedTransaction::Public(public) = indexed else {
                            continue;
                        };
                        if !Self::matches_discovery_terms(&request.terms, request.effect, public)? {
                            continue;
                        }
                        (public, None)
                    }
                };
                let candidate = Self::effect_candidate(
                    &request.terms,
                    request.effect,
                    expected,
                    public,
                    &block.header,
                    index,
                )?;
                if found.replace(candidate).is_some() {
                    return Err(ClassifiedFailure::Outcome(
                        FinalizedNativeXmrUnavailableReasonV3::ConflictingMatches,
                    ));
                }
            }
        }
        if let Some(candidate) = found {
            stable.confirm_block(self, candidate.block.block_id).await?;
            Ok(Scan::Found(Box::new(candidate), Box::new(stable)))
        } else {
            Ok(Scan::Missing(Box::new(stable)))
        }
    }

    fn matches_discovery_terms(
        terms: &XmrNativeEscrowTermsV3,
        effect: XmrNativeEffectV3,
        indexed: &IndexedPublicTransaction,
    ) -> Classified<bool> {
        let input = terms.to_input();
        if indexed.message.program_id.0 != program_id_from_hex(input.escrow_program_id) {
            return Ok(false);
        }
        let instruction = risc0_zkvm::serde::from_slice::<ZecEscrowInstruction, u32>(
            &indexed.message.instruction_data,
        )
        .map_err(|_| ClassifiedFailure::Runtime(BridgeRuntimeError::InvalidObservation))?;
        Ok(match (effect, instruction) {
            (
                XmrNativeEffectV3::AuthorizeClaim,
                ZecEscrowInstruction::AuthorizeNativeXmrClaim { swap_id, .. },
            ) => swap_id == *input.swap_id.as_bytes(),
            (XmrNativeEffectV3::Claim, ZecEscrowInstruction::ClaimNativeXmr { swap_id })
            | (XmrNativeEffectV3::Refund, ZecEscrowInstruction::RefundNativeXmr { swap_id }) => {
                swap_id == *input.swap_id.as_bytes()
            }
            _ => false,
        })
    }

    #[allow(
        clippy::too_many_lines,
        reason = "one candidate check keeps canonical bytes, ABI, accounts, and signers joined"
    )]
    fn effect_candidate(
        terms: &XmrNativeEscrowTermsV3,
        effect: XmrNativeEffectV3,
        expected: Option<&PreparedTransaction>,
        indexed: &IndexedPublicTransaction,
        block: &BlockHeader,
        index: usize,
    ) -> Classified<EffectCandidate> {
        if indexed.witness_set.proof.is_some() {
            return Err(ClassifiedFailure::Runtime(
                BridgeRuntimeError::InvalidObservation,
            ));
        }
        let public = decode_indexed_public(indexed).map_err(ClassifiedFailure::Runtime)?;
        if public.hash() != indexed.hash.0 {
            return Err(ClassifiedFailure::Runtime(
                BridgeRuntimeError::InvalidObservation,
            ));
        }
        LeeTransaction::Public(public.clone())
            .transaction_stateless_check()
            .map_err(|_| ClassifiedFailure::Runtime(BridgeRuntimeError::InvalidObservation))?;
        let prepared = prepared_from_transaction(&public)
            .map_err(|_| ClassifiedFailure::Runtime(BridgeRuntimeError::InvalidObservation))?;
        if expected.is_some_and(|expected| expected != &prepared) {
            return Err(ClassifiedFailure::Runtime(
                BridgeRuntimeError::InvalidObservation,
            ));
        }
        let input = terms.to_input();
        if public.message().program_id != program_id_from_hex(input.escrow_program_id)
            || public.message().nonces.len() != 1
        {
            return Err(ClassifiedFailure::Runtime(
                BridgeRuntimeError::InvalidObservation,
            ));
        }
        if effect == XmrNativeEffectV3::Refund
            && !(input.refund_at_ms..input.punish_at_ms).contains(&block.timestamp)
        {
            return Err(ClassifiedFailure::Runtime(
                BridgeRuntimeError::InvalidObservation,
            ));
        }
        let observed_instruction = risc0_zkvm::serde::from_slice::<ZecEscrowInstruction, u32>(
            &public.message().instruction_data,
        )
        .map_err(|_| ClassifiedFailure::Runtime(BridgeRuntimeError::InvalidObservation))?;
        let published_claim_partial = if effect == XmrNativeEffectV3::AuthorizeClaim {
            let ZecEscrowInstruction::AuthorizeNativeXmrClaim {
                swap_id,
                claim_partial,
            } = observed_instruction
            else {
                return Err(ClassifiedFailure::Runtime(
                    BridgeRuntimeError::InvalidObservation,
                ));
            };
            if swap_id != *input.swap_id.as_bytes()
                || !xmr_claim_partial_matches_terms(terms, &claim_partial)
            {
                return Err(ClassifiedFailure::Runtime(
                    BridgeRuntimeError::InvalidObservation,
                ));
            }
            Some(Hex32::from_bytes(claim_partial))
        } else {
            None
        };
        let canonical_instruction = match effect {
            XmrNativeEffectV3::Initialize => ZecEscrowInstruction::InitializeNativeXmr {
                swap_id: *input.swap_id.as_bytes(),
                terms_hash: *input.activation_commitment.as_bytes(),
                claim_aggregate_x_only_public_key: *input
                    .claim_aggregate_x_only_public_key
                    .as_bytes(),
                refund_aggregate_x_only_public_key: *input
                    .refund_aggregate_x_only_public_key
                    .as_bytes(),
                maker_dleq_transcript_commitment: *input
                    .maker_dleq_transcript_commitment
                    .as_bytes(),
                taker_dleq_transcript_commitment: *input
                    .taker_dleq_transcript_commitment
                    .as_bytes(),
                claim_partial_context_binding: *input.claim_partial_context_binding.as_bytes(),
                claim_partial_commitment: *input.claim_partial_commitment.as_bytes(),
                amount: input.amount,
                refund_at: input.refund_at_ms,
                punish_at: input.punish_at_ms,
                authenticated_transfer_program: program_id_from_hex(
                    input.authenticated_transfer_program_id,
                ),
            },
            XmrNativeEffectV3::Fund => ZecEscrowInstruction::FundNative {
                swap_id: *input.swap_id.as_bytes(),
            },
            XmrNativeEffectV3::AuthorizeClaim => ZecEscrowInstruction::AuthorizeNativeXmrClaim {
                swap_id: *input.swap_id.as_bytes(),
                claim_partial: *published_claim_partial
                    .ok_or(ClassifiedFailure::Runtime(
                        BridgeRuntimeError::InvalidObservation,
                    ))?
                    .as_bytes(),
            },
            XmrNativeEffectV3::Claim => ZecEscrowInstruction::ClaimNativeXmr {
                swap_id: *input.swap_id.as_bytes(),
            },
            XmrNativeEffectV3::Refund => ZecEscrowInstruction::RefundNativeXmr {
                swap_id: *input.swap_id.as_bytes(),
            },
            XmrNativeEffectV3::Punish => {
                return Err(ClassifiedFailure::Runtime(
                    BridgeRuntimeError::InvalidObservation,
                ));
            }
        };
        let canonical_instruction = risc0_zkvm::serde::to_vec(&canonical_instruction)
            .map_err(|_| ClassifiedFailure::Runtime(BridgeRuntimeError::InvalidObservation))?;
        if public.message().instruction_data != canonical_instruction {
            return Err(ClassifiedFailure::Runtime(
                BridgeRuntimeError::InvalidObservation,
            ));
        }
        let accounts = AccountIds::new(
            public
                .message()
                .account_ids
                .iter()
                .map(|account| Hex32::from_bytes(account.into_value()))
                .collect(),
        )
        .map_err(|_| ClassifiedFailure::Runtime(BridgeRuntimeError::InvalidObservation))?;
        let expected_accounts = match effect {
            XmrNativeEffectV3::Initialize => vec![
                input.metadata_account_id,
                input.custody_account_id,
                input.depositor_account_id,
                input.claimant_account_id,
                input.claim_authority_account_id,
                input.refund_authority_account_id,
            ],
            XmrNativeEffectV3::Fund => vec![
                input.metadata_account_id,
                input.custody_account_id,
                input.depositor_account_id,
            ],
            XmrNativeEffectV3::AuthorizeClaim => {
                vec![input.metadata_account_id, input.depositor_account_id]
            }
            XmrNativeEffectV3::Claim => vec![
                input.metadata_account_id,
                input.custody_account_id,
                input.claimant_account_id,
                input.claim_authority_account_id,
            ],
            XmrNativeEffectV3::Refund => vec![
                input.metadata_account_id,
                input.custody_account_id,
                input.depositor_account_id,
                input.refund_authority_account_id,
            ],
            XmrNativeEffectV3::Punish => {
                return Err(ClassifiedFailure::Runtime(
                    BridgeRuntimeError::InvalidObservation,
                ));
            }
        };
        let expected_accounts = AccountIds::new(expected_accounts)
            .map_err(|_| ClassifiedFailure::Runtime(BridgeRuntimeError::InvalidObservation))?;
        if accounts != expected_accounts {
            return Err(ClassifiedFailure::Runtime(
                BridgeRuntimeError::InvalidObservation,
            ));
        }
        let witnesses = public.witness_set().signatures_and_public_keys();
        let signer_ids = AccountIds::new(
            witnesses
                .iter()
                .map(|(_, key)| Hex32::from_bytes(AccountId::from(key).into_value()))
                .collect(),
        )
        .map_err(|_| ClassifiedFailure::Runtime(BridgeRuntimeError::InvalidObservation))?;
        let expected_signer = match effect {
            XmrNativeEffectV3::Claim => input.claim_authority_account_id,
            XmrNativeEffectV3::Refund => input.refund_authority_account_id,
            _ => input.depositor_account_id,
        };
        let expected_signers = AccountIds::new(vec![expected_signer])
            .map_err(|_| ClassifiedFailure::Runtime(BridgeRuntimeError::InvalidObservation))?;
        if signer_ids != expected_signers {
            return Err(ClassifiedFailure::Runtime(
                BridgeRuntimeError::InvalidObservation,
            ));
        }
        let aggregate_signature = match effect {
            XmrNativeEffectV3::Claim | XmrNativeEffectV3::Refund => {
                let [(signature, public_key)] = witnesses else {
                    return Err(ClassifiedFailure::Runtime(
                        BridgeRuntimeError::InvalidObservation,
                    ));
                };
                let (expected_key, expected_hash) = match effect {
                    XmrNativeEffectV3::Claim => (
                        input.claim_aggregate_x_only_public_key,
                        input.claim_message_hash,
                    ),
                    XmrNativeEffectV3::Refund => (
                        input.refund_aggregate_x_only_public_key,
                        input.refund_message_hash,
                    ),
                    _ => unreachable!("effect is narrowed above"),
                };
                if public_key.value() != expected_key.as_bytes()
                    || public.message().hash() != *expected_hash.as_bytes()
                {
                    return Err(ClassifiedFailure::Runtime(
                        BridgeRuntimeError::InvalidObservation,
                    ));
                }
                Some(AggregateBip340Signature::from_bytes(signature.value))
            }
            _ => None,
        };
        let instruction = XmrNativeInstructionFactsV3::new(
            effect,
            input.escrow_program_id,
            accounts,
            input.swap_id,
            Hex32::from_bytes(public.message().hash()),
            published_claim_partial,
        )
        .map_err(|_| ClassifiedFailure::Runtime(BridgeRuntimeError::InvalidObservation))?;
        Ok(EffectCandidate {
            transaction: ObservedTransactionFacts::new(
                prepared.transaction_id,
                prepared.exact_bytes,
                ChainPosition::new(
                    Hex32::from_bytes(block.hash.0),
                    block.block_id,
                    u32::try_from(index).map_err(|_| {
                        ClassifiedFailure::Runtime(BridgeRuntimeError::InvalidObservation)
                    })?,
                ),
                signer_ids,
                true,
            ),
            instruction,
            aggregate_signature,
            block: block.clone(),
        })
    }

    async fn read_window(
        &self,
        window: lez_bridge_protocol::DiscoveryWindow,
    ) -> Classified<StableXmrWindow> {
        let finalized_tip = self
            .indexer
            .last_finalized_block_id()
            .await
            .map_err(finality_failure)?
            .ok_or(ClassifiedFailure::Outcome(
                FinalizedNativeXmrUnavailableReasonV3::FinalityUnavailable,
            ))?;
        let requested_end = window
            .start_height()
            .checked_add(u64::from(window.max_blocks().saturating_sub(1)))
            .ok_or(ClassifiedFailure::Runtime(
                BridgeRuntimeError::InvalidObservation,
            ))?;
        if requested_end > finalized_tip {
            return Err(ClassifiedFailure::Outcome(
                FinalizedNativeXmrUnavailableReasonV3::FinalityUnavailable,
            ));
        }
        let mut blocks = Vec::with_capacity(
            usize::try_from(window.max_blocks())
                .map_err(|_| ClassifiedFailure::Runtime(BridgeRuntimeError::InvalidObservation))?,
        );
        let mut previous_hash = None;
        for block_id in window.start_height()..=requested_end {
            let block = self.read_finalized_block(block_id).await?;
            if previous_hash.is_some_and(|hash| block.header.prev_block_hash != hash) {
                return Err(ClassifiedFailure::Runtime(
                    BridgeRuntimeError::InvalidObservation,
                ));
            }
            previous_hash = Some(block.header.hash);
            blocks.push(block);
        }
        let requested_end_block = blocks.last().ok_or(ClassifiedFailure::Runtime(
            BridgeRuntimeError::InvalidObservation,
        ))?;
        let confirmed_end = self.read_finalized_block(requested_end).await?;
        if &confirmed_end != requested_end_block {
            return Err(ClassifiedFailure::Outcome(
                FinalizedNativeXmrUnavailableReasonV3::MovingTip,
            ));
        }
        Ok(StableXmrWindow {
            finalized_clock: ChainClock::new(
                Hex32::from_bytes(requested_end_block.header.hash.0),
                requested_end_block.header.block_id,
                requested_end_block.header.timestamp,
            ),
            blocks,
            requested_end,
        })
    }

    async fn read_finalized_block(&self, block_id: u64) -> Classified<Block> {
        let by_id = self
            .indexer
            .block_by_id(block_id)
            .await
            .map_err(history_failure)?
            .ok_or(ClassifiedFailure::Outcome(
                FinalizedNativeXmrUnavailableReasonV3::HistoryUnavailable,
            ))?;
        if by_id.header.block_id != block_id || by_id.bedrock_status != BedrockStatus::Finalized {
            return Err(ClassifiedFailure::Runtime(
                BridgeRuntimeError::InvalidObservation,
            ));
        }
        let by_hash = self
            .indexer
            .block_by_hash(by_id.header.hash.0)
            .await
            .map_err(history_failure)?
            .ok_or(ClassifiedFailure::Outcome(
                FinalizedNativeXmrUnavailableReasonV3::HistoryUnavailable,
            ))?;
        if by_hash != by_id {
            return Err(ClassifiedFailure::Runtime(
                BridgeRuntimeError::InvalidObservation,
            ));
        }
        Ok(by_id)
    }

    async fn read_effect_state(
        &self,
        terms: &XmrNativeEscrowTermsV3,
        effect: XmrNativeEffectV3,
        block_id: u64,
    ) -> Classified<(XmrNativeEscrowMetadataFactsV3, NativeCustodyFacts)> {
        let input = terms.to_input();
        let (metadata, custody) = tokio::join!(
            self.indexer
                .account_at_block(*input.metadata_account_id.as_bytes(), block_id),
            self.indexer
                .account_at_block(*input.custody_account_id.as_bytes(), block_id),
        );
        let metadata = metadata.map_err(history_failure)?;
        let custody = custody.map_err(history_failure)?;
        let HistoricalAccount::Present(metadata) = metadata else {
            return Err(ClassifiedFailure::Runtime(
                BridgeRuntimeError::InvalidObservation,
            ));
        };
        let HistoricalAccount::Present(custody) = custody else {
            return Err(ClassifiedFailure::Runtime(
                BridgeRuntimeError::InvalidObservation,
            ));
        };
        let (expected_state, expected_balance) = match effect {
            XmrNativeEffectV3::Initialize => (XmrNativeEscrowStateV3::Empty, 0),
            XmrNativeEffectV3::Fund => (XmrNativeEscrowStateV3::Funded, input.amount),
            XmrNativeEffectV3::AuthorizeClaim => {
                (XmrNativeEscrowStateV3::ClaimAuthorized, input.amount)
            }
            XmrNativeEffectV3::Claim => (XmrNativeEscrowStateV3::Claimed, 0),
            XmrNativeEffectV3::Refund => (XmrNativeEscrowStateV3::Refunded, 0),
            XmrNativeEffectV3::Punish => {
                return Err(ClassifiedFailure::Runtime(
                    BridgeRuntimeError::InvalidObservation,
                ));
            }
        };
        let state = validate_metadata(terms, &metadata)?;
        if state != expected_state {
            return Err(ClassifiedFailure::Runtime(
                BridgeRuntimeError::InvalidObservation,
            ));
        }
        validate_custody(terms, &custody, expected_balance)?;
        Ok((
            XmrNativeEscrowMetadataFactsV3::from_terms(*terms, state),
            NativeCustodyFacts::new(
                input.custody_account_id,
                input.authenticated_transfer_program_id,
                expected_balance,
            ),
        ))
    }

    async fn validate_missing_state(
        &self,
        terms: &XmrNativeEscrowTermsV3,
        block_id: u64,
    ) -> Classified<()> {
        let input = terms.to_input();
        let (metadata, custody) = tokio::join!(
            self.indexer
                .account_at_block(*input.metadata_account_id.as_bytes(), block_id),
            self.indexer
                .account_at_block(*input.custody_account_id.as_bytes(), block_id),
        );
        let metadata = metadata.map_err(history_failure)?;
        let custody = custody.map_err(history_failure)?;
        match (metadata, custody) {
            (HistoricalAccount::Absent, HistoricalAccount::Absent) => Ok(()),
            (HistoricalAccount::Present(metadata), HistoricalAccount::Present(custody)) => {
                let state = validate_metadata(terms, &metadata)?;
                let balance = match state {
                    XmrNativeEscrowStateV3::Empty
                    | XmrNativeEscrowStateV3::Claimed
                    | XmrNativeEscrowStateV3::Refunded => 0,
                    XmrNativeEscrowStateV3::Funded | XmrNativeEscrowStateV3::ClaimAuthorized => {
                        input.amount
                    }
                };
                validate_custody(terms, &custody, balance)?;
                Ok(())
            }
            _ => Err(ClassifiedFailure::Runtime(
                BridgeRuntimeError::InvalidObservation,
            )),
        }
    }
}

fn validate_metadata(
    terms: &XmrNativeEscrowTermsV3,
    account: &indexer_service_protocol::Account,
) -> Classified<XmrNativeEscrowStateV3> {
    let input = terms.to_input();
    let metadata = EscrowMetadata::try_from_slice(account.data.0.as_ref())
        .map_err(|_| ClassifiedFailure::Runtime(BridgeRuntimeError::InvalidObservation))?;
    let ClaimAuthority::XmrDualAdaptor {
        claim_aggregate_x_only_public_key,
        claim_aggregate_account_id,
        refund_aggregate_x_only_public_key,
        refund_aggregate_account_id,
        maker_dleq_transcript_commitment,
        taker_dleq_transcript_commitment,
        claim_partial_context_binding,
        claim_partial_commitment,
        punish_at,
    } = metadata.claim_authority
    else {
        return Err(ClassifiedFailure::Runtime(
            BridgeRuntimeError::InvalidObservation,
        ));
    };
    if account.program_owner.0 != program_id_from_hex(input.escrow_program_id)
        || metadata.version != 3
        || metadata.swap_id != *input.swap_id.as_bytes()
        || metadata.terms_hash != *input.activation_commitment.as_bytes()
        || claim_aggregate_x_only_public_key != *input.claim_aggregate_x_only_public_key.as_bytes()
        || claim_aggregate_account_id.into_value() != *input.claim_authority_account_id.as_bytes()
        || refund_aggregate_x_only_public_key
            != *input.refund_aggregate_x_only_public_key.as_bytes()
        || refund_aggregate_account_id.into_value() != *input.refund_authority_account_id.as_bytes()
        || maker_dleq_transcript_commitment != *input.maker_dleq_transcript_commitment.as_bytes()
        || taker_dleq_transcript_commitment != *input.taker_dleq_transcript_commitment.as_bytes()
        || claim_partial_context_binding != *input.claim_partial_context_binding.as_bytes()
        || claim_partial_commitment != *input.claim_partial_commitment.as_bytes()
        || punish_at != input.punish_at_ms
        || metadata.depositor.into_value() != *input.depositor_account_id.as_bytes()
        || metadata.depositor_asset != metadata.depositor
        || metadata.claimant.into_value() != *input.claimant_account_id.as_bytes()
        || metadata.claimant_asset != metadata.claimant
        || metadata.custody.into_value() != *input.custody_account_id.as_bytes()
        || metadata.asset_program != program_id_from_hex(input.authenticated_transfer_program_id)
        || metadata.custody_program != metadata.asset_program
        || metadata.asset_definition != [0; 32]
        || metadata.amount != input.amount
        || metadata.refund_at != input.refund_at_ms
    {
        return Err(ClassifiedFailure::Runtime(
            BridgeRuntimeError::InvalidObservation,
        ));
    }
    Ok(match metadata.status {
        EscrowStatus::Empty => XmrNativeEscrowStateV3::Empty,
        EscrowStatus::Funded => XmrNativeEscrowStateV3::Funded,
        EscrowStatus::XmrClaimAuthorized => XmrNativeEscrowStateV3::ClaimAuthorized,
        EscrowStatus::Claimed => XmrNativeEscrowStateV3::Claimed,
        EscrowStatus::Refunded => XmrNativeEscrowStateV3::Refunded,
    })
}

fn validate_custody(
    terms: &XmrNativeEscrowTermsV3,
    account: &indexer_service_protocol::Account,
    expected_balance: u128,
) -> Classified<()> {
    let input = terms.to_input();
    if account.program_owner.0 != program_id_from_hex(input.authenticated_transfer_program_id)
        || account.balance != expected_balance
    {
        return Err(ClassifiedFailure::Runtime(
            BridgeRuntimeError::InvalidObservation,
        ));
    }
    Ok(())
}

fn containing_block(block: &BlockHeader) -> FinalizedBlockIdentity {
    FinalizedBlockIdentity::new(
        block.block_id,
        Hex32::from_bytes(block.hash.0),
        block.timestamp,
    )
}
