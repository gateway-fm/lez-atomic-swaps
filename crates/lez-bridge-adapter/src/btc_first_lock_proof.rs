//! Finalized-history plus fresh-current proof of a BTC pair's LEZ first lock.

use async_trait::async_trait;
use lez_bridge_client::{BridgeClient, BridgeClientError, FinalizedWitnessedFundingPresence};
use lez_bridge_protocol::{
    ChainClock, ChainPosition, ChainTip, DiscoveryWindow, EscrowObservationTarget, EscrowState,
    Hex32, MessageContext, NativeCustodyFacts, ObserveFinalizedWitnessedFundingRequest,
    ObserveWitnessedEscrowRequest, ObserveWitnessedEscrowResult, Participant as BridgeParticipant,
    ProtocolValueError, RequestId, RuntimeCompatibility, WitnessedEscrowMetadataFacts,
    WitnessedFundingFoundFacts, WitnessedFundingObservation, WitnessedInitializationFoundFacts,
    WitnessedInitializationObservation, WitnessedNativeEscrowTerms,
};
use lez_btc_swap_sdk::{BtcAgreementV1, LezFirstLockEvidenceV1, PreparedLezFundingV1};
use lez_swap_core::{Participant, SwapDirection};
use thiserror::Error;

use crate::{LezBridgeAdapter, encode_hex32};

/// Read-only finalized and current witnessed-escrow operations needed by the BTC actor.
///
/// This deliberately excludes every preparation and submission capability. The
/// finalized read occurs first; the current pair read is always the last chain
/// operation so callers receive a genuinely fresh current-state gate.
#[async_trait]
pub trait LezBridgeBtcFirstLockProofTransport: Send + Sync {
    /// Concrete transport or evidence-validation failure.
    type Error: std::error::Error + Send + Sync + 'static;

    /// Discovers the unique witnessed funding effect in finalized ancestry.
    async fn classify_finalized_witnessed_funding(
        &self,
        request: ObserveFinalizedWitnessedFundingRequest,
    ) -> Result<FinalizedWitnessedFundingPresence, Self::Error>;

    /// Discovers the complete witnessed initialization/funding pair at one stable current tip.
    async fn observe_witnessed_escrow(
        &self,
        request: ObserveWitnessedEscrowRequest,
    ) -> Result<ObserveWitnessedEscrowResult, Self::Error>;
}

#[async_trait]
impl LezBridgeBtcFirstLockProofTransport for BridgeClient {
    type Error = BridgeClientError;

    async fn classify_finalized_witnessed_funding(
        &self,
        request: ObserveFinalizedWitnessedFundingRequest,
    ) -> Result<FinalizedWitnessedFundingPresence, Self::Error> {
        BridgeClient::classify_finalized_witnessed_funding(self, request).await
    }

    async fn observe_witnessed_escrow(
        &self,
        request: ObserveWitnessedEscrowRequest,
    ) -> Result<ObserveWitnessedEscrowResult, Self::Error> {
        BridgeClient::observe_witnessed_escrow(self, request).await
    }
}

/// Complete SDK material proven both finalized historically and funded now.
#[derive(Clone, Debug, Eq, PartialEq)]
#[must_use]
pub struct BtcLezFirstLockProofV1 {
    prepared: PreparedLezFundingV1,
    evidence: LezFirstLockEvidenceV1,
    finalized_clock: ChainClock,
    current_tip: ChainTip,
}

impl BtcLezFirstLockProofV1 {
    /// Exact initialization/funding plan reconstructed from canonical chain bytes.
    pub const fn prepared(&self) -> &PreparedLezFundingV1 {
        &self.prepared
    }

    /// Finalized exact first-lock evidence accepted by the BTC SDK.
    pub const fn evidence(&self) -> &LezFirstLockEvidenceV1 {
        &self.evidence
    }

    /// Stable finalized clock that covered the historical funding scan.
    pub const fn finalized_clock(&self) -> ChainClock {
        self.finalized_clock
    }

    /// Stable current tip bracketing the final complete-pair read.
    pub const fn current_tip(&self) -> ChainTip {
        self.current_tip
    }

