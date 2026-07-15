use std::{fmt, sync::Arc, time::Duration};

use async_trait::async_trait;
use borsh::{BorshDeserialize as _, to_vec};
use common::transaction::LeeTransaction;
use indexer_service_protocol::{
    Account as IndexedAccount, AccountId as IndexedAccountId, BedrockStatus, Block, BlockHeader,
    HashType as IndexedHash, PublicTransaction as IndexedPublicTransaction,
    Transaction as IndexedTransaction,
};
use indexer_service_rpc::RpcClient as _;
use lez_bridge_protocol::{
    AccountIds, AggregateBip340Signature, ChainPosition, ChainTip, EscrowState,
    FinalizedBlockIdentity, FinalizedWitnessedClaimFacts, FinalizedWitnessedClaimObservationTarget,
    FinalizedWitnessedFundingFacts, FinalizedWitnessedFundingObservationTarget, Hex32,
    MAX_DISCOVERY_BLOCKS, NativeCustodyFacts, NativeFundInstructionFacts,
    ObserveFinalizedWitnessedClaimRequest, ObserveFinalizedWitnessedClaimResult,
    ObserveFinalizedWitnessedFundingRequest, ObserveFinalizedWitnessedFundingResult,
    ObservedTransactionFacts, WitnessedClaimInstructionFacts, WitnessedEscrowMetadataFacts,
};
use lez_zec_escrow_v02::{ClaimAuthority, EscrowMetadata, EscrowStatus};
use nssa::{
    AccountId, PublicKey, PublicTransaction, Signature,
    public_transaction::{Message, WitnessSet},
};
use sequencer_service_rpc::{SequencerClient, SequencerClientBuilder};

use crate::{
    BridgeRuntimeError, ZecEscrowInstruction, compute_custody_pda, compute_metadata_pda,
    prepared_from_transaction, program_id_from_hex, program_id_to_hex,
    validate_loopback_http_endpoint,
};

const MAX_INDEXER_REQUEST_BYTES: u32 = 2_800_000;
const MAX_INDEXER_RESPONSE_BYTES: u32 = 8 * 1024 * 1024;
const INDEXER_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// Narrow read-only official indexer boundary used by finalized claim observation.
#[async_trait]
pub trait FinalizedIndexerApi: Send + Sync + fmt::Debug {
    /// Returns the official indexer's latest finalized numeric block ID.
    async fn last_finalized_block_id(&self) -> Result<Option<u64>, BridgeRuntimeError>;

    /// Reads one exact block by official numeric ID.
    async fn block_by_id(&self, block_id: u64) -> Result<Option<Block>, BridgeRuntimeError>;

    /// Reads one exact block independently by its hash.
    async fn block_by_hash(
        &self,
        block_hash: [u8; 32],
    ) -> Result<Option<Block>, BridgeRuntimeError>;

    /// Reads one exact account from the historical state at a numeric block ID.
    async fn account_at_block(
        &self,
        account_id: [u8; 32],
        block_id: u64,
    ) -> Result<IndexedAccount, BridgeRuntimeError>;
}

/// Direct, bounded, no-retry client for the pinned official v0.2 indexer RPC.
#[derive(Clone)]
pub struct OfficialIndexerRpc {
    client: SequencerClient,
}

impl fmt::Debug for OfficialIndexerRpc {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OfficialIndexerRpc")
            .finish_non_exhaustive()
    }
}

impl OfficialIndexerRpc {
    /// Connects to an explicit literal-loopback indexer endpoint without I/O or retries.
    ///
    /// # Errors
    ///
    /// Rejects non-loopback, non-HTTP, or otherwise invalid endpoint configuration.
    pub fn connect_local(endpoint: &str) -> Result<Self, crate::RuntimeBoundaryError> {
        validate_loopback_http_endpoint(endpoint)?;
        Self::connect(endpoint)
    }

    /// Connects to the exact allowlisted official public LEZ node origin.
    ///
    /// # Errors
    ///
    /// Rejects every endpoint outside the pinned official-public origin allowlist.
    pub fn connect_official_public(endpoint: &str) -> Result<Self, crate::RuntimeBoundaryError> {
        crate::OfficialNodeRpc::validate_official_public_endpoint(endpoint)?;
        Self::connect(endpoint)
    }

