use std::{fmt, sync::Arc};

use borsh::BorshDeserialize as _;
use common::transaction::LeeTransaction;
use indexer_service_protocol::{
    BedrockStatus, Block, BlockHeader, PublicTransaction as IndexedPublicTransaction,
    Transaction as IndexedTransaction,
};
use lez_bridge_protocol::{
    AccountIds, ChainClock, ChainPosition, EscrowState, Hex32, MAX_DISCOVERY_BLOCKS,
    NativeCustodyFacts, NativeEscrowAccountFacts, NativeEscrowAccountObservation,
    NativeRefundFoundFacts, NativeRefundInstructionFacts, NativeRefundObservation,
    NativeRefundObservationTarget, ObserveNativeRefundRequest, ObserveNativeRefundResult,
    ObservedTransactionFacts, PreparedTransaction, WitnessedEscrowMetadataFacts,
    WitnessedNativeEscrowTerms,
};
use lez_zec_escrow_v02::{ClaimAuthority, EscrowMetadata, EscrowStatus};
use nssa::{AccountId, PublicKey, public_transaction::Message};

use crate::finalized_claim_observation::decode_indexed_public;
use crate::{
    BridgeRuntimeError, FinalizedIndexerApi, NativeEscrowPlanner, ZecEscrowInstruction,
    compute_custody_pda, compute_metadata_pda, prepared_from_transaction, program_id_from_hex,
    program_id_to_hex,
};

struct FoundRefund {
    header: BlockHeader,
    transaction_index: usize,
    public: IndexedPublicTransaction,
}

/// Fail-closed observer for witnessed native refunds in one stable finalized window.
pub struct FinalizedWitnessedRefundObserver {
    runtime: lez_bridge_protocol::RuntimeDescriptor,
    planner: Arc<NativeEscrowPlanner>,
    indexer: Arc<dyn FinalizedIndexerApi>,
}

impl fmt::Debug for FinalizedWitnessedRefundObserver {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FinalizedWitnessedRefundObserver")
            .field("runtime", &self.runtime)
            .finish_non_exhaustive()
    }
}

impl FinalizedWitnessedRefundObserver {
    /// Binds one immutable actor/runtime and planner identity to a finalized indexer.
    #[must_use]
    pub fn new(
        runtime: lez_bridge_protocol::RuntimeDescriptor,
        planner: Arc<NativeEscrowPlanner>,
        indexer: Arc<dyn FinalizedIndexerApi>,
    ) -> Self {
        Self {
            runtime,
            planner,
            indexer,
        }
    }

    /// Observes state, an exact owned refund, or a terms-discovered counterparty refund.
    ///
    /// # Errors
    ///
    /// Fails closed on actor/runtime/authority drift, incomplete finality, contradictory
    /// block lookups, broken ancestry, a moving finalized tip, noncanonical refund bytes,
    /// early inclusion, ambiguous discovery, or invalid historical terminal state.
    pub async fn observe(
        &self,
        request: &ObserveNativeRefundRequest,
    ) -> Result<ObserveNativeRefundResult, BridgeRuntimeError> {
        let terms = self.validate_request(request)?;
        let expected = match request.target {
            NativeRefundObservationTarget::Exact {
                refund_transaction_id,
                ..
            } => Some(
                self.planner
                    .owned_native_refund(request, refund_transaction_id)
                    .await?,
            ),
            NativeRefundObservationTarget::StateOnly
            | NativeRefundObservationTarget::DiscoverByTerms { .. } => None,
        };
        let finalized_before = self
            .indexer
            .last_finalized_block_id()
            .await?
            .ok_or(BridgeRuntimeError::Unavailable)?;
        let tip_before = self.read_finalized_block(finalized_before).await?;
        let clock = ChainClock::new(
            Hex32::from_bytes(tip_before.header.hash.0),
            tip_before.header.block_id,
            tip_before.header.timestamp,
        );

        let (refund, expected_terminal) = match request.target {
            NativeRefundObservationTarget::StateOnly => {
                (NativeRefundObservation::NotRequested, None)
            }
            NativeRefundObservationTarget::Exact { window, .. } => {
                let expected = expected
                    .as_ref()
                    .ok_or(BridgeRuntimeError::InvalidObservation)?;
                let found = self
                    .scan_refund(request, window, Some(expected), &tip_before)
                    .await?;
                match found {
                    Some(found) => {
                        self.validate_refunded_state(terms, found.header.block_id)
                            .await?;
                        let facts = self.validate_refund(request, &found, Some(expected))?;
                        (
                            NativeRefundObservation::found(facts),
                            Some(EscrowState::Refunded),
                        )
                    }
                    None => (NativeRefundObservation::UnknownOrPending, None),
                }
            }
            NativeRefundObservationTarget::DiscoverByTerms { window } => {
                let found = self.scan_refund(request, window, None, &tip_before).await?;
                match found {
                    Some(found) => {
                        self.validate_refunded_state(terms, found.header.block_id)
                            .await?;
                        let facts = self.validate_refund(request, &found, None)?;
                        (
                            NativeRefundObservation::found(facts),
                            Some(EscrowState::Refunded),
                        )
                    }
                    None => (NativeRefundObservation::Absent, None),
                }
            }
        };

        let accounts = self
            .read_accounts(terms, finalized_before, expected_terminal)
            .await?;
        if accounts.metadata.status() == EscrowState::Refunded
            && clock.timestamp_ms < terms.refund_at_ms()
        {
            return Err(BridgeRuntimeError::InvalidObservation);
        }
        self.ensure_snapshot(
            finalized_before,
            &tip_before,
            matches!(&refund, NativeRefundObservation::Found(_)),
        )
        .await?;
        Ok(ObserveNativeRefundResult::new(
            request.context.clone(),
            clock,
            NativeEscrowAccountObservation::found(accounts),
            refund,
            clock,
        ))
    }