    /// Splits the proof into SDK-owned prepared material and evidence.
    pub fn into_sdk_parts(self) -> (PreparedLezFundingV1, LezFirstLockEvidenceV1) {
        (self.prepared, self.evidence)
    }
}

/// Failure to prove the maker-observed LEZ taker first lock.
#[derive(Debug, Error)]
pub enum BtcLezFirstLockProofError<E: std::error::Error + 'static> {
    /// This product direction has a Bitcoin taker first lock.
    #[error("agreement does not select a LEZ taker first lock")]
    WrongDirection,
    /// Only the LEZ claimant Maker consumes this proof before its second lock.
    #[error("local participant is not the LEZ first-lock claimant Maker")]
    WrongLocalRole,
    /// Finalized and current reads require distinct durable operation identities.
    #[error("LEZ first-lock proof request identities must be distinct")]
    DuplicateRequestId,

    /// The selected sidecar is not the pinned LEZ v0.2 runtime.
    #[error("LEZ first-lock proof runtime is incompatible")]
    IncompatibleRuntime,
    /// Runtime channel or genesis differs from the signed agreement.
    #[error("LEZ first-lock proof chain identity differs from agreement")]
    ChainIdentityMismatch,
    /// Runtime escrow deployment differs from the signed agreement.
    #[error("LEZ first-lock proof program differs from agreement")]
    EscrowProgramMismatch,
    /// Runtime signer is not the agreement-bound Maker claimant.
    #[error("LEZ first-lock proof signer differs from agreement")]
    SignerAccountMismatch,
    /// Agreement fields could not form exact witnessed bridge terms.
    #[error("LEZ first-lock proof terms are invalid")]
    Protocol(#[source] ProtocolValueError),
    /// One of the two bounded read-only bridge operations failed.
    #[error("LEZ first-lock proof transport is unavailable")]
    Transport(#[source] E),
    /// The finalized scan did not contain the unique funding transaction.
    #[error("finalized funding is unavailable for LEZ first-lock proof")]
    FinalizedFundingUnavailable,
    /// A response did not echo the exact caller-owned context or bounded window.
    #[error("LEZ first-lock proof response context or window differs")]
    ResponseEnvelopeMismatch,
    /// The bounded scan was not completely covered by the reported tip.
    #[error("LEZ first-lock proof window is not completely covered")]
    IncompleteWindow,
    /// Finalized funding facts differ from the runtime, terms, accounts, or custody.
    #[error("finalized funding differs from the signed LEZ first lock")]
    FinalizedFundingMismatch,
    /// The current canonical tip moved while the complete pair was read.
    #[error("LEZ first-lock current tip changed during observation")]
    UnstableCurrentTip,
    /// The current read did not return both initialization and funding.
    #[error("complete current pair is unavailable for LEZ first-lock proof")]
    CurrentPairUnavailable,
    /// Current transactions or decoded instructions differ from the signed agreement.
    #[error("current LEZ first-lock pair differs from agreement")]
    CurrentPairMismatch,
    /// Initialization does not strictly precede funding in the current chain.
    #[error("current LEZ first-lock pair is not chronological")]
    PairOrderMismatch,
    /// Current metadata/custody does not prove the exact complete funded escrow.
    #[error("current funded escrow differs from the signed LEZ first lock")]
    CurrentFundedEscrowMismatch,
    /// The current funding identity/bytes/position differ from finalized facts.
    #[error("current finalized funding differs from the current pair")]
    FinalizedCurrentCrossBindingMismatch,
    /// Canonical facts could not form strict BTC SDK material.
    #[error("canonical LEZ first-lock facts cannot form BTC SDK evidence")]
    InvalidSdkMaterial,
}

impl<T> LezBridgeAdapter<T>
where
    T: LezBridgeBtcFirstLockProofTransport,
{
    /// Proves the LEZ taker first lock for a Maker's BTC-pair second-lock decision.
    ///
    /// The method first discovers and independently validates finalized funding.
    /// It then performs a separate current complete-pair discovery as its final
    /// chain operation. Initialization and funding are decoded and checked
    /// against the agreement, ordered strictly, and the current funding bytes
    /// are cross-bound to the finalized facts before SDK material is returned.
    /// No preparation or submission capability is present on this boundary.
    ///
    /// # Errors
    ///
    /// Fails before transport for direction, role, runtime, signer, or terms
    /// drift. Fails closed on missing finality, moving current tip, incomplete
    /// windows, malformed instructions, account/custody drift, pair reordering,
    /// or any finalized/current funding substitution.
    pub async fn prove_btc_lez_first_lock(
        &self,
        agreement: &BtcAgreementV1,
        finalized_request_id: RequestId,
        current_request_id: RequestId,
        window: DiscoveryWindow,
    ) -> Result<BtcLezFirstLockProofV1, BtcLezFirstLockProofError<T::Error>> {
        self.validate_btc_lez_first_lock_proof_authority(agreement)?;
        validate_distinct_request_ids(&finalized_request_id, &current_request_id)?;
        let terms = btc_witnessed_terms(agreement).map_err(BtcLezFirstLockProofError::Protocol)?;
        let finalized_context = MessageContext::new(
            self.run_id.clone(),
            finalized_request_id,
            BridgeParticipant::Maker,
        );
        let finalized_request = ObserveFinalizedWitnessedFundingRequest::discover_by_terms(
            finalized_context.clone(),
            self.runtime.clone(),
            terms.clone(),
            window,
        );
        let finalized = self
            .transport
            .classify_finalized_witnessed_funding(finalized_request)
            .await
            .map_err(BtcLezFirstLockProofError::Transport)?;
        let (response_context, finalized_clock, scanned_window, finalized_funding) = match finalized
        {
            FinalizedWitnessedFundingPresence::Found {
                context,
                finalized_clock,
                scanned_window,
                funding,
            } => (context, finalized_clock, scanned_window, funding),
            FinalizedWitnessedFundingPresence::Absent { .. } => {
                return Err(BtcLezFirstLockProofError::FinalizedFundingUnavailable);
            }
        };
        validate_finalized_envelope(
            &finalized_context,
            &response_context,
            window,
            scanned_window,
            finalized_clock,
        )?;
        validate_finalized_funding(
            agreement,
            &self.runtime,
            &terms,
            window,
            finalized_clock,
            finalized_funding.as_ref(),
        )?;

        let current_context = MessageContext::new(
            self.run_id.clone(),
            current_request_id,
            BridgeParticipant::Maker,
        );
        let current = self
            .transport
            .observe_witnessed_escrow(ObserveWitnessedEscrowRequest::new(
                current_context.clone(),
                self.runtime.clone(),
                terms.clone(),
                EscrowObservationTarget::DiscoverByTerms { window },
            ))
            .await
            .map_err(BtcLezFirstLockProofError::Transport)?;
        if current.context != current_context {
            return Err(BtcLezFirstLockProofError::ResponseEnvelopeMismatch);
        }
        if current.tip_before != current.tip_after {
            return Err(BtcLezFirstLockProofError::UnstableCurrentTip);
        }
        require_window_covered(window, current.tip_after.height)?;
        let (
            WitnessedInitializationObservation::Found(initialization),
            WitnessedFundingObservation::Found(funding),
        ) = (&current.initialization, &current.funding)
        else {
            return Err(BtcLezFirstLockProofError::CurrentPairUnavailable);
        };
        validate_current_pair(
            agreement,
            &self.runtime,
            &terms,
            window,
            current.tip_after,
            initialization,
            funding,
        )?;
        if funding.transaction.transaction_id != finalized_funding.transaction.transaction_id
            || funding.transaction.exact_bytes != finalized_funding.transaction.exact_bytes
            || funding.transaction.position != finalized_funding.transaction.position
        {
            return Err(BtcLezFirstLockProofError::FinalizedCurrentCrossBindingMismatch);
        }

        build_sdk_proof(
            agreement,
            finalized_clock,
            current.tip_after,
            initialization,
            funding,
        )
    }

    fn validate_btc_lez_first_lock_proof_authority(
        &self,
        agreement: &BtcAgreementV1,
    ) -> Result<(), BtcLezFirstLockProofError<T::Error>> {
        if agreement.direction() != SwapDirection::TakerSellsLez {
            return Err(BtcLezFirstLockProofError::WrongDirection);
        }
        if self.local_participant != Participant::Maker
            || agreement.lez_claimant() != Participant::Maker
            || agreement.lez_depositor() != Participant::Taker
        {
            return Err(BtcLezFirstLockProofError::WrongLocalRole);
        }
        let signed = agreement.lez_terms();
        if self.runtime.compatibility != RuntimeCompatibility::LeeV0_2_0 {
            return Err(BtcLezFirstLockProofError::IncompatibleRuntime);
        }
        if self.runtime.channel_id.as_bytes() != signed.channel_id()
            || self.runtime.genesis_block_hash.as_bytes() != signed.genesis_block_hash()
        {
            return Err(BtcLezFirstLockProofError::ChainIdentityMismatch);
        }
        if self.runtime.escrow_program_id.as_bytes() != signed.escrow_program_id() {
            return Err(BtcLezFirstLockProofError::EscrowProgramMismatch);
        }
        if self.runtime.sidecar_role != BridgeParticipant::Maker
            || self.runtime.signer_account_id.as_bytes()
                != agreement
                    .participant(Participant::Maker)
                    .lez_owner_account()
        {
            return Err(BtcLezFirstLockProofError::SignerAccountMismatch);
        }
        Ok(())
    }
}

fn validate_distinct_request_ids<E: std::error::Error + 'static>(
    finalized: &RequestId,
    current: &RequestId,
) -> Result<(), BtcLezFirstLockProofError<E>> {
    if finalized == current {
        return Err(BtcLezFirstLockProofError::DuplicateRequestId);
    }
    Ok(())
}