    fn connect(endpoint: &str) -> Result<Self, crate::RuntimeBoundaryError> {
        let client = SequencerClientBuilder::default()
            .max_request_size(MAX_INDEXER_REQUEST_BYTES)
            .max_response_size(MAX_INDEXER_RESPONSE_BYTES)
            .request_timeout(INDEXER_REQUEST_TIMEOUT)
            .max_concurrent_requests(1)
            .build(endpoint)
            .map_err(|_| crate::RuntimeBoundaryError::InvalidNodeEndpoint)?;
        Ok(Self { client })
    }
}

#[async_trait]
impl FinalizedIndexerApi for OfficialIndexerRpc {
    async fn last_finalized_block_id(&self) -> Result<Option<u64>, BridgeRuntimeError> {
        self.client
            .get_last_finalized_block_id()
            .await
            .map_err(|_| BridgeRuntimeError::Unavailable)
    }

    async fn block_by_id(&self, block_id: u64) -> Result<Option<Block>, BridgeRuntimeError> {
        self.client
            .get_block_by_id(block_id)
            .await
            .map_err(|_| BridgeRuntimeError::Unavailable)
    }

    async fn block_by_hash(
        &self,
        block_hash: [u8; 32],
    ) -> Result<Option<Block>, BridgeRuntimeError> {
        self.client
            .get_block_by_hash(IndexedHash(block_hash))
            .await
            .map_err(|_| BridgeRuntimeError::Unavailable)
    }

    async fn account_at_block(
        &self,
        account_id: [u8; 32],
        block_id: u64,
    ) -> Result<IndexedAccount, BridgeRuntimeError> {
        self.client
            .get_account_at_block(IndexedAccountId { value: account_id }, block_id)
            .await
            .map_err(|_| BridgeRuntimeError::Unavailable)
    }
}

/// Fail-closed observer for one exact witnessed claim in one stable finalized window.
pub struct FinalizedWitnessedClaimObserver {
    runtime: lez_bridge_protocol::RuntimeDescriptor,
    indexer: Arc<dyn FinalizedIndexerApi>,
}

impl fmt::Debug for FinalizedWitnessedClaimObserver {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FinalizedWitnessedClaimObserver")
            .field("runtime", &self.runtime)
            .finish_non_exhaustive()
    }
}

impl FinalizedWitnessedClaimObserver {
    /// Binds one immutable actor/runtime identity to one read-only indexer source.
    pub fn new(
        runtime: lez_bridge_protocol::RuntimeDescriptor,
        indexer: Arc<dyn FinalizedIndexerApi>,
    ) -> Self {
        Self { runtime, indexer }
    }

    /// Observes the exact transaction once in a fully covered, stable finalized window.
    ///
    /// # Errors
    ///
    /// Fails closed on role/runtime/message/terms drift, incomplete finality, missing
    /// blocks, by-ID/by-hash disagreement, noncanonical transactions, duplicates,
    /// or movement of the finalized tip. This method performs no submission.
    pub async fn observe(
        &self,
        request: &ObserveFinalizedWitnessedClaimRequest,
    ) -> Result<ObserveFinalizedWitnessedClaimResult, BridgeRuntimeError> {
        self.validate_request(request)?;
        let finalized_before = self
            .indexer
            .last_finalized_block_id()
            .await?
            .ok_or(BridgeRuntimeError::Unavailable)?;
        let window_end = request
            .window
            .start_height()
            .checked_add(u64::from(request.window.max_blocks() - 1))
            .ok_or(BridgeRuntimeError::InvalidObservation)?;
        if window_end > finalized_before
            || finalized_before
                .checked_sub(request.window.start_height())
                .and_then(|distance| distance.checked_add(1))
                .is_none_or(|length| length > u64::from(MAX_DISCOVERY_BLOCKS))
        {
            return Err(BridgeRuntimeError::Unavailable);
        }
        let finalized_tip_before = self.read_finalized_block(finalized_before).await?;

        let mut found = None;
        let mut previous_hash = None;
        for block_id in request.window.start_height()..=finalized_before {
            let block = self.read_finalized_block(block_id).await?;
            if block_id == finalized_before && block != finalized_tip_before {
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
                    FinalizedWitnessedClaimObservationTarget::Exact {
                        claim_transaction_id,
                    } => {
                        if transaction.hash().0 != *claim_transaction_id.as_bytes() {
                            continue;
                        }
                        let IndexedTransaction::Public(public) = transaction else {
                            return Err(BridgeRuntimeError::InvalidObservation);
                        };
                        public
                    }
                    FinalizedWitnessedClaimObservationTarget::DiscoverByTerms => {
                        let IndexedTransaction::Public(public) = transaction else {
                            continue;
                        };
                        if !self.matches_discovery_terms(request, public)? {
                            continue;
                        }
                        public
                    }
                };
                if found
                    .replace((block.header.clone(), transaction_index, public.clone()))
                    .is_some()
                {
                    return Err(BridgeRuntimeError::AmbiguousDiscovery);
                }
            }
        }

