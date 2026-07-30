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
    AccountIds, AggregateBip340Signature, ChainClock, ChainPosition, ChainTip,
    ClassifyFinalizedWitnessedClaimResult, ClassifyFinalizedWitnessedFundingResult,
    ClassifyFinalizedWitnessedInitializationRequest,
    ClassifyFinalizedWitnessedInitializationResult, DiscoveryWindow, EscrowState,
    FinalizedBlockIdentity, FinalizedWitnessedClaimFacts, FinalizedWitnessedClaimObservationTarget,
    FinalizedWitnessedClaimScanOutcome, FinalizedWitnessedFundingFacts,
    FinalizedWitnessedFundingObservationTarget, FinalizedWitnessedFundingScanOutcome,
    FinalizedWitnessedInitializationFacts, Hex32, NativeCustodyFacts, NativeFundInstructionFacts,
    ObserveFinalizedWitnessedClaimRequest, ObserveFinalizedWitnessedClaimResult,
    ObserveFinalizedWitnessedFundingRequest, ObserveFinalizedWitnessedFundingResult,
    ObservedTransactionFacts, WitnessedClaimInstructionFacts, WitnessedEscrowMetadataFacts,
    WitnessedNativeInitializeInstructionFacts,
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
const INDEXER_MAX_CONCURRENT_REQUESTS: usize = 1;
const HISTORICAL_ACCOUNT_REQUEST_TIMEOUT: Duration = Duration::from_secs(90);
const HISTORICAL_ACCOUNT_MAX_CONCURRENT_REQUESTS: usize = 3;

/// Provenance-preserving historical account state from pinned indexer v0.2.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HistoricalAccount {
    /// The pinned state machine returned its canonical default for a missing account.
    Absent,
    /// The historical account existed with these exact indexed fields.
    Present(IndexedAccount),
}

impl HistoricalAccount {
    pub(crate) fn require_present(self) -> Result<IndexedAccount, BridgeRuntimeError> {
        match self {
            Self::Present(account) => Ok(account),
            Self::Absent => Err(BridgeRuntimeError::InvalidObservation),
        }
    }
}

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
    ) -> Result<HistoricalAccount, BridgeRuntimeError>;
}

pub(crate) struct StableFinalizedWindow {
    pub(crate) blocks: Vec<Block>,
    pub(crate) finalized_clock: ChainClock,
    pub(crate) finalized_tip: Block,
    pub(crate) requested_end: u64,
}

impl StableFinalizedWindow {
    pub(crate) fn block(&self, block_id: u64) -> Result<&Block, BridgeRuntimeError> {
        self.blocks
            .iter()
            .find(|block| block.header.block_id == block_id)
            .ok_or(BridgeRuntimeError::Unavailable)
    }

    pub(crate) fn requested_end_block(&self) -> Result<&Block, BridgeRuntimeError> {
        self.block(self.requested_end)
    }

    pub(crate) fn requested_end_clock(&self) -> Result<ChainClock, BridgeRuntimeError> {
        let block = self.requested_end_block()?;
        Ok(ChainClock::new(
            Hex32::from_bytes(block.header.hash.0),
            block.header.block_id,
            block.header.timestamp,
        ))
    }

    pub(crate) async fn confirm_requested_end(
        &self,
        indexer: &dyn FinalizedIndexerApi,
    ) -> Result<(), BridgeRuntimeError> {
        self.confirm_block(indexer, self.requested_end).await
    }

    pub(crate) async fn confirm_block(
        &self,
        indexer: &dyn FinalizedIndexerApi,
        block_id: u64,
    ) -> Result<(), BridgeRuntimeError> {
        let expected = self.block(block_id)?;
        let observed = read_finalized_block(indexer, block_id).await?;
        if &observed != expected {
            return Err(BridgeRuntimeError::MovingTip);
        }
        Ok(())
    }