fn build_sdk_proof<E: std::error::Error + 'static>(
    agreement: &BtcAgreementV1,
    finalized_clock: ChainClock,
    current_tip: ChainTip,
    initialization: &WitnessedInitializationFoundFacts,
    funding: &WitnessedFundingFoundFacts,
) -> Result<BtcLezFirstLockProofV1, BtcLezFirstLockProofError<E>> {
    let initialization_id = encode_hex32(initialization.transaction.transaction_id.as_bytes());
    let funding_id = encode_hex32(funding.transaction.transaction_id.as_bytes());
    let initialization_bytes = initialization.transaction.exact_bytes.as_slice().to_vec();
    let funding_bytes = funding.transaction.exact_bytes.as_slice().to_vec();
    let prepared = PreparedLezFundingV1::new(
        initialization_id.clone(),
        initialization_bytes.clone(),
        funding_id.clone(),
        funding_bytes.clone(),
    )
    .map_err(|_| BtcLezFirstLockProofError::InvalidSdkMaterial)?;
    let evidence = LezFirstLockEvidenceV1::new(
        *agreement.lez_terms().genesis_block_hash(),
        initialization_id,
        initialization_bytes,
        funding_id,
        funding_bytes,
        *agreement.lez_terms().metadata_account(),
        *agreement.lez_terms().custody_account(),
        agreement.lez_terms().amount(),
        true,
    )
    .map_err(|_| BtcLezFirstLockProofError::InvalidSdkMaterial)?;
    Ok(BtcLezFirstLockProofV1 {
        prepared,
        evidence,
        finalized_clock,
        current_tip,
    })
}
fn validate_finalized_envelope<E: std::error::Error + 'static>(
    expected_context: &MessageContext,
    actual_context: &MessageContext,
    expected_window: DiscoveryWindow,
    scanned_window: DiscoveryWindow,
    finalized_clock: ChainClock,
) -> Result<(), BtcLezFirstLockProofError<E>> {
    if actual_context != expected_context || scanned_window != expected_window {
        return Err(BtcLezFirstLockProofError::ResponseEnvelopeMismatch);
    }
    if finalized_clock.timestamp_ms == 0 {
        return Err(BtcLezFirstLockProofError::FinalizedFundingMismatch);
    }
    require_window_covered(expected_window, finalized_clock.height)
}