        let (containing_header, transaction_index, public) =
            found.ok_or(BridgeRuntimeError::Unavailable)?;
        let claim = self
            .validate_claim(request, &containing_header, transaction_index, &public)
            .await?;

        let finalized_after = self
            .indexer
            .last_finalized_block_id()
            .await?
            .ok_or(BridgeRuntimeError::Unavailable)?;
        if finalized_after != finalized_before {
            return Err(BridgeRuntimeError::MovingTip);
        }
        let tip_block = self.read_finalized_block(finalized_after).await?;
        if tip_block != finalized_tip_before {
            return Err(BridgeRuntimeError::MovingTip);
        }
        Ok(ObserveFinalizedWitnessedClaimResult::new(
            request.context.clone(),
            ChainTip::new(
                Hex32::from_bytes(tip_block.header.hash.0),
                tip_block.header.block_id,
            ),
            claim,
        ))
    }

    fn validate_request(
        &self,
        request: &ObserveFinalizedWitnessedClaimRequest,
    ) -> Result<(), BridgeRuntimeError> {
        if request.runtime != self.runtime
            || request.context.sidecar_role != self.runtime.sidecar_role
        {
            return Err(BridgeRuntimeError::Planner);
        }
        let expected_signer = if self.runtime.sidecar_role == request.terms.depositor() {
            request.terms.depositor_account_id()
        } else if self.runtime.sidecar_role == request.terms.claimant() {
            request.terms.claimant_account_id()
        } else {
            return Err(BridgeRuntimeError::Planner);
        };
        if self.runtime.signer_account_id != expected_signer {
            return Err(BridgeRuntimeError::Planner);
        }
        let authority_key =
            PublicKey::try_new(*request.terms.aggregate_x_only_public_key().as_bytes())
                .map_err(|_| BridgeRuntimeError::InvalidObservation)?;
        if AccountId::from(&authority_key).into_value()
            != *request.terms.aggregate_authority_account_id().as_bytes()
        {
            return Err(BridgeRuntimeError::InvalidObservation);
        }
        let message = Message::try_from_slice(request.claim.exact_message_bytes.as_slice())
            .map_err(|_| BridgeRuntimeError::InvalidObservation)?;
        if to_vec(&message).map_err(|_| BridgeRuntimeError::InvalidObservation)?
            != request.claim.exact_message_bytes.as_slice()
            || message.hash() != *request.claim.message_hash.as_bytes()
        {
            return Err(BridgeRuntimeError::InvalidObservation);
        }
        self.validate_message(request, &message)
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

    fn matches_discovery_terms(
        &self,
        request: &ObserveFinalizedWitnessedClaimRequest,
        indexed: &IndexedPublicTransaction,
    ) -> Result<bool, BridgeRuntimeError> {
        let escrow_program = program_id_from_hex(self.runtime.escrow_program_id);
        if indexed.message.program_id.0 != escrow_program {
            return Ok(false);
        }
        let swap_id = request.terms.swap_id();
        let expected_accounts = [
            compute_metadata_pda(&escrow_program, swap_id.as_bytes()).into_value(),
            compute_custody_pda(&escrow_program, swap_id.as_bytes()).into_value(),
            *request.terms.claimant_account_id().as_bytes(),
            *request.terms.aggregate_authority_account_id().as_bytes(),
        ];
        if indexed.message.account_ids.len() != expected_accounts.len()
            || !indexed
                .message
                .account_ids
                .iter()
                .zip(expected_accounts)
                .all(|(observed, expected)| observed.value == expected)
        {
            return Ok(false);
        }
        let instruction = risc0_zkvm::serde::from_slice::<ZecEscrowInstruction, u32>(
            &indexed.message.instruction_data,
        )
        .map_err(|_| BridgeRuntimeError::ConflictingDiscovery)?;
        match instruction {
            ZecEscrowInstruction::ClaimNativeWitnessed { swap_id: observed }
                if observed == *swap_id.as_bytes() =>
            {
                Ok(true)
            }
            _ => Err(BridgeRuntimeError::ConflictingDiscovery),
        }
    }

    async fn validate_claim(
        &self,
        request: &ObserveFinalizedWitnessedClaimRequest,
        block: &BlockHeader,
        transaction_index: usize,
        indexed: &IndexedPublicTransaction,
    ) -> Result<FinalizedWitnessedClaimFacts, BridgeRuntimeError> {
        let expected_transaction_id = match request.target {
            FinalizedWitnessedClaimObservationTarget::Exact {
                claim_transaction_id,
            } => Some(claim_transaction_id),
            FinalizedWitnessedClaimObservationTarget::DiscoverByTerms => None,
        };
        let transcript_error = if expected_transaction_id.is_some() {
            BridgeRuntimeError::InvalidObservation
        } else {
            BridgeRuntimeError::ConflictingDiscovery
        };
        if expected_transaction_id.is_some_and(|expected| indexed.hash.0 != *expected.as_bytes())
            || indexed.witness_set.proof.is_some()
            || indexed.witness_set.signatures_and_public_keys.len() != 1
        {
            return Err(BridgeRuntimeError::InvalidObservation);
        }
        let public = decode_indexed_public(indexed)?;
        if public.hash() != indexed.hash.0 {
            return Err(BridgeRuntimeError::InvalidObservation);
        }
        LeeTransaction::Public(public.clone())
            .transaction_stateless_check()
            .map_err(|_| BridgeRuntimeError::InvalidObservation)?;
        let exact_message =
            to_vec(public.message()).map_err(|_| BridgeRuntimeError::InvalidObservation)?;
        if exact_message != request.claim.exact_message_bytes.as_slice()
            || public.message().hash() != *request.claim.message_hash.as_bytes()
        {
            return Err(transcript_error);
        }
        self.validate_message(request, public.message())
            .map_err(|_| transcript_error)?;
        let [(signature, key)] = public.witness_set().signatures_and_public_keys() else {
            return Err(BridgeRuntimeError::InvalidObservation);
        };
        if key.value() != request.terms.aggregate_x_only_public_key().as_bytes()
            || AccountId::from(key).into_value()
                != *request.terms.aggregate_authority_account_id().as_bytes()
            || !signature.is_valid_for(request.claim.message_hash.as_bytes(), key)
        {
            return Err(transcript_error);
        }
        let prepared = prepared_from_transaction(&public)?;
        if expected_transaction_id.is_some_and(|expected| prepared.transaction_id != expected) {
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
        let transaction = ObservedTransactionFacts::new(
            prepared.transaction_id,
            prepared.exact_bytes,
            ChainPosition::new(
                Hex32::from_bytes(block.hash.0),
                block.block_id,
                u32::try_from(transaction_index)
                    .map_err(|_| BridgeRuntimeError::InvalidObservation)?,
            ),
            AccountIds::new(vec![request.terms.aggregate_authority_account_id()])
                .map_err(|_| BridgeRuntimeError::InvalidObservation)?,
            true,
        );
        let (metadata, custody) = self
            .validate_terminal_state(request, block.block_id)
            .await?;
        Ok(FinalizedWitnessedClaimFacts::new(
            transaction,
            WitnessedClaimInstructionFacts::new(
                self.runtime.escrow_program_id,
                ordered_account_ids,
                request.terms.swap_id(),
                request.terms.claimant_account_id(),
                request.terms.aggregate_authority_account_id(),
                request.claim.clone(),
            ),
            AggregateBip340Signature::from_bytes(signature.value),
            FinalizedBlockIdentity::new(
                block.block_id,
                Hex32::from_bytes(block.hash.0),
                block.timestamp,
            ),
            metadata,
            custody,
        ))
    }

    async fn validate_terminal_state(
        &self,
        request: &ObserveFinalizedWitnessedClaimRequest,
        block_id: u64,
    ) -> Result<(WitnessedEscrowMetadataFacts, NativeCustodyFacts), BridgeRuntimeError> {
        let escrow_program = program_id_from_hex(self.runtime.escrow_program_id);
        let transfer_program =
            program_id_from_hex(request.terms.authenticated_transfer_program_id());
        let swap_id = request.terms.swap_id();
        let metadata_id = compute_metadata_pda(&escrow_program, swap_id.as_bytes());
        let custody_id = compute_custody_pda(&escrow_program, swap_id.as_bytes());
        let metadata_account = self
            .indexer
            .account_at_block(metadata_id.into_value(), block_id)
            .await?;
        let custody_account = self
            .indexer
            .account_at_block(custody_id.into_value(), block_id)
            .await?;
        let metadata = EscrowMetadata::try_from_slice(metadata_account.data.0.as_ref())
            .map_err(|_| BridgeRuntimeError::InvalidObservation)?;
        let ClaimAuthority::AggregateWitness {
            x_only_public_key,
            account_id,
        } = metadata.claim_authority
        else {
            return Err(BridgeRuntimeError::InvalidObservation);
        };
        if metadata_account.program_owner.0 != escrow_program
            || metadata.version != 2
            || metadata.swap_id != *swap_id.as_bytes()
            || metadata.terms_hash != *request.terms.terms_hash().as_bytes()
            || x_only_public_key != *request.terms.aggregate_x_only_public_key().as_bytes()
            || account_id.into_value() != *request.terms.aggregate_authority_account_id().as_bytes()
            || metadata.depositor.into_value() != *request.terms.depositor_account_id().as_bytes()
            || metadata.depositor_asset != metadata.depositor
            || metadata.claimant.into_value() != *request.terms.claimant_account_id().as_bytes()
            || metadata.claimant_asset != metadata.claimant
            || metadata.custody != custody_id
            || metadata.asset_program != transfer_program
            || metadata.custody_program != transfer_program
            || metadata.asset_definition != [0; 32]
            || metadata.amount != request.terms.amount().as_u128()
            || metadata.refund_at != request.terms.refund_at_ms()
            || metadata.status != EscrowStatus::Claimed
            || custody_account.program_owner.0 != transfer_program
            || custody_account.balance != 0
        {
            return Err(BridgeRuntimeError::InvalidObservation);
        }
        let metadata_id = Hex32::from_bytes(metadata_id.into_value());
        let custody_id = Hex32::from_bytes(custody_id.into_value());
        Ok((
            WitnessedEscrowMetadataFacts::from_witnessed_native_terms(
                metadata_id,
                self.runtime.escrow_program_id,
                custody_id,
                &request.terms,
                EscrowState::Claimed,
            ),
            NativeCustodyFacts::new(
                custody_id,
                program_id_to_hex(custody_account.program_owner.0),
                custody_account.balance,
            ),
        ))
    }

    fn validate_message(
        &self,
        request: &ObserveFinalizedWitnessedClaimRequest,
        message: &Message,
    ) -> Result<(), BridgeRuntimeError> {
        let escrow_program = program_id_from_hex(self.runtime.escrow_program_id);
        let swap_id = request.terms.swap_id();
        let expected_accounts = [
            compute_metadata_pda(&escrow_program, swap_id.as_bytes()),
            compute_custody_pda(&escrow_program, swap_id.as_bytes()),
            AccountId::new(*request.terms.claimant_account_id().as_bytes()),
            AccountId::new(*request.terms.aggregate_authority_account_id().as_bytes()),
        ];
        let instruction =
            risc0_zkvm::serde::from_slice::<ZecEscrowInstruction, u32>(&message.instruction_data)
                .map_err(|_| BridgeRuntimeError::InvalidObservation)?;
        let exact_instruction = matches!(
            instruction,
            ZecEscrowInstruction::ClaimNativeWitnessed { swap_id: observed }
                if observed == *swap_id.as_bytes()
        );
        if message.program_id != escrow_program
            || message.account_ids != expected_accounts
            || message.nonces.len() != 1
            || !exact_instruction
        {
            return Err(BridgeRuntimeError::InvalidObservation);
        }
        Ok(())
    }
}

/// Fail-closed observer for one witnessed funding effect in one stable finalized window.
pub struct FinalizedWitnessedFundingObserver {
    runtime: lez_bridge_protocol::RuntimeDescriptor,
    indexer: Arc<dyn FinalizedIndexerApi>,
}

impl fmt::Debug for FinalizedWitnessedFundingObserver {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FinalizedWitnessedFundingObserver")
            .field("runtime", &self.runtime)
            .finish_non_exhaustive()
    }
}