    fn validate_request<'a>(
        &self,
        request: &'a ObserveNativeRefundRequest,
    ) -> Result<&'a WitnessedNativeEscrowTerms, BridgeRuntimeError> {
        if request.runtime != self.runtime
            || request.context.sidecar_role != self.runtime.sidecar_role
            || request.runtime.compatibility != lez_bridge_protocol::RuntimeCompatibility::LeeV0_2_0
        {
            return Err(BridgeRuntimeError::Planner);
        }
        let terms = request
            .terms
            .witnessed()
            .ok_or(BridgeRuntimeError::Planner)?;
        let expected_signer = match request.target {
            NativeRefundObservationTarget::StateOnly
            | NativeRefundObservationTarget::Exact { .. } => {
                if terms.depositor() != self.runtime.sidecar_role {
                    return Err(BridgeRuntimeError::Planner);
                }
                terms.depositor_account_id()
            }
            NativeRefundObservationTarget::DiscoverByTerms { .. } => {
                if terms.claimant() != self.runtime.sidecar_role
                    || terms.depositor() == self.runtime.sidecar_role
                {
                    return Err(BridgeRuntimeError::Planner);
                }
                terms.claimant_account_id()
            }
        };
        if self.runtime.signer_account_id != expected_signer
            || self.runtime.escrow_program_id != request.runtime.escrow_program_id
        {
            return Err(BridgeRuntimeError::Planner);
        }
        let authority_key = PublicKey::try_new(*terms.aggregate_x_only_public_key().as_bytes())
            .map_err(|_| BridgeRuntimeError::InvalidObservation)?;
        if AccountId::from(&authority_key).into_value()
            != *terms.aggregate_authority_account_id().as_bytes()
        {
            return Err(BridgeRuntimeError::InvalidObservation);
        }
        Ok(terms)
    }

    async fn scan_refund(
        &self,
        request: &ObserveNativeRefundRequest,
        window: lez_bridge_protocol::DiscoveryWindow,
        expected: Option<&PreparedTransaction>,
        finalized_tip: &Block,
    ) -> Result<Option<FoundRefund>, BridgeRuntimeError> {
        let finalized_height = finalized_tip.header.block_id;
        let window_end = window
            .start_height()
            .checked_add(u64::from(window.max_blocks() - 1))
            .ok_or(BridgeRuntimeError::InvalidObservation)?;
        let covered_length = finalized_height
            .checked_sub(window.start_height())
            .and_then(|distance| distance.checked_add(1));
        if window_end > finalized_height
            || covered_length.is_none_or(|length| length > u64::from(MAX_DISCOVERY_BLOCKS))
        {
            return Err(BridgeRuntimeError::Unavailable);
        }

        let mut found = None;
        let mut previous_hash = None;
        for block_id in window.start_height()..=finalized_height {
            let block = self.read_finalized_block(block_id).await?;
            if block_id == finalized_height && block != *finalized_tip {
                return Err(BridgeRuntimeError::MovingTip);
            }
            if previous_hash.is_some_and(|hash| block.header.prev_block_hash != hash) {
                return Err(BridgeRuntimeError::InvalidObservation);
            }
            previous_hash = Some(block.header.hash);
            if block_id > window_end {
                continue;
            }
            for (transaction_index, transaction) in block.body.transactions.iter().enumerate() {
                let public = match request.target {
                    NativeRefundObservationTarget::Exact {
                        refund_transaction_id,
                        ..
                    } => {
                        if transaction.hash().0 != *refund_transaction_id.as_bytes() {
                            continue;
                        }
                        let IndexedTransaction::Public(public) = transaction else {
                            return Err(BridgeRuntimeError::InvalidObservation);
                        };
                        public
                    }
                    NativeRefundObservationTarget::DiscoverByTerms { .. } => {
                        let IndexedTransaction::Public(public) = transaction else {
                            continue;
                        };
                        if !self.matches_discovery_terms(request, public)? {
                            continue;
                        }
                        public
                    }
                    NativeRefundObservationTarget::StateOnly => unreachable!("state does not scan"),
                };
                if found
                    .replace(FoundRefund {
                        header: block.header.clone(),
                        transaction_index,
                        public: public.clone(),
                    })
                    .is_some()
                {
                    return Err(if expected.is_some() {
                        BridgeRuntimeError::InvalidObservation
                    } else {
                        BridgeRuntimeError::AmbiguousDiscovery
                    });
                }
            }
        }
        Ok(found)
    }

    fn matches_discovery_terms(
        &self,
        request: &ObserveNativeRefundRequest,
        indexed: &IndexedPublicTransaction,
    ) -> Result<bool, BridgeRuntimeError> {
        let terms = request
            .terms
            .witnessed()
            .ok_or(BridgeRuntimeError::Planner)?;
        let escrow_program = program_id_from_hex(self.runtime.escrow_program_id);
        if indexed.message.program_id.0 != escrow_program {
            return Ok(false);
        }
        let Ok(instruction) = risc0_zkvm::serde::from_slice::<ZecEscrowInstruction, u32>(
            &indexed.message.instruction_data,
        ) else {
            return Ok(false);
        };
        let ZecEscrowInstruction::RefundNative { swap_id } = instruction else {
            return Ok(false);
        };
        if swap_id != *terms.swap_id().as_bytes() {
            return Ok(false);
        }
        let expected_accounts = self.expected_accounts(terms);
        if indexed.message.account_ids.len() != expected_accounts.len()
            || !indexed
                .message
                .account_ids
                .iter()
                .zip(expected_accounts)
                .all(|(observed, expected)| observed.value == expected.into_value())
        {
            return Err(BridgeRuntimeError::ConflictingDiscovery);
        }
        Ok(true)
    }

    fn validate_refund(
        &self,
        request: &ObserveNativeRefundRequest,
        found: &FoundRefund,
        expected: Option<&PreparedTransaction>,
    ) -> Result<NativeRefundFoundFacts, BridgeRuntimeError> {
        let terms = request
            .terms
            .witnessed()
            .ok_or(BridgeRuntimeError::Planner)?;
        let expected_id = expected.map(|prepared| prepared.transaction_id);
        let transcript_error = if expected.is_some() {
            BridgeRuntimeError::InvalidObservation
        } else {
            BridgeRuntimeError::ConflictingDiscovery
        };
        if expected_id.is_some_and(|id| found.public.hash.0 != *id.as_bytes())
            || found.public.witness_set.proof.is_some()
            || !found
                .public
                .witness_set
                .signatures_and_public_keys
                .is_empty()
            || found.header.timestamp < terms.refund_at_ms()
        {
            return Err(BridgeRuntimeError::InvalidObservation);
        }
        let public = decode_indexed_public(&found.public)?;
        if public.hash() != found.public.hash.0 {
            return Err(BridgeRuntimeError::InvalidObservation);
        }
        LeeTransaction::Public(public.clone())
            .transaction_stateless_check()
            .map_err(|_| BridgeRuntimeError::InvalidObservation)?;
        self.validate_message(terms, public.message())
            .map_err(|_| transcript_error)?;
        let prepared = prepared_from_transaction(&public)?;
        if expected.is_some_and(|persisted| persisted != &prepared) {
            return Err(BridgeRuntimeError::InvalidObservation);
        }
        let ordered_account_ids = AccountIds::new(
            public
                .message()
                .account_ids
                .iter()
                .map(|account| Hex32::from_bytes(account.into_value()))
                .collect(),
        )
        .map_err(|_| BridgeRuntimeError::InvalidObservation)?;
        Ok(NativeRefundFoundFacts::new(
            ObservedTransactionFacts::new(
                prepared.transaction_id,
                prepared.exact_bytes,
                ChainPosition::new(
                    Hex32::from_bytes(found.header.hash.0),
                    found.header.block_id,
                    u32::try_from(found.transaction_index)
                        .map_err(|_| BridgeRuntimeError::InvalidObservation)?,
                ),
                AccountIds::new(Vec::new()).map_err(|_| BridgeRuntimeError::InvalidObservation)?,
                true,
            ),
            NativeRefundInstructionFacts::new(
                self.runtime.escrow_program_id,
                ordered_account_ids,
                terms.swap_id(),
            ),
        ))
    }

    fn validate_message(
        &self,
        terms: &WitnessedNativeEscrowTerms,
        message: &Message,
    ) -> Result<(), BridgeRuntimeError> {
        let expected_instruction = risc0_zkvm::serde::to_vec(&ZecEscrowInstruction::RefundNative {
            swap_id: *terms.swap_id().as_bytes(),
        })
        .map_err(|_| BridgeRuntimeError::InvalidObservation)?;
        let instruction =
            risc0_zkvm::serde::from_slice::<ZecEscrowInstruction, u32>(&message.instruction_data)
                .map_err(|_| BridgeRuntimeError::InvalidObservation)?;
        if message.program_id != program_id_from_hex(self.runtime.escrow_program_id)
            || message.account_ids != self.expected_accounts(terms)
            || !message.nonces.is_empty()
            || message.instruction_data != expected_instruction
            || !matches!(
                instruction,
                ZecEscrowInstruction::RefundNative { swap_id }
                    if swap_id == *terms.swap_id().as_bytes()
            )
        {
            return Err(BridgeRuntimeError::InvalidObservation);
        }
        Ok(())
    }

    fn expected_accounts(&self, terms: &WitnessedNativeEscrowTerms) -> [AccountId; 3] {
        let escrow_program = program_id_from_hex(self.runtime.escrow_program_id);
        [
            compute_metadata_pda(&escrow_program, terms.swap_id().as_bytes()),
            compute_custody_pda(&escrow_program, terms.swap_id().as_bytes()),
            AccountId::new(*terms.depositor_account_id().as_bytes()),
        ]
    }

    async fn validate_refunded_state(
        &self,
        terms: &WitnessedNativeEscrowTerms,
        block_id: u64,
    ) -> Result<(), BridgeRuntimeError> {
        self.read_accounts(terms, block_id, Some(EscrowState::Refunded))
            .await
            .map(|_| ())
    }

    async fn read_accounts(
        &self,
        terms: &WitnessedNativeEscrowTerms,
        block_id: u64,
        expected_status: Option<EscrowState>,
    ) -> Result<NativeEscrowAccountFacts, BridgeRuntimeError> {
        let escrow_program = program_id_from_hex(self.runtime.escrow_program_id);
        let transfer_program = program_id_from_hex(terms.authenticated_transfer_program_id());
        let metadata_id = compute_metadata_pda(&escrow_program, terms.swap_id().as_bytes());
        let custody_id = compute_custody_pda(&escrow_program, terms.swap_id().as_bytes());
        let metadata_account = self
            .indexer
            .account_at_block(metadata_id.into_value(), block_id)
            .await?
            .require_present()?;
        let custody_account = self
            .indexer
            .account_at_block(custody_id.into_value(), block_id)
            .await?
            .require_present()?;
        let metadata = EscrowMetadata::try_from_slice(metadata_account.data.0.as_ref())
            .map_err(|_| BridgeRuntimeError::InvalidObservation)?;
        let ClaimAuthority::AggregateWitness {
            x_only_public_key,
            account_id,
        } = metadata.claim_authority
        else {
            return Err(BridgeRuntimeError::InvalidObservation);
        };
        let (status, expected_balance) = match metadata.status {
            EscrowStatus::Funded => (EscrowState::Funded, terms.amount().as_u128()),
            EscrowStatus::Refunded => (EscrowState::Refunded, 0),
            _ => return Err(BridgeRuntimeError::InvalidObservation),
        };
        if expected_status.is_some_and(|expected| expected != status)
            || metadata_account.program_owner.0 != escrow_program
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
            || metadata.asset_program != transfer_program
            || metadata.custody_program != transfer_program
            || metadata.asset_definition != [0; 32]
            || metadata.amount != terms.amount().as_u128()
            || metadata.refund_at != terms.refund_at_ms()
            || custody_account.program_owner.0 != transfer_program
            || custody_account.balance != expected_balance
        {
            return Err(BridgeRuntimeError::InvalidObservation);
        }
        let metadata_id = Hex32::from_bytes(metadata_id.into_value());
        let custody_id = Hex32::from_bytes(custody_id.into_value());
        Ok(NativeEscrowAccountFacts::new_witnessed(
            WitnessedEscrowMetadataFacts::from_witnessed_native_terms(
                metadata_id,
                self.runtime.escrow_program_id,
                custody_id,
                terms,
                status,
            ),
            NativeCustodyFacts::new(
                custody_id,
                program_id_to_hex(custody_account.program_owner.0),
                custody_account.balance,
            ),
        ))
    }

    async fn read_finalized_block(&self, block_id: u64) -> Result<Block, BridgeRuntimeError> {
        let by_id = self
            .indexer
            .block_by_id(block_id)
            .await?
            .ok_or(BridgeRuntimeError::Unavailable)?;
        if by_id.header.block_id != block_id || by_id.bedrock_status != BedrockStatus::Finalized {
            return Err(BridgeRuntimeError::InvalidObservation);
        }
        let by_hash = self
            .indexer
            .block_by_hash(by_id.header.hash.0)
            .await?
            .ok_or(BridgeRuntimeError::Unavailable)?;
        if by_hash != by_id {
            return Err(BridgeRuntimeError::InvalidObservation);
        }
        Ok(by_id)
    }

    async fn ensure_snapshot(
        &self,
        finalized_before: u64,
        tip_before: &Block,
        terminal_refund_found: bool,
    ) -> Result<(), BridgeRuntimeError> {
        let finalized_after = self
            .indexer
            .last_finalized_block_id()
            .await?
            .ok_or(BridgeRuntimeError::Unavailable)?;
        if finalized_after < finalized_before {
            return Err(BridgeRuntimeError::MovingTip);
        }
        if finalized_after == finalized_before {
            let tip_after = self.read_finalized_block(finalized_after).await?;
            return if &tip_after == tip_before {
                Ok(())
            } else {
                Err(BridgeRuntimeError::MovingTip)
            };
        }
        if !terminal_refund_found {
            return Err(BridgeRuntimeError::MovingTip);
        }

        let descendant_count = finalized_after
            .checked_sub(finalized_before)
            .ok_or(BridgeRuntimeError::MovingTip)?;
        if descendant_count > u64::from(MAX_DISCOVERY_BLOCKS) {
            return Err(BridgeRuntimeError::Unavailable);
        }

        let pinned_again = self.read_finalized_block(finalized_before).await?;
        if &pinned_again != tip_before {
            return Err(BridgeRuntimeError::MovingTip);
        }
        let mut previous = pinned_again;
        for block_id in (finalized_before + 1)..=finalized_after {
            let descendant = self.read_finalized_block(block_id).await?;
            if descendant.header.prev_block_hash != previous.header.hash {
                return Err(BridgeRuntimeError::InvalidObservation);
            }
            previous = descendant;
        }

        let tip_again = self.read_finalized_block(finalized_after).await?;
        if tip_again != previous {
            return Err(BridgeRuntimeError::MovingTip);
        }
        let pinned_last = self.read_finalized_block(finalized_before).await?;
        if &pinned_last != tip_before {
            return Err(BridgeRuntimeError::MovingTip);
        }
        Ok(())
    }
}