fn require_window_covered<E: std::error::Error + 'static>(
    window: DiscoveryWindow,
    tip_height: u64,
) -> Result<(), BtcLezFirstLockProofError<E>> {
    let end = window
        .start_height()
        .checked_add(u64::from(window.max_blocks() - 1))
        .ok_or(BtcLezFirstLockProofError::IncompleteWindow)?;
    if end > tip_height {
        return Err(BtcLezFirstLockProofError::IncompleteWindow);
    }
    Ok(())
}

fn validate_finalized_funding<E: std::error::Error + 'static>(
    agreement: &BtcAgreementV1,
    runtime: &lez_bridge_protocol::RuntimeDescriptor,
    terms: &WitnessedNativeEscrowTerms,
    window: DiscoveryWindow,
    finalized_clock: ChainClock,
    funding: &lez_bridge_protocol::FinalizedWitnessedFundingFacts,
) -> Result<(), BtcLezFirstLockProofError<E>> {
    let expected_metadata = expected_metadata(runtime, agreement, terms);
    let expected_custody = expected_custody(agreement, terms);
    let block = funding.containing_block;
    let position = funding.transaction.position;
    if !position_in_window(position, window, finalized_clock.height)
        || position.height != block.block_id
        || position.block_hash != block.block_hash
        || (block.block_id == finalized_clock.height
            && block.block_hash != finalized_clock.block_hash)
        || block.timestamp_ms == 0
        || !valid_funding_transaction_and_instruction(
            runtime,
            agreement,
            terms,
            &funding.transaction,
            &funding.instruction,
        )
        || funding.metadata != expected_metadata
        || funding.custody != expected_custody
        || funding.metadata.account_id.as_bytes() != agreement.lez_terms().metadata_account()
        || funding.custody.account_id.as_bytes() != agreement.lez_terms().custody_account()
    {
        return Err(BtcLezFirstLockProofError::FinalizedFundingMismatch);
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn validate_current_pair<E: std::error::Error + 'static>(
    agreement: &BtcAgreementV1,
    runtime: &lez_bridge_protocol::RuntimeDescriptor,
    terms: &WitnessedNativeEscrowTerms,
    window: DiscoveryWindow,
    current_tip: ChainTip,
    initialization: &WitnessedInitializationFoundFacts,
    funding: &WitnessedFundingFoundFacts,
) -> Result<(), BtcLezFirstLockProofError<E>> {
    if !position_in_window(
        initialization.transaction.position,
        window,
        current_tip.height,
    ) || !position_in_window(funding.transaction.position, window, current_tip.height)
        || (initialization.transaction.position.height == current_tip.height
            && initialization.transaction.position.block_hash != current_tip.block_hash)
        || (funding.transaction.position.height == current_tip.height
            && funding.transaction.position.block_hash != current_tip.block_hash)
        || !valid_initialization(runtime, agreement, terms, initialization)
        || !valid_funding_transaction_and_instruction(
            runtime,
            agreement,
            terms,
            &funding.transaction,
            &funding.instruction,
        )
    {
        return Err(BtcLezFirstLockProofError::CurrentPairMismatch);
    }
    if position_key(initialization.transaction.position)
        >= position_key(funding.transaction.position)
    {
        return Err(BtcLezFirstLockProofError::PairOrderMismatch);
    }
    let metadata = expected_metadata(runtime, agreement, terms);
    let custody = expected_custody(agreement, terms);
    if initialization.metadata != metadata
        || funding.metadata != metadata
        || funding.custody != custody
    {
        return Err(BtcLezFirstLockProofError::CurrentFundedEscrowMismatch);
    }
    Ok(())
}

fn valid_initialization(
    runtime: &lez_bridge_protocol::RuntimeDescriptor,
    agreement: &BtcAgreementV1,
    terms: &WitnessedNativeEscrowTerms,
    initialization: &WitnessedInitializationFoundFacts,
) -> bool {
    let expected_accounts = [
        Hex32::from_bytes(*agreement.lez_terms().metadata_account()),
        Hex32::from_bytes(*agreement.lez_terms().custody_account()),
        terms.depositor_account_id(),
        terms.claimant_account_id(),
        terms.aggregate_authority_account_id(),
    ];
    initialization.transaction.is_public
        && initialization.transaction.signer_account_ids.as_slice()
            == [terms.depositor_account_id()]
        && initialization.instruction.program_id == runtime.escrow_program_id
        && initialization.instruction.ordered_account_ids.as_slice() == expected_accounts
        && initialization.instruction.terms == *terms
}

fn valid_funding_transaction_and_instruction(
    runtime: &lez_bridge_protocol::RuntimeDescriptor,
    agreement: &BtcAgreementV1,
    terms: &WitnessedNativeEscrowTerms,
    transaction: &lez_bridge_protocol::ObservedTransactionFacts,
    instruction: &lez_bridge_protocol::NativeFundInstructionFacts,
) -> bool {
    let expected_accounts = [
        Hex32::from_bytes(*agreement.lez_terms().metadata_account()),
        Hex32::from_bytes(*agreement.lez_terms().custody_account()),
        terms.depositor_account_id(),
    ];
    transaction.is_public
        && transaction.signer_account_ids.as_slice() == [terms.depositor_account_id()]
        && instruction.program_id == runtime.escrow_program_id
        && instruction.swap_id == terms.swap_id()
        && instruction.ordered_account_ids.as_slice() == expected_accounts
}

fn expected_metadata(
    runtime: &lez_bridge_protocol::RuntimeDescriptor,
    agreement: &BtcAgreementV1,
    terms: &WitnessedNativeEscrowTerms,
) -> WitnessedEscrowMetadataFacts {
    WitnessedEscrowMetadataFacts::from_witnessed_native_terms(
        Hex32::from_bytes(*agreement.lez_terms().metadata_account()),
        runtime.escrow_program_id,
        Hex32::from_bytes(*agreement.lez_terms().custody_account()),
        terms,
        EscrowState::Funded,
    )
}

fn expected_custody(
    agreement: &BtcAgreementV1,
    terms: &WitnessedNativeEscrowTerms,
) -> NativeCustodyFacts {
    NativeCustodyFacts::new(
        Hex32::from_bytes(*agreement.lez_terms().custody_account()),
        terms.authenticated_transfer_program_id(),
        terms.amount().as_u128(),
    )
}

fn position_in_window(position: ChainPosition, window: DiscoveryWindow, tip_height: u64) -> bool {
    let Some(end) = window
        .start_height()
        .checked_add(u64::from(window.max_blocks() - 1))
    else {
        return false;
    };
    position.height >= window.start_height()
        && position.height <= end
        && position.height <= tip_height
}

const fn position_key(position: ChainPosition) -> (u64, u32) {
    (position.height, position.transaction_index)
}

fn btc_witnessed_terms(
    agreement: &BtcAgreementV1,
) -> Result<WitnessedNativeEscrowTerms, ProtocolValueError> {
    let signed = agreement.lez_terms();
    lez_bridge_protocol::WitnessedNativeEscrowTerms::new(
        lez_bridge_protocol::WitnessedNativeEscrowTermsInput {
            swap_id: Hex32::from_bytes(*agreement.body().swap_id()),
            terms_hash: Hex32::from_bytes(*agreement.agreement_commitment()),
            depositor: BridgeParticipant::Taker,
            depositor_account_id: Hex32::from_bytes(*signed.depositor_account()),
            claimant: BridgeParticipant::Maker,
            claimant_account_id: Hex32::from_bytes(*signed.claimant_account()),
            aggregate_authority_account_id: Hex32::from_bytes(
                *signed.aggregate_authority_account(),
            ),
            aggregate_x_only_public_key: Hex32::from_bytes(
                agreement.p2tr_contract().aggregate_internal_key_bytes(),
            ),
            amount: signed.amount(),
            refund_at_ms: signed.refund_at_ms(),
            authenticated_transfer_program_id: Hex32::from_bytes(
                *signed.authenticated_transfer_program_id(),
            ),
        },
    )
}