impl FinalizedWitnessedFundingObserver {
    /// Binds one immutable actor/runtime identity to one read-only indexer source.
    pub fn new(
        runtime: lez_bridge_protocol::RuntimeDescriptor,
        indexer: Arc<dyn FinalizedIndexerApi>,
    ) -> Self {
        Self { runtime, indexer }
    }

    /// Observes one exact or terms-discovered funding effect in a finalized window.
    ///
    /// # Errors
    ///
    /// Fails closed on actor/runtime drift, incomplete finality, contradictory block
    /// lookups, noncanonical funding transactions, ambiguous discovery, invalid
    /// historical funded state, or movement of the finalized tip. This method never
    /// submits a transaction.
    pub async fn observe(
        &self,
        request: &ObserveFinalizedWitnessedFundingRequest,
    ) -> Result<ObserveFinalizedWitnessedFundingResult, BridgeRuntimeError> {
        self.validate_request(request)?;
        let finalized_before = self
            .indexer
            .last_finalized_block_id()
            .await?
            .ok_or(BridgeRuntimeError::Unavailable)?;
        let window_end = request
            .window
            .start_height()
            .checked_add(u64::from(request.window.max_blocks() - 1))
            .ok_or(BridgeRuntimeError::InvalidObservation)?;
        if window_end > finalized_before
            || finalized_before
                .checked_sub(request.window.start_height())
                .and_then(|distance| distance.checked_add(1))
                .is_none_or(|length| length > u64::from(MAX_DISCOVERY_BLOCKS))
        {
            return Err(BridgeRuntimeError::Unavailable);
        }
        let finalized_tip_before = self.read_finalized_block(finalized_before).await?;

        let mut found = None;
        let mut previous_hash = None;
        for block_id in request.window.start_height()..=finalized_before {
            let block = self.read_finalized_block(block_id).await?;
            if block_id == finalized_before && block != finalized_tip_before {
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
                    FinalizedWitnessedFundingObservationTarget::Exact {
                        funding_transaction_id,
                    } => {
                        if transaction.hash().0 != *funding_transaction_id.as_bytes() {
                            continue;
                        }
                        let IndexedTransaction::Public(public) = transaction else {
                            return Err(BridgeRuntimeError::InvalidObservation);
                        };
                        public
                    }
                    FinalizedWitnessedFundingObservationTarget::DiscoverByTerms => {
                        let IndexedTransaction::Public(public) = transaction else {
                            continue;
                        };
                        if !self.matches_discovery_terms(request, public)? {
                            continue;
                        }
                        public
                    }
                };
                if found
                    .replace((block.header.clone(), transaction_index, public.clone()))
                    .is_some()
                {
                    return Err(BridgeRuntimeError::AmbiguousDiscovery);
                }
            }
        }