    pub(crate) async fn confirm_pinned_snapshot(
        &self,
        indexer: &dyn FinalizedIndexerApi,
    ) -> Result<(), BridgeRuntimeError> {
        let finalized_after = indexer
            .last_finalized_block_id()
            .await?
            .ok_or(BridgeRuntimeError::Unavailable)?;
        if finalized_after < self.finalized_tip.header.block_id {
            return Err(BridgeRuntimeError::MovingTip);
        }
        self.confirm_block(indexer, self.finalized_tip.header.block_id)
            .await
    }
}

async fn read_stable_finalized_prefix(
    indexer: &dyn FinalizedIndexerApi,
    authorized_window: DiscoveryWindow,
) -> Result<(StableFinalizedWindow, DiscoveryWindow, bool), BridgeRuntimeError> {
    let finalized_before = indexer
        .last_finalized_block_id()
        .await?
        .ok_or(BridgeRuntimeError::Unavailable)?;
    let start_height = authorized_window.start_height();
    if finalized_before < start_height {
        return Err(BridgeRuntimeError::Unavailable);
    }
    let authorized_end = start_height
        .checked_add(u64::from(authorized_window.max_blocks() - 1))
        .ok_or(BridgeRuntimeError::InvalidObservation)?;
    let prefix_end = authorized_end.min(finalized_before);
    let prefix_blocks = prefix_end
        .checked_sub(start_height)
        .and_then(|distance| distance.checked_add(1))
        .ok_or(BridgeRuntimeError::InvalidObservation)?;
    let scanned_window = DiscoveryWindow::new(
        start_height,
        u32::try_from(prefix_blocks).map_err(|_| BridgeRuntimeError::Unavailable)?,
    )
    .map_err(|_| BridgeRuntimeError::InvalidObservation)?;
    let stable = read_fixed_finalized_window(indexer, scanned_window).await?;

    Ok((stable, scanned_window, prefix_end == authorized_end))
}

/// Reads one fail-closed finalized clock whose indexer chain is bound to the
/// runtime descriptor's exact genesis hash.
///
/// The finalized tip must remain unchanged for the complete sample. This
/// avoids authorizing from an internally stale bracket; the checked guest
/// remains the definitive deadline enforcement. The tip may advance after
/// return, so callers must not claim an atomic clock-and-send snapshot.
///
/// # Errors
///
/// Returns a typed bridge error when the indexer is unavailable, the expected
/// genesis or finalized block facts are invalid, or the finalized sample moves.
pub async fn read_genesis_bound_finalized_clock(
    indexer: &dyn FinalizedIndexerApi,
    expected_genesis_hash: Hex32,
) -> Result<ChainClock, BridgeRuntimeError> {
    if expected_genesis_hash == Hex32::from_bytes([0; 32]) {
        return Err(BridgeRuntimeError::InvalidObservation);
    }
    let finalized_before = indexer
        .last_finalized_block_id()
        .await?
        .ok_or(BridgeRuntimeError::Unavailable)?;
    if finalized_before < nssa::GENESIS_BLOCK_ID {
        return Err(BridgeRuntimeError::InvalidObservation);
    }

    let genesis_before = read_finalized_block(indexer, nssa::GENESIS_BLOCK_ID).await?;
    if genesis_before.header.hash.0 != *expected_genesis_hash.as_bytes() {
        return Err(BridgeRuntimeError::InvalidObservation);
    }
    let tip_before = read_finalized_block(indexer, finalized_before).await?;
    if tip_before.header.hash.0 == [0; 32] || tip_before.header.timestamp == 0 {
        return Err(BridgeRuntimeError::InvalidObservation);
    }

    let genesis_after = read_finalized_block(indexer, nssa::GENESIS_BLOCK_ID).await?;
    let tip_after = read_finalized_block(indexer, finalized_before).await?;
    if genesis_after != genesis_before || tip_after != tip_before {
        return Err(BridgeRuntimeError::MovingTip);
    }
    let finalized_after = indexer
        .last_finalized_block_id()
        .await?
        .ok_or(BridgeRuntimeError::Unavailable)?;
    if finalized_after != finalized_before {
        return Err(BridgeRuntimeError::MovingTip);
    }

    Ok(ChainClock::new(
        Hex32::from_bytes(tip_before.header.hash.0),
        tip_before.header.block_id,
        tip_before.header.timestamp,
    ))
}

pub(crate) async fn read_fixed_finalized_window(
    indexer: &dyn FinalizedIndexerApi,
    window: lez_bridge_protocol::DiscoveryWindow,
) -> Result<StableFinalizedWindow, BridgeRuntimeError> {
    let finalized_tip = indexer
        .last_finalized_block_id()
        .await?
        .ok_or(BridgeRuntimeError::Unavailable)?;
    let requested_end = window
        .start_height()
        .checked_add(u64::from(window.max_blocks() - 1))
        .ok_or(BridgeRuntimeError::InvalidObservation)?;
    if requested_end > finalized_tip {
        return Err(BridgeRuntimeError::Unavailable);
    }

    let capacity =
        usize::try_from(window.max_blocks()).map_err(|_| BridgeRuntimeError::Unavailable)?;
    let mut blocks = Vec::with_capacity(capacity);
    let mut previous_hash = None;
    for block_id in window.start_height()..=requested_end {
        let block = read_finalized_block(indexer, block_id).await?;
        if previous_hash.is_some_and(|hash| block.header.prev_block_hash != hash) {
            return Err(BridgeRuntimeError::InvalidObservation);
        }
        previous_hash = Some(block.header.hash);
        blocks.push(block);
    }
    let requested_end_block = blocks
        .last()
        .cloned()
        .ok_or(BridgeRuntimeError::Unavailable)?;
    let confirmed_end = read_finalized_block(indexer, requested_end).await?;
    if confirmed_end != requested_end_block {
        return Err(BridgeRuntimeError::MovingTip);
    }

    Ok(StableFinalizedWindow {
        finalized_clock: ChainClock::new(
            Hex32::from_bytes(requested_end_block.header.hash.0),
            requested_end_block.header.block_id,
            requested_end_block.header.timestamp,
        ),
        finalized_tip: requested_end_block,
        blocks,
        requested_end,
    })
}

async fn read_finalized_block(
    indexer: &dyn FinalizedIndexerApi,
    block_id: u64,
) -> Result<Block, BridgeRuntimeError> {
    let by_id = indexer
        .block_by_id(block_id)
        .await?
        .ok_or(BridgeRuntimeError::Unavailable)?;
    if by_id.header.block_id != block_id || by_id.bedrock_status != BedrockStatus::Finalized {
        return Err(BridgeRuntimeError::InvalidObservation);
    }
    let by_hash = indexer
        .block_by_hash(by_id.header.hash.0)
        .await?
        .ok_or(BridgeRuntimeError::Unavailable)?;
    if by_hash != by_id {
        return Err(BridgeRuntimeError::InvalidObservation);
    }
    Ok(by_id)
}

fn build_indexer_client(
    endpoint: &str,
    request_timeout: Duration,
    max_concurrent_requests: usize,
) -> Result<SequencerClient, crate::RuntimeBoundaryError> {
    SequencerClientBuilder::default()
        .max_request_size(MAX_INDEXER_REQUEST_BYTES)
        .max_response_size(MAX_INDEXER_RESPONSE_BYTES)
        .request_timeout(request_timeout)
        .max_concurrent_requests(max_concurrent_requests)
        .build(endpoint)
        .map_err(|_| crate::RuntimeBoundaryError::InvalidNodeEndpoint)
}