        let (containing_header, transaction_index, public) =
            found.ok_or(BridgeRuntimeError::Unavailable)?;
        let funding = self
            .validate_funding(request, &containing_header, transaction_index, &public)
            .await?;

        let finalized_after = self
            .indexer
            .last_finalized_block_id()
            .await?
            .ok_or(BridgeRuntimeError::Unavailable)?;
        if finalized_after != finalized_before {
            return Err(BridgeRuntimeError::MovingTip);
        }
        let tip_block = self.read_finalized_block(finalized_after).await?;
        if tip_block != finalized_tip_before {
            return Err(BridgeRuntimeError::MovingTip);
        }
        Ok(ObserveFinalizedWitnessedFundingResult::new(
            request.context.clone(),
            ChainTip::new(
                Hex32::from_bytes(tip_block.header.hash.0),
                tip_block.header.block_id,
            ),
            funding,
        ))
    }

    fn validate_request(
        &self,
        request: &ObserveFinalizedWitnessedFundingRequest,
    ) -> Result<(), BridgeRuntimeError> {
        if request.runtime != self.runtime
            || request.context.sidecar_role != self.runtime.sidecar_role
        {
            return Err(BridgeRuntimeError::Planner);
        }
        let expected_signer = if self.runtime.sidecar_role == request.terms.depositor() {
            request.terms.depositor_account_id()
        } else if self.runtime.sidecar_role == request.terms.claimant() {
            request.terms.claimant_account_id()
        } else {
            return Err(BridgeRuntimeError::Planner);
        };
        if self.runtime.signer_account_id != expected_signer {
            return Err(BridgeRuntimeError::Planner);
        }
        let authority_key =
            PublicKey::try_new(*request.terms.aggregate_x_only_public_key().as_bytes())
                .map_err(|_| BridgeRuntimeError::InvalidObservation)?;
        if AccountId::from(&authority_key).into_value()
            != *request.terms.aggregate_authority_account_id().as_bytes()
        {
            return Err(BridgeRuntimeError::InvalidObservation);
        }
        Ok(())
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

    fn matches_discovery_terms(
        &self,
        request: &ObserveFinalizedWitnessedFundingRequest,
        indexed: &IndexedPublicTransaction,
    ) -> Result<bool, BridgeRuntimeError> {
        let escrow_program = program_id_from_hex(self.runtime.escrow_program_id);
        if indexed.message.program_id.0 != escrow_program {
            return Ok(false);
        }
        let Ok(instruction) = risc0_zkvm::serde::from_slice::<ZecEscrowInstruction, u32>(
            &indexed.message.instruction_data,
        ) else {
            return Ok(false);
        };
        let ZecEscrowInstruction::FundNative { swap_id } = instruction else {
            return Ok(false);
        };
        if swap_id != *request.terms.swap_id().as_bytes() {
            return Ok(false);
        }
        let expected_accounts = self.expected_accounts(request);
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

    async fn validate_funding(
        &self,
        request: &ObserveFinalizedWitnessedFundingRequest,
        block: &BlockHeader,
        transaction_index: usize,
        indexed: &IndexedPublicTransaction,
    ) -> Result<FinalizedWitnessedFundingFacts, BridgeRuntimeError> {
        let expected_transaction_id = match request.target {
            FinalizedWitnessedFundingObservationTarget::Exact {
                funding_transaction_id,
            } => Some(funding_transaction_id),
            FinalizedWitnessedFundingObservationTarget::DiscoverByTerms => None,
        };
        let terms_error = if expected_transaction_id.is_some() {
            BridgeRuntimeError::InvalidObservation
        } else {
            BridgeRuntimeError::ConflictingDiscovery
        };
        if expected_transaction_id.is_some_and(|expected| indexed.hash.0 != *expected.as_bytes())
            || indexed.witness_set.proof.is_some()
            || indexed.witness_set.signatures_and_public_keys.len() != 1
        {
            return Err(BridgeRuntimeError::InvalidObservation);
        }
        let public = decode_indexed_public(indexed)?;
        if public.hash() != indexed.hash.0 {
            return Err(BridgeRuntimeError::InvalidObservation);
        }
        LeeTransaction::Public(public.clone())
            .transaction_stateless_check()
            .map_err(|_| BridgeRuntimeError::InvalidObservation)?;
        self.validate_message(request, public.message())
            .map_err(|_| terms_error)?;
        let [(signature, key)] = public.witness_set().signatures_and_public_keys() else {
            return Err(BridgeRuntimeError::InvalidObservation);
        };
        if AccountId::from(key).into_value() != *request.terms.depositor_account_id().as_bytes()
            || !signature.is_valid_for(&public.message().hash(), key)
        {
            return Err(terms_error);
        }
        let prepared = prepared_from_transaction(&public)?;
        if expected_transaction_id.is_some_and(|expected| prepared.transaction_id != expected) {
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
        let transaction = ObservedTransactionFacts::new(
            prepared.transaction_id,
            prepared.exact_bytes,
            ChainPosition::new(
                Hex32::from_bytes(block.hash.0),
                block.block_id,
                u32::try_from(transaction_index)
                    .map_err(|_| BridgeRuntimeError::InvalidObservation)?,
            ),
            AccountIds::new(vec![request.terms.depositor_account_id()])
                .map_err(|_| BridgeRuntimeError::InvalidObservation)?,
            true,
        );
        let (metadata, custody) = self.validate_funded_state(request, block.block_id).await?;
        Ok(FinalizedWitnessedFundingFacts::new(
            transaction,
            NativeFundInstructionFacts::new(
                self.runtime.escrow_program_id,
                ordered_account_ids,
                request.terms.swap_id(),
            ),
            FinalizedBlockIdentity::new(
                block.block_id,
                Hex32::from_bytes(block.hash.0),
                block.timestamp,
            ),
            metadata,
            custody,
        ))
    }

    async fn validate_funded_state(
        &self,
        request: &ObserveFinalizedWitnessedFundingRequest,
        block_id: u64,
    ) -> Result<(WitnessedEscrowMetadataFacts, NativeCustodyFacts), BridgeRuntimeError> {
        let escrow_program = program_id_from_hex(self.runtime.escrow_program_id);
        let transfer_program =
            program_id_from_hex(request.terms.authenticated_transfer_program_id());
        let swap_id = request.terms.swap_id();
        let metadata_id = compute_metadata_pda(&escrow_program, swap_id.as_bytes());
        let custody_id = compute_custody_pda(&escrow_program, swap_id.as_bytes());
        let metadata_account = self
            .indexer
            .account_at_block(metadata_id.into_value(), block_id)
            .await?;
        let custody_account = self
            .indexer
            .account_at_block(custody_id.into_value(), block_id)
            .await?;
        let metadata = EscrowMetadata::try_from_slice(metadata_account.data.0.as_ref())
            .map_err(|_| BridgeRuntimeError::InvalidObservation)?;
        let ClaimAuthority::AggregateWitness {
            x_only_public_key,
            account_id,
        } = metadata.claim_authority
        else {
            return Err(BridgeRuntimeError::InvalidObservation);
        };
        if metadata_account.program_owner.0 != escrow_program
            || metadata.version != 2
            || metadata.swap_id != *swap_id.as_bytes()
            || metadata.terms_hash != *request.terms.terms_hash().as_bytes()
            || x_only_public_key != *request.terms.aggregate_x_only_public_key().as_bytes()
            || account_id.into_value() != *request.terms.aggregate_authority_account_id().as_bytes()
            || metadata.depositor.into_value() != *request.terms.depositor_account_id().as_bytes()
            || metadata.depositor_asset != metadata.depositor
            || metadata.claimant.into_value() != *request.terms.claimant_account_id().as_bytes()
            || metadata.claimant_asset != metadata.claimant
            || metadata.custody != custody_id
            || metadata.asset_program != transfer_program
            || metadata.custody_program != transfer_program
            || metadata.asset_definition != [0; 32]
            || metadata.amount != request.terms.amount().as_u128()
            || metadata.refund_at != request.terms.refund_at_ms()
            || metadata.status != EscrowStatus::Funded
            || custody_account.program_owner.0 != transfer_program
            || custody_account.balance != request.terms.amount().as_u128()
        {
            return Err(BridgeRuntimeError::InvalidObservation);
        }
        let metadata_id = Hex32::from_bytes(metadata_id.into_value());
        let custody_id = Hex32::from_bytes(custody_id.into_value());
        Ok((
            WitnessedEscrowMetadataFacts::from_witnessed_native_terms(
                metadata_id,
                self.runtime.escrow_program_id,
                custody_id,
                &request.terms,
                EscrowState::Funded,
            ),
            NativeCustodyFacts::new(
                custody_id,
                program_id_to_hex(custody_account.program_owner.0),
                custody_account.balance,
            ),
        ))
    }

    fn validate_message(
        &self,
        request: &ObserveFinalizedWitnessedFundingRequest,
        message: &Message,
    ) -> Result<(), BridgeRuntimeError> {
        let expected_instruction = risc0_zkvm::serde::to_vec(&ZecEscrowInstruction::FundNative {
            swap_id: *request.terms.swap_id().as_bytes(),
        })
        .map_err(|_| BridgeRuntimeError::InvalidObservation)?;
        let instruction =
            risc0_zkvm::serde::from_slice::<ZecEscrowInstruction, u32>(&message.instruction_data)
                .map_err(|_| BridgeRuntimeError::InvalidObservation)?;
        let exact_instruction = matches!(
            instruction,
            ZecEscrowInstruction::FundNative { swap_id }
                if swap_id == *request.terms.swap_id().as_bytes()
        );
        if message.program_id != program_id_from_hex(self.runtime.escrow_program_id)
            || message.account_ids != self.expected_accounts(request)
            || message.nonces.len() != 1
            || message.instruction_data != expected_instruction
            || !exact_instruction
        {
            return Err(BridgeRuntimeError::InvalidObservation);
        }
        Ok(())
    }

    fn expected_accounts(
        &self,
        request: &ObserveFinalizedWitnessedFundingRequest,
    ) -> [AccountId; 3] {
        let escrow_program = program_id_from_hex(self.runtime.escrow_program_id);
        let swap_id = request.terms.swap_id();
        [
            compute_metadata_pda(&escrow_program, swap_id.as_bytes()),
            compute_custody_pda(&escrow_program, swap_id.as_bytes()),
            AccountId::new(*request.terms.depositor_account_id().as_bytes()),
        ]
    }
}

fn decode_indexed_public(
    indexed: &IndexedPublicTransaction,
) -> Result<PublicTransaction, BridgeRuntimeError> {
    let message = Message::new_preserialized(
        indexed.message.program_id.0,
        indexed
            .message
            .account_ids
            .iter()
            .map(|account| AccountId::new(account.value))
            .collect(),
        indexed
            .message
            .nonces
            .iter()
            .copied()
            .map(Into::into)
            .collect(),
        indexed.message.instruction_data.clone(),
    );
    let witnesses = indexed
        .witness_set
        .signatures_and_public_keys
        .iter()
        .map(|(signature, key)| {
            Ok((
                Signature { value: signature.0 },
                PublicKey::try_new(key.0).map_err(|_| BridgeRuntimeError::InvalidObservation)?,
            ))
        })
        .collect::<Result<Vec<_>, BridgeRuntimeError>>()?;
    Ok(PublicTransaction::new(
        message,
        WitnessSet::from_raw_parts(witnesses),
    ))
}