/// Direct, bounded, no-retry client for the pinned official v0.2 indexer RPC.
#[derive(Clone)]
pub struct OfficialIndexerRpc {
    client: SequencerClient,
    historical_account_client: SequencerClient,
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
        let client = build_indexer_client(
            endpoint,
            INDEXER_REQUEST_TIMEOUT,
            INDEXER_MAX_CONCURRENT_REQUESTS,
        )?;
        let historical_account_client = build_indexer_client(
            endpoint,
            HISTORICAL_ACCOUNT_REQUEST_TIMEOUT,
            HISTORICAL_ACCOUNT_MAX_CONCURRENT_REQUESTS,
        )?;
        Ok(Self {
            client,
            historical_account_client,
        })
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
    ) -> Result<HistoricalAccount, BridgeRuntimeError> {
        let account = self
            .historical_account_client
            .get_account_at_block(IndexedAccountId { value: account_id }, block_id)
            .await
            .map_err(|_| BridgeRuntimeError::Unavailable)?;
        Ok(
            if account.program_owner.0 == [0; 8]
                && account.balance == 0
                && account.data.0.is_empty()
                && account.nonce == 0
            {
                HistoricalAccount::Absent
            } else {
                HistoricalAccount::Present(account)
            },
        )
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

    /// Preserves the original found-only finalized observer contract.
    ///
    /// Definitive absence from [`Self::classify`] maps to the legacy
    /// `Unavailable` error rather than changing this established response shape.
    ///
    /// # Errors
    ///
    /// Returns every classifier failure and maps definitive absence to
    /// [`BridgeRuntimeError::Unavailable`].
    pub async fn observe(
        &self,
        request: &ObserveFinalizedWitnessedClaimRequest,
    ) -> Result<ObserveFinalizedWitnessedClaimResult, BridgeRuntimeError> {
        let classified = self.classify(request).await?;
        match classified.outcome {
            FinalizedWitnessedClaimScanOutcome::PresentExact { claim } => {
                Ok(ObserveFinalizedWitnessedClaimResult::new(
                    classified.context,
                    classified.finalized_tip,
                    *claim,
                ))
            }
            FinalizedWitnessedClaimScanOutcome::NotFound
            | FinalizedWitnessedClaimScanOutcome::Uncertain {} => {
                Err(BridgeRuntimeError::Unavailable)
            }
        }
    }

    /// Classifies the exact transaction in the stable finalized prefix of an authorized window.
    ///
    /// # Errors
    ///
    /// Fails closed on role/runtime/message/terms drift, unavailable finality, missing
    /// blocks, by-ID/by-hash disagreement, noncanonical transactions, duplicates,
    /// or movement of the pinned finalized prefix. A strict-prefix miss is
    /// `Uncertain`, never definitive absence. This method performs no submission.
    pub async fn classify(
        &self,
        request: &ObserveFinalizedWitnessedClaimRequest,
    ) -> Result<ClassifyFinalizedWitnessedClaimResult, BridgeRuntimeError> {
        self.validate_request(request)?;
        let (stable, scanned_window, window_complete) =
            read_stable_finalized_prefix(self.indexer.as_ref(), request.window).await?;

        let mut found = None;
        for block in &stable.blocks {
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

        let claim = if let Some((containing_header, transaction_index, public)) = found {
            Some(
                self.validate_claim(request, &containing_header, transaction_index, &public)
                    .await?,
            )
        } else {
            None
        };

        stable
            .confirm_pinned_snapshot(self.indexer.as_ref())
            .await?;
        let finalized_tip = ChainTip::new(
            stable.finalized_clock.block_hash,
            stable.finalized_clock.height,
        );
        Ok(if let Some(claim) = claim {
            ClassifyFinalizedWitnessedClaimResult::present_exact(
                request.context.clone(),
                finalized_tip,
                scanned_window,
                claim,
            )
        } else if window_complete {
            ClassifyFinalizedWitnessedClaimResult::not_found(
                request.context.clone(),
                finalized_tip,
                scanned_window,
            )
        } else {
            ClassifyFinalizedWitnessedClaimResult::uncertain(
                request.context.clone(),
                finalized_tip,
                scanned_window,
            )
        })
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

/// Fail-closed observer for one exact witnessed initialization in finalized ancestry.
pub struct FinalizedWitnessedInitializationObserver {
    runtime: lez_bridge_protocol::RuntimeDescriptor,
    indexer: Arc<dyn FinalizedIndexerApi>,
}

impl fmt::Debug for FinalizedWitnessedInitializationObserver {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FinalizedWitnessedInitializationObserver")
            .field("runtime", &self.runtime)
            .finish_non_exhaustive()
    }
}

impl FinalizedWitnessedInitializationObserver {
    /// Binds one immutable depositor/runtime identity to one read-only finalized source.
    pub fn new(
        runtime: lez_bridge_protocol::RuntimeDescriptor,
        indexer: Arc<dyn FinalizedIndexerApi>,
    ) -> Self {
        Self { runtime, indexer }
    }

    /// Classifies the caller-owned exact initialization in stable finalized ancestry.
    ///
    /// A finalized miss is deliberately `Uncertain`: the pinned v0.2 current
    /// lookup cannot distinguish pending presence from absence. The composed
    /// runtime performs that exact current lookup as an additional conflict and
    /// malformed-fact check before returning this result.
    ///
    /// # Errors
    ///
    /// Fails closed on actor/runtime drift, incomplete or moving finalized
    /// history, exact-byte substitution, signer/instruction/account drift,
    /// conflicting same-swap initialization, or invalid historical Empty state.
    pub async fn classify(
        &self,
        request: &ClassifyFinalizedWitnessedInitializationRequest,
    ) -> Result<ClassifyFinalizedWitnessedInitializationResult, BridgeRuntimeError> {
        self.validate_request(request)?;
        let (stable, scanned_window, _window_complete) =
            read_stable_finalized_prefix(self.indexer.as_ref(), request.window).await?;
        let mut found = None;
        for block in &stable.blocks {
            if block.header.block_id > stable.requested_end {
                continue;
            }
            for (transaction_index, transaction) in block.body.transactions.iter().enumerate() {
                if transaction.hash().0 == *request.initialization.transaction_id.as_bytes() {
                    let IndexedTransaction::Public(public) = transaction else {
                        return Err(BridgeRuntimeError::InvalidObservation);
                    };
                    let facts = self
                        .validate_initialization(request, &block.header, transaction_index, public)
                        .await?;
                    if found.replace(facts).is_some() {
                        return Err(BridgeRuntimeError::AmbiguousDiscovery);
                    }
                } else if let IndexedTransaction::Public(public) = transaction
                    && self.matches_same_swap_initialization(request, public)
                {
                    return Err(BridgeRuntimeError::ConflictingDiscovery);
                }
            }
        }
        stable
            .confirm_pinned_snapshot(self.indexer.as_ref())
            .await?;
        Ok(if let Some(initialization) = found {
            ClassifyFinalizedWitnessedInitializationResult::found(
                request.context.clone(),
                stable.finalized_clock,
                scanned_window,
                initialization,
            )
        } else {
            ClassifyFinalizedWitnessedInitializationResult::uncertain(
                request.context.clone(),
                stable.finalized_clock,
                scanned_window,
            )
        })
    }

    fn validate_request(
        &self,
        request: &ClassifyFinalizedWitnessedInitializationRequest,
    ) -> Result<(), BridgeRuntimeError> {
        if request.runtime != self.runtime
            || request.context.sidecar_role != self.runtime.sidecar_role
            || self.runtime.sidecar_role != request.terms.depositor()
            || self.runtime.signer_account_id != request.terms.depositor_account_id()
            || request.initialization.transaction_id == request.funding_transaction_id
            || request.initialization.exact_bytes.as_slice().is_empty()
        {
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

    fn matches_same_swap_initialization(
        &self,
        request: &ClassifyFinalizedWitnessedInitializationRequest,
        indexed: &IndexedPublicTransaction,
    ) -> bool {
        if indexed.message.program_id.0 != program_id_from_hex(self.runtime.escrow_program_id) {
            return false;
        }
        let Ok(instruction) = risc0_zkvm::serde::from_slice::<ZecEscrowInstruction, u32>(
            &indexed.message.instruction_data,
        ) else {
            return false;
        };
        matches!(
            instruction,
            ZecEscrowInstruction::InitializeNativeWitnessed { swap_id, .. }
                if swap_id == *request.terms.swap_id().as_bytes()
        )
    }

    async fn validate_initialization(
        &self,
        request: &ClassifyFinalizedWitnessedInitializationRequest,
        block: &BlockHeader,
        transaction_index: usize,
        indexed: &IndexedPublicTransaction,
    ) -> Result<FinalizedWitnessedInitializationFacts, BridgeRuntimeError> {
        if indexed.hash.0 != *request.initialization.transaction_id.as_bytes()
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
        self.validate_initialization_message(request, public.message())?;
        let [(signature, key)] = public.witness_set().signatures_and_public_keys() else {
            return Err(BridgeRuntimeError::InvalidObservation);
        };
        if AccountId::from(key).into_value() != *request.terms.depositor_account_id().as_bytes()
            || !signature.is_valid_for(&public.message().hash(), key)
        {
            return Err(BridgeRuntimeError::InvalidObservation);
        }
        let prepared = prepared_from_transaction(&public)?;
        if prepared != request.initialization {
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
        let (metadata, custody) = self.validate_empty_state(request, block.block_id).await?;
        Ok(FinalizedWitnessedInitializationFacts::new(
            transaction,
            WitnessedNativeInitializeInstructionFacts::new(
                self.runtime.escrow_program_id,
                ordered_account_ids,
                request.terms.clone(),
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

    fn validate_initialization_message(
        &self,
        request: &ClassifyFinalizedWitnessedInitializationRequest,
        message: &Message,
    ) -> Result<(), BridgeRuntimeError> {
        let escrow_program = program_id_from_hex(self.runtime.escrow_program_id);
        let swap_id = request.terms.swap_id();
        let expected_accounts = [
            compute_metadata_pda(&escrow_program, swap_id.as_bytes()),
            compute_custody_pda(&escrow_program, swap_id.as_bytes()),
            AccountId::new(*request.terms.depositor_account_id().as_bytes()),
            AccountId::new(*request.terms.claimant_account_id().as_bytes()),
            AccountId::new(*request.terms.aggregate_authority_account_id().as_bytes()),
        ];
        let instruction =
            risc0_zkvm::serde::from_slice::<ZecEscrowInstruction, u32>(&message.instruction_data)
                .map_err(|_| BridgeRuntimeError::InvalidObservation)?;
        let exact_instruction = matches!(
            instruction,
            ZecEscrowInstruction::InitializeNativeWitnessed {
                swap_id: observed_swap_id,
                terms_hash,
                aggregate_x_only_public_key,
                amount,
                refund_at,
                authenticated_transfer_program,
            } if observed_swap_id == *swap_id.as_bytes()
                && terms_hash == *request.terms.terms_hash().as_bytes()
                && aggregate_x_only_public_key
                    == *request.terms.aggregate_x_only_public_key().as_bytes()
                && amount == request.terms.amount().as_u128()
                && refund_at == request.terms.refund_at_ms()
                && authenticated_transfer_program
                    == program_id_from_hex(
                        request.terms.authenticated_transfer_program_id(),
                    )
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

    async fn validate_empty_state(
        &self,
        request: &ClassifyFinalizedWitnessedInitializationRequest,
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
            || metadata.status != EscrowStatus::Empty
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
                EscrowState::Empty,
            ),
            NativeCustodyFacts::new(
                custody_id,
                program_id_to_hex(custody_account.program_owner.0),
                custody_account.balance,
            ),
        ))
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

    /// Preserves the original found-only finalized funding observer contract.
    ///
    /// Affirmative absence from [`Self::classify`] maps to the legacy
    /// `Unavailable` error rather than changing the established response shape.
    ///
    /// # Errors
    ///
    /// Returns every classifier failure and maps affirmative absence to
    /// [`BridgeRuntimeError::Unavailable`].
    pub async fn observe(
        &self,
        request: &ObserveFinalizedWitnessedFundingRequest,
    ) -> Result<ObserveFinalizedWitnessedFundingResult, BridgeRuntimeError> {
        let classified = self.classify(request).await?;
        match classified.outcome {
            FinalizedWitnessedFundingScanOutcome::Found { funding } => {
                Ok(ObserveFinalizedWitnessedFundingResult::new(
                    classified.context,
                    ChainTip::new(
                        classified.finalized_clock.block_hash,
                        classified.finalized_clock.height,
                    ),
                    *funding,
                ))
            }
            FinalizedWitnessedFundingScanOutcome::Absent {}
            | FinalizedWitnessedFundingScanOutcome::Uncertain {} => {
                Err(BridgeRuntimeError::Unavailable)
            }
        }
    }

    /// Classifies one exact or terms-discovered funding effect in a finalized window.
    ///
    /// # Errors
    ///
    /// Fails closed on actor/runtime drift, incomplete finality, contradictory block
    /// lookups, noncanonical funding transactions, ambiguous discovery, invalid
    /// historical funded state, or movement of the finalized tip. This method never
    /// submits a transaction.
    pub async fn classify(
        &self,
        request: &ObserveFinalizedWitnessedFundingRequest,
    ) -> Result<ClassifyFinalizedWitnessedFundingResult, BridgeRuntimeError> {
        self.validate_request(request)?;
        let (stable, scanned_window, window_complete) =
            read_stable_finalized_prefix(self.indexer.as_ref(), request.window).await?;

        let mut found = None;
        for block in &stable.blocks {
            if block.header.block_id > stable.requested_end {
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

        let funding = if let Some((containing_header, transaction_index, public)) = found {
            Some(
                self.validate_funding(request, &containing_header, transaction_index, &public)
                    .await?,
            )
        } else {
            None
        };

        stable
            .confirm_pinned_snapshot(self.indexer.as_ref())
            .await?;

        Ok(if let Some(funding) = funding {
            ClassifyFinalizedWitnessedFundingResult::found(
                request.context.clone(),
                stable.finalized_clock,
                scanned_window,
                funding,
            )
        } else if window_complete {
            ClassifyFinalizedWitnessedFundingResult::absent(
                request.context.clone(),
                stable.finalized_clock,
                scanned_window,
            )
        } else {
            ClassifyFinalizedWitnessedFundingResult::uncertain(
                request.context.clone(),
                stable.finalized_clock,
                scanned_window,
            )
        })
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

pub(crate) fn decode_indexed_public(
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

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    use jsonrpsee::{RpcModule, server::ServerBuilder, types::ErrorObjectOwned};
    use tokio::sync::Notify;

    use super::*;

    const SLOW_HISTORICAL_ACCOUNT_DELAY: Duration = Duration::from_secs(11);
    const SCALED_FAST_REQUEST_TIMEOUT: Duration = Duration::from_millis(100);

    #[derive(Debug)]
    struct SlowHistoricalAccountRpc {
        entered: AtomicUsize,
        all_entered: Notify,
        begin_delay: tokio::sync::Semaphore,
        serialized_reconstruction: tokio::sync::Semaphore,
    }

    #[tokio::test]
    #[allow(
        clippy::too_many_lines,
        reason = "one production-budget and scaled-timeout RPC regression keeps serialized server behavior together"
    )]
    async fn historical_account_rpc_budget_covers_serialized_server_reconstruction() {
        assert_eq!(INDEXER_REQUEST_TIMEOUT, Duration::from_secs(10));
        assert_eq!(INDEXER_MAX_CONCURRENT_REQUESTS, 1);
        assert_eq!(HISTORICAL_ACCOUNT_REQUEST_TIMEOUT, Duration::from_secs(90));
        assert_eq!(HISTORICAL_ACCOUNT_MAX_CONCURRENT_REQUESTS, 3);
        let state = Arc::new(SlowHistoricalAccountRpc {
            entered: AtomicUsize::new(0),
            all_entered: Notify::new(),
            begin_delay: tokio::sync::Semaphore::new(0),
            serialized_reconstruction: tokio::sync::Semaphore::new(1),
        });
        let server = ServerBuilder::default()
            .build("127.0.0.1:0")
            .await
            .expect("mock indexer binds loopback");
        let address = server.local_addr().expect("mock indexer address");
        let mut module = RpcModule::new(Arc::clone(&state));
        module
            .register_async_method("getAccountAtBlock", |_params, state, _| async move {
                if (state.entered.fetch_add(1, Ordering::SeqCst) + 1)
                    % HISTORICAL_ACCOUNT_MAX_CONCURRENT_REQUESTS
                    == 0
                {
                    state.all_entered.notify_one();
                }
                state
                    .begin_delay
                    .acquire()
                    .await
                    .expect("test delay barrier remains open")
                    .forget();
                let _serialized_reconstruction = state
                    .serialized_reconstruction
                    .acquire()
                    .await
                    .expect("serialized reconstruction remains open");
                tokio::time::sleep(SLOW_HISTORICAL_ACCOUNT_DELAY).await;
                Ok::<_, ErrorObjectOwned>(IndexedAccount {
                    program_owner: indexer_service_protocol::ProgramId([0; 8]),
                    balance: 0,
                    data: indexer_service_protocol::Data(Vec::new()),
                    nonce: 0,
                })
            })
            .expect("historical account method");
        let handle = server.start(module);
        let endpoint = format!("http://{address}");
        let production =
            OfficialIndexerRpc::connect_local(&endpoint).expect("loopback indexer client");
        let indexer = production.historical_account_client.clone();
        let read = tokio::spawn(async move {
            tokio::try_join!(
                indexer.get_account_at_block(IndexedAccountId { value: [7; 32] }, 181),
                indexer.get_account_at_block(IndexedAccountId { value: [8; 32] }, 181),
                indexer.get_account_at_block(IndexedAccountId { value: [9; 32] }, 181),
            )
        });
        state.all_entered.notified().await;
        assert_eq!(
            state.entered.load(Ordering::SeqCst),
            HISTORICAL_ACCOUNT_MAX_CONCURRENT_REQUESTS
        );
        state
            .begin_delay
            .add_permits(HISTORICAL_ACCOUNT_MAX_CONCURRENT_REQUESTS);

        let accounts = read
            .await
            .expect("historical account task")
            .expect("three slow historical accounts succeed concurrently");
        assert_eq!(accounts.0.program_owner.0, [0; 8]);
        assert_eq!(accounts.1.program_owner.0, [0; 8]);
        assert_eq!(accounts.2.program_owner.0, [0; 8]);

        let fast = build_indexer_client(
            &endpoint,
            SCALED_FAST_REQUEST_TIMEOUT,
            HISTORICAL_ACCOUNT_MAX_CONCURRENT_REQUESTS,
        )
        .expect("scaled fast client");
        let fast_read = tokio::spawn(async move {
            tokio::try_join!(
                fast.get_account_at_block(IndexedAccountId { value: [17; 32] }, 181),
                fast.get_account_at_block(IndexedAccountId { value: [18; 32] }, 181),
                fast.get_account_at_block(IndexedAccountId { value: [19; 32] }, 181),
            )
        });
        state.all_entered.notified().await;
        assert_eq!(state.entered.load(Ordering::SeqCst), 6);
        state
            .begin_delay
            .add_permits(HISTORICAL_ACCOUNT_MAX_CONCURRENT_REQUESTS);
        assert!(matches!(
            fast_read.await.expect("scaled fast account task"),
            Err(jsonrpsee::core::ClientError::RequestTimeout)
        ));
        handle.stop().expect("mock indexer stops");
    }
}
