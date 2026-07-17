//! Agreement- and local-policy-bound BTC witnessed LEZ asset operations.

use async_trait::async_trait;
use lez_bridge_client::{BridgeClient, BridgeClientError};
use lez_bridge_protocol::{
    AggregateBip340Signature, ClassifyFinalizedWitnessedAssetClaimV2Request,
    ClassifyFinalizedWitnessedAssetClaimV2Result,
    ClassifyFinalizedWitnessedAssetCustodyCreationV2Request,
    ClassifyFinalizedWitnessedAssetCustodyCreationV2Result,
    ClassifyFinalizedWitnessedAssetFundingV2Request,
    ClassifyFinalizedWitnessedAssetFundingV2Result,
    ClassifyFinalizedWitnessedAssetInitializationV2Request,
    ClassifyFinalizedWitnessedAssetInitializationV2Result, CompleteWitnessedAssetClaimV2Request,
    CompleteWitnessedAssetClaimV2Result, DiscoveryWindow, FinalizedWitnessedAssetClaimFactsV2,
    FinalizedWitnessedAssetCustodyCreationFactsV2, FinalizedWitnessedAssetFundingFactsV2,
    FinalizedWitnessedAssetInitializationFactsV2, FinalizedWitnessedAssetScanOutcomeV2,
    FinalizedWitnessedAssetTransactionTargetV2, FinalizedWitnessedClaimObservationTarget, Hex32,
    MessageContext, NativeRefundObservationTarget, ObserveFinalizedWitnessedAssetClaimV2Request,
    ObserveFinalizedWitnessedAssetClaimV2Result, ObserveWitnessedAssetEscrowV2Request,
    ObserveWitnessedAssetEscrowV2Result, ObserveWitnessedAssetRefundV2Request,
    ObserveWitnessedAssetRefundV2Result, Participant as BridgeParticipant,
    PrepareWitnessedAssetClaimV2Request, PrepareWitnessedAssetClaimV2Result,
    PrepareWitnessedAssetEscrowV2Request, PrepareWitnessedAssetEscrowV2Result,
    PrepareWitnessedAssetRefundV2Request, PrepareWitnessedAssetRefundV2Result,
    PreparedWitnessedClaim, ProtocolValueError, RequestId, RuntimeCompatibility, TransactionId,
    WitnessedAssetPreparedEffectV2, WitnessedLezAssetTermsV2, WitnessedLezAssetV2,
    WitnessedNativeEscrowTerms, WitnessedNativeEscrowTermsInput, WitnessedTokenEscrowTermsV2,
    WitnessedTokenEscrowTermsV2Input,
};
use lez_btc_swap_sdk::{
    BtcAgreementV1, BtcLezAssetExtensionV1, BtcLezAssetExtensionV1Error, BtcLezAssetV1,
};
use lez_swap_core::Participant;
use thiserror::Error;

use crate::LezBridgeAdapter;

/// An exact BTC agreement plus countersigned and locally accepted LEZ asset policy.
///
/// Construction is intentionally separate from I/O. A validated extension can
/// otherwise be replayed beside a different agreement or accepted without the
/// application's exact program, definition, owner, and ATA policy.
#[derive(Clone, Debug, Eq, PartialEq)]
#[must_use]
pub struct BtcLezAssetBridgeBindingV2 {
    terms: WitnessedLezAssetTermsV2,
    channel_id: [u8; 32],
    genesis_block_hash: [u8; 32],
    escrow_program_id: [u8; 32],
    maker_account_id: [u8; 32],
    taker_account_id: [u8; 32],
    depositor: Participant,
    claimant: Participant,
}

impl BtcLezAssetBridgeBindingV2 {
    /// Maps one validated BTC agreement and extension into exact bridge-v2 terms.
    ///
    /// `expected_asset` is application-owned local policy. For custom tokens it
    /// must name every exact program, definition, owner, and ATA.
    ///
    /// # Errors
    ///
    /// Rejects a cross-agreement extension, local policy drift, or values that
    /// cannot form strict bridge protocol terms.
    pub fn new(
        agreement: &BtcAgreementV1,
        extension: &BtcLezAssetExtensionV1,
        expected_asset: &BtcLezAssetV1,
    ) -> Result<Self, BtcLezAssetBridgeBindingV2Error> {
        if extension.base_agreement_commitment() != agreement.agreement_commitment() {
            return Err(BtcLezAssetBridgeBindingV2Error::BaseAgreementMismatch);
        }
        extension
            .ensure_asset(expected_asset)
            .map_err(BtcLezAssetBridgeBindingV2Error::LocalAssetPolicy)?;

        let signed = agreement.lez_terms();
        let common = CommonTerms {
            swap_id: Hex32::from_bytes(*agreement.body().swap_id()),
            terms_hash: Hex32::from_bytes(extension.lez_terms_binding()),
            depositor: bridge_participant(agreement.lez_depositor()),
            claimant: bridge_participant(agreement.lez_claimant()),
        };
        let terms = match extension.asset() {
            BtcLezAssetV1::Native => {
                let native = WitnessedNativeEscrowTerms::new(WitnessedNativeEscrowTermsInput {
                    swap_id: common.swap_id,
                    terms_hash: common.terms_hash,
                    depositor: common.depositor,
                    depositor_account_id: Hex32::from_bytes(*signed.depositor_account()),
                    claimant: common.claimant,
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
                })?;
                WitnessedLezAssetTermsV2::native(native)
            }
            BtcLezAssetV1::CustomToken(token) => {
                let token = WitnessedTokenEscrowTermsV2::new(WitnessedTokenEscrowTermsV2Input {
                    swap_id: common.swap_id,
                    terms_hash: common.terms_hash,
                    depositor: common.depositor,
                    depositor_owner_account_id: Hex32::from_bytes(*token.depositor_owner_account()),
                    depositor_ata_account_id: Hex32::from_bytes(*token.depositor_ata_account()),
                    claimant: common.claimant,
                    claimant_owner_account_id: Hex32::from_bytes(*token.claimant_owner_account()),
                    claimant_ata_account_id: Hex32::from_bytes(*token.claimant_ata_account()),
                    custody_ata_account_id: Hex32::from_bytes(*token.custody_ata_account()),
                    token_program_id: Hex32::from_bytes(*token.token_program_id()),
                    ata_program_id: Hex32::from_bytes(*token.ata_program_id()),
                    token_definition_account_id: Hex32::from_bytes(
                        *token.token_definition_account(),
                    ),
                    aggregate_authority_account_id: Hex32::from_bytes(
                        *token.aggregate_authority_account(),
                    ),
                    aggregate_x_only_public_key: Hex32::from_bytes(
                        *token.aggregate_x_only_public_key(),
                    ),
                    amount: token.amount(),
                    refund_at_ms: token.refund_at_ms(),
                })?;
                WitnessedLezAssetTermsV2::custom_token(token)
            }
        };

        Ok(Self {
            terms,
            channel_id: *signed.channel_id(),
            genesis_block_hash: *signed.genesis_block_hash(),
            escrow_program_id: *signed.escrow_program_id(),
            maker_account_id: *agreement
                .participant(Participant::Maker)
                .lez_owner_account(),
            taker_account_id: *agreement
                .participant(Participant::Taker)
                .lez_owner_account(),
            depositor: agreement.lez_depositor(),
            claimant: agreement.lez_claimant(),
        })
    }

    /// Exact additive v2 terms passed unchanged to every bridge operation.
    pub const fn terms(&self) -> &WitnessedLezAssetTermsV2 {
        &self.terms
    }

    /// Agreement-derived LEZ depositor role.
    #[must_use]
    pub const fn depositor(&self) -> Participant {
        self.depositor
    }

    /// Agreement-derived immutable LEZ claimant role.
    #[must_use]
    pub const fn claimant(&self) -> Participant {
        self.claimant
    }

    fn account_id(&self, participant: Participant) -> &[u8; 32] {
        match participant {
            Participant::Maker => &self.maker_account_id,
            Participant::Taker => &self.taker_account_id,
        }
    }
}

#[derive(Clone, Copy)]
struct CommonTerms {
    swap_id: Hex32,
    terms_hash: Hex32,
    depositor: BridgeParticipant,
    claimant: BridgeParticipant,
}

/// Failure to bind the additive asset extension to exact local bridge terms.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum BtcLezAssetBridgeBindingV2Error {
    /// The extension was validated for a different base agreement.
    #[error("LEZ asset extension belongs to a different BTC agreement")]
    BaseAgreementMismatch,
    /// The countersigned asset differs from exact application-owned policy.
    #[error("countersigned LEZ asset differs from local policy")]
    LocalAssetPolicy(#[source] BtcLezAssetExtensionV1Error),
    /// The signed values cannot form strict bridge-v2 protocol terms.
    #[error("countersigned LEZ asset values are invalid at the bridge boundary")]
    Protocol(#[from] ProtocolValueError),
}

/// Exactly-once transport surface for all additive witnessed-asset operations.
///
/// No generic submission method is present. A classifier result therefore
/// cannot itself send, and `Uncertain`, `Unavailable`, timeout, or transport
/// failure cannot be collapsed into send authority inside this boundary.
#[async_trait]
pub trait LezBridgeAssetV2Transport: Send + Sync {
    /// Concrete transport or client validation failure.
    type Error: std::error::Error + Send + Sync + 'static;

    /// Prepares the complete native or token escrow plan once.
    async fn prepare_witnessed_asset_escrow_v2(
        &self,
        request: PrepareWitnessedAssetEscrowV2Request,
    ) -> Result<PrepareWitnessedAssetEscrowV2Result, Self::Error>;

    /// Observes the exact prepared escrow plan once.
    async fn observe_witnessed_asset_escrow_v2(
        &self,
        request: ObserveWitnessedAssetEscrowV2Request,
    ) -> Result<ObserveWitnessedAssetEscrowV2Result, Self::Error>;

    /// Reserves an exact witnessed claim transcript once.
    async fn prepare_witnessed_asset_claim_v2(
        &self,
        request: PrepareWitnessedAssetClaimV2Request,
    ) -> Result<PrepareWitnessedAssetClaimV2Result, Self::Error>;

    /// Completes an exact countersigned claim transcript once.
    async fn complete_witnessed_asset_claim_v2(
        &self,
        request: CompleteWitnessedAssetClaimV2Request,
    ) -> Result<CompleteWitnessedAssetClaimV2Result, Self::Error>;

    /// Observes one exact finalized claim once.
    async fn observe_finalized_witnessed_asset_claim_v2(
        &self,
        request: ObserveFinalizedWitnessedAssetClaimV2Request,
    ) -> Result<ObserveFinalizedWitnessedAssetClaimV2Result, Self::Error>;

    /// Prepares one fixed-destination refund once.
    async fn prepare_witnessed_asset_refund_v2(
        &self,
        request: PrepareWitnessedAssetRefundV2Request,
    ) -> Result<PrepareWitnessedAssetRefundV2Result, Self::Error>;

    /// Observes state and optional refund evidence once.
    async fn observe_witnessed_asset_refund_v2(
        &self,
        request: ObserveWitnessedAssetRefundV2Request,
    ) -> Result<ObserveWitnessedAssetRefundV2Result, Self::Error>;

    /// Classifies finalized initialization once.
    async fn classify_finalized_witnessed_asset_initialization_v2(
        &self,
        request: ClassifyFinalizedWitnessedAssetInitializationV2Request,
    ) -> Result<ClassifyFinalizedWitnessedAssetInitializationV2Result, Self::Error>;

    /// Classifies finalized token custody creation once.
    async fn classify_finalized_witnessed_asset_custody_creation_v2(
        &self,
        request: ClassifyFinalizedWitnessedAssetCustodyCreationV2Request,
    ) -> Result<ClassifyFinalizedWitnessedAssetCustodyCreationV2Result, Self::Error>;

    /// Classifies finalized funding once.
    async fn classify_finalized_witnessed_asset_funding_v2(
        &self,
        request: ClassifyFinalizedWitnessedAssetFundingV2Request,
    ) -> Result<ClassifyFinalizedWitnessedAssetFundingV2Result, Self::Error>;

    /// Classifies finalized claim presence once.
    async fn classify_finalized_witnessed_asset_claim_v2(
        &self,
        request: ClassifyFinalizedWitnessedAssetClaimV2Request,
    ) -> Result<ClassifyFinalizedWitnessedAssetClaimV2Result, Self::Error>;
}

#[async_trait]
impl LezBridgeAssetV2Transport for BridgeClient {
    type Error = BridgeClientError;

    async fn prepare_witnessed_asset_escrow_v2(
        &self,
        request: PrepareWitnessedAssetEscrowV2Request,
    ) -> Result<PrepareWitnessedAssetEscrowV2Result, Self::Error> {
        BridgeClient::prepare_witnessed_asset_escrow_v2(self, request).await
    }

    async fn observe_witnessed_asset_escrow_v2(
        &self,
        request: ObserveWitnessedAssetEscrowV2Request,
    ) -> Result<ObserveWitnessedAssetEscrowV2Result, Self::Error> {
        BridgeClient::observe_witnessed_asset_escrow_v2(self, request).await
    }

    async fn prepare_witnessed_asset_claim_v2(
        &self,
        request: PrepareWitnessedAssetClaimV2Request,
    ) -> Result<PrepareWitnessedAssetClaimV2Result, Self::Error> {
        BridgeClient::prepare_witnessed_asset_claim_v2(self, request).await
    }

    async fn complete_witnessed_asset_claim_v2(
        &self,
        request: CompleteWitnessedAssetClaimV2Request,
    ) -> Result<CompleteWitnessedAssetClaimV2Result, Self::Error> {
        BridgeClient::complete_witnessed_asset_claim_v2(self, request).await
    }

    async fn observe_finalized_witnessed_asset_claim_v2(
        &self,
        request: ObserveFinalizedWitnessedAssetClaimV2Request,
    ) -> Result<ObserveFinalizedWitnessedAssetClaimV2Result, Self::Error> {
        BridgeClient::observe_finalized_witnessed_asset_claim_v2(self, request).await
    }

    async fn prepare_witnessed_asset_refund_v2(
        &self,
        request: PrepareWitnessedAssetRefundV2Request,
    ) -> Result<PrepareWitnessedAssetRefundV2Result, Self::Error> {
        BridgeClient::prepare_witnessed_asset_refund_v2(self, request).await
    }

    async fn observe_witnessed_asset_refund_v2(
        &self,
        request: ObserveWitnessedAssetRefundV2Request,
    ) -> Result<ObserveWitnessedAssetRefundV2Result, Self::Error> {
        BridgeClient::observe_witnessed_asset_refund_v2(self, request).await
    }

    async fn classify_finalized_witnessed_asset_initialization_v2(
        &self,
        request: ClassifyFinalizedWitnessedAssetInitializationV2Request,
    ) -> Result<ClassifyFinalizedWitnessedAssetInitializationV2Result, Self::Error> {
        BridgeClient::classify_finalized_witnessed_asset_initialization_v2(self, request).await
    }

    async fn classify_finalized_witnessed_asset_custody_creation_v2(
        &self,
        request: ClassifyFinalizedWitnessedAssetCustodyCreationV2Request,
    ) -> Result<ClassifyFinalizedWitnessedAssetCustodyCreationV2Result, Self::Error> {
        BridgeClient::classify_finalized_witnessed_asset_custody_creation_v2(self, request).await
    }

    async fn classify_finalized_witnessed_asset_funding_v2(
        &self,
        request: ClassifyFinalizedWitnessedAssetFundingV2Request,
    ) -> Result<ClassifyFinalizedWitnessedAssetFundingV2Result, Self::Error> {
        BridgeClient::classify_finalized_witnessed_asset_funding_v2(self, request).await
    }

    async fn classify_finalized_witnessed_asset_claim_v2(
        &self,
        request: ClassifyFinalizedWitnessedAssetClaimV2Request,
    ) -> Result<ClassifyFinalizedWitnessedAssetClaimV2Result, Self::Error> {
        BridgeClient::classify_finalized_witnessed_asset_claim_v2(self, request).await
    }
}

/// Failure to perform one policy-bound BTC witnessed-asset bridge operation.
#[derive(Debug, Error)]
pub enum BtcLezAssetBridgeV2Error<E: std::error::Error + 'static> {
    /// The sidecar is not the pinned official Lee v0.2 compatibility runtime.
    #[error("LEZ asset bridge runtime is incompatible")]
    IncompatibleRuntime,
    /// Runtime channel or genesis differs from the countersigned agreement.
    #[error("LEZ asset bridge chain identity differs from the agreement")]
    ChainIdentityMismatch,
    /// Runtime escrow program differs from the countersigned agreement.
    #[error("LEZ asset bridge program differs from the agreement")]
    EscrowProgramMismatch,
    /// Runtime signer differs from the local agreement role account.
    #[error("LEZ asset bridge signer differs from the local agreement role")]
    SignerAccountMismatch,
    /// Escrow preparation is restricted to the agreement depositor.
    #[error("local participant is not the LEZ asset depositor")]
    WrongDepositor,
    /// Claim preparation and completion are restricted to the agreement claimant.
    #[error("local participant is not the LEZ asset claimant")]
    WrongClaimant,
    /// Native assets have no distinct custody-ATA creation effect.
    #[error("custody creation classification requires a custom token")]
    CustodyCreationRequiresCustomToken,
    /// The request could not form strict bridge-protocol values.
    #[error("LEZ asset request is invalid at the bridge boundary")]
    Protocol(#[source] ProtocolValueError),
    /// One exactly-once call failed; the result is never converted to absence.
    #[error("LEZ asset bridge operation is unavailable")]
    Transport(#[source] E),
    /// The response did not echo the caller-owned request context.
    #[error("LEZ asset bridge response context mismatch")]
    ResponseContextMismatch,
    /// The response did not echo the exact policy-bound terms.
    #[error("LEZ asset bridge response terms mismatch")]
    ResponseTermsMismatch,
    /// The classifier response did not echo the caller-owned target.
    #[error("LEZ asset bridge response target mismatch")]
    ResponseTargetMismatch,
    /// The classifier response did not echo the exact claim transcript.
    #[error("LEZ asset bridge response claim mismatch")]
    ResponseClaimMismatch,
}

#[derive(Clone, Copy)]
enum RequiredRole {
    Depositor,
    Claimant,
    EitherParticipant,
}

impl<T> LezBridgeAdapter<T>
where
    T: LezBridgeAssetV2Transport,
{
    /// Prepares one complete native or token escrow plan exactly once.
    ///
    /// # Errors
    ///
    /// Rejects runtime/role/echo drift or preserves the one transport failure.
    pub async fn prepare_btc_asset_escrow_v2(
        &self,
        binding: &BtcLezAssetBridgeBindingV2,
        request_id: RequestId,
    ) -> Result<PrepareWitnessedAssetEscrowV2Result, BtcLezAssetBridgeV2Error<T::Error>> {
        self.validate_asset_operation(binding, RequiredRole::Depositor)?;
        let context = self.asset_context(request_id);
        let response = self
            .transport
            .prepare_witnessed_asset_escrow_v2(PrepareWitnessedAssetEscrowV2Request::new(
                context.clone(),
                self.runtime.clone(),
                binding.terms.clone(),
            ))
            .await
            .map_err(BtcLezAssetBridgeV2Error::Transport)?;
        validate_response_echo(binding, &context, &response.context, &response.terms)?;
        Ok(response)
    }

    /// Observes an exact ordered prepared plan in a caller-owned window once.
    ///
    /// # Errors
    ///
    /// Rejects runtime, role, plan, protocol, response, or transport failures.
    pub async fn observe_btc_asset_escrow_v2(
        &self,
        binding: &BtcLezAssetBridgeBindingV2,
        request_id: RequestId,
        prepared_effects: Vec<WitnessedAssetPreparedEffectV2>,
        window: DiscoveryWindow,
    ) -> Result<ObserveWitnessedAssetEscrowV2Result, BtcLezAssetBridgeV2Error<T::Error>> {
        self.validate_asset_operation(binding, RequiredRole::EitherParticipant)?;
        let context = self.asset_context(request_id);
        let request = ObserveWitnessedAssetEscrowV2Request::new(
            context.clone(),
            self.runtime.clone(),
            binding.terms.clone(),
            prepared_effects,
            window,
        )
        .map_err(BtcLezAssetBridgeV2Error::Protocol)?;
        let response = self
            .transport
            .observe_witnessed_asset_escrow_v2(request)
            .await
            .map_err(BtcLezAssetBridgeV2Error::Transport)?;
        validate_response_echo(binding, &context, &response.context, &response.terms)?;
        Ok(response)
    }

    /// Reserves the claimant's exact witnessed transcript once.
    ///
    /// # Errors
    ///
    /// Rejects runtime/claimant/echo drift or preserves the transport failure.
    pub async fn prepare_btc_asset_claim_v2(
        &self,
        binding: &BtcLezAssetBridgeBindingV2,
        request_id: RequestId,
        funding_transaction_id: TransactionId,
    ) -> Result<PrepareWitnessedAssetClaimV2Result, BtcLezAssetBridgeV2Error<T::Error>> {
        self.validate_asset_operation(binding, RequiredRole::Claimant)?;
        let context = self.asset_context(request_id);
        let response = self
            .transport
            .prepare_witnessed_asset_claim_v2(PrepareWitnessedAssetClaimV2Request::new(
                context.clone(),
                self.runtime.clone(),
                binding.terms.clone(),
                funding_transaction_id,
            ))
            .await
            .map_err(BtcLezAssetBridgeV2Error::Transport)?;
        validate_response_echo(binding, &context, &response.context, &response.terms)?;
        Ok(response)
    }

    /// Completes the claimant's exact externally countersigned transcript once.
    ///
    /// # Errors
    ///
    /// Rejects runtime/claimant/echo drift or preserves the transport failure.
    pub async fn complete_btc_asset_claim_v2(
        &self,
        binding: &BtcLezAssetBridgeBindingV2,
        request_id: RequestId,
        claim: PreparedWitnessedClaim,
        aggregate_signature: AggregateBip340Signature,
    ) -> Result<CompleteWitnessedAssetClaimV2Result, BtcLezAssetBridgeV2Error<T::Error>> {
        self.validate_asset_operation(binding, RequiredRole::Claimant)?;
        let context = self.asset_context(request_id);
        let response = self
            .transport
            .complete_witnessed_asset_claim_v2(CompleteWitnessedAssetClaimV2Request::new(
                context.clone(),
                self.runtime.clone(),
                binding.terms.clone(),
                claim,
                aggregate_signature,
            ))
            .await
            .map_err(BtcLezAssetBridgeV2Error::Transport)?;
        validate_response_echo(binding, &context, &response.context, &response.terms)?;
        Ok(response)
    }

    /// Observes one exact finalized claim or terms-discovered claim once.
    ///
    /// # Errors
    ///
    /// Rejects runtime, participant, response, or transport failures.
    pub async fn observe_finalized_btc_asset_claim_v2(
        &self,
        binding: &BtcLezAssetBridgeBindingV2,
        request_id: RequestId,
        claim: PreparedWitnessedClaim,
        target: FinalizedWitnessedClaimObservationTarget,
        window: DiscoveryWindow,
    ) -> Result<ObserveFinalizedWitnessedAssetClaimV2Result, BtcLezAssetBridgeV2Error<T::Error>>
    {
        self.validate_asset_operation(binding, RequiredRole::EitherParticipant)?;
        let context = self.asset_context(request_id);
        let response = self
            .transport
            .observe_finalized_witnessed_asset_claim_v2(
                ObserveFinalizedWitnessedAssetClaimV2Request {
                    context: context.clone(),
                    runtime: self.runtime.clone(),
                    terms: binding.terms.clone(),
                    claim,
                    target,
                    window,
                },
            )
            .await
            .map_err(BtcLezAssetBridgeV2Error::Transport)?;
        validate_response_echo(binding, &context, &response.context, &response.terms)?;
        Ok(response)
    }

    /// Prepares one fixed-destination refund from either bound participant once.
    ///
    /// # Errors
    ///
    /// Rejects runtime/participant/echo drift or preserves the transport failure.
    pub async fn prepare_btc_asset_refund_v2(
        &self,
        binding: &BtcLezAssetBridgeBindingV2,
        request_id: RequestId,
    ) -> Result<PrepareWitnessedAssetRefundV2Result, BtcLezAssetBridgeV2Error<T::Error>> {
        self.validate_asset_operation(binding, RequiredRole::EitherParticipant)?;
        let context = self.asset_context(request_id);
        let response = self
            .transport
            .prepare_witnessed_asset_refund_v2(PrepareWitnessedAssetRefundV2Request::new(
                context.clone(),
                self.runtime.clone(),
                binding.terms.clone(),
            ))
            .await
            .map_err(BtcLezAssetBridgeV2Error::Transport)?;
        validate_response_echo(binding, &context, &response.context, &response.terms)?;
        Ok(response)
    }

    /// Observes current state and caller-selected refund evidence once.
    ///
    /// # Errors
    ///
    /// Rejects runtime, participant, response, or transport failures.
    pub async fn observe_btc_asset_refund_v2(
        &self,
        binding: &BtcLezAssetBridgeBindingV2,
        request_id: RequestId,
        target: NativeRefundObservationTarget,
    ) -> Result<ObserveWitnessedAssetRefundV2Result, BtcLezAssetBridgeV2Error<T::Error>> {
        self.validate_asset_operation(binding, RequiredRole::EitherParticipant)?;
        let context = self.asset_context(request_id);
        let response = self
            .transport
            .observe_witnessed_asset_refund_v2(ObserveWitnessedAssetRefundV2Request::new(
                context.clone(),
                self.runtime.clone(),
                binding.terms.clone(),
                target,
            ))
            .await
            .map_err(BtcLezAssetBridgeV2Error::Transport)?;
        validate_response_echo(binding, &context, &response.context, &response.terms)?;
        Ok(response)
    }

    /// Classifies finalized initialization without collapsing uncertainty once.
    ///
    /// # Errors
    ///
    /// Rejects runtime/participant/echo drift or preserves transport uncertainty.
    pub async fn classify_finalized_btc_asset_initialization_v2(
        &self,
        binding: &BtcLezAssetBridgeBindingV2,
        request_id: RequestId,
        target: FinalizedWitnessedAssetTransactionTargetV2,
        window: DiscoveryWindow,
    ) -> Result<
        FinalizedWitnessedAssetScanOutcomeV2<FinalizedWitnessedAssetInitializationFactsV2>,
        BtcLezAssetBridgeV2Error<T::Error>,
    > {
        self.validate_asset_operation(binding, RequiredRole::EitherParticipant)?;
        let context = self.asset_context(request_id);
        let response = self
            .transport
            .classify_finalized_witnessed_asset_initialization_v2(
                ClassifyFinalizedWitnessedAssetInitializationV2Request {
                    context: context.clone(),
                    runtime: self.runtime.clone(),
                    terms: binding.terms.clone(),
                    target: target.clone(),
                    window,
                },
            )
            .await
            .map_err(BtcLezAssetBridgeV2Error::Transport)?;
        validate_classifier_echo(
            binding,
            &context,
            &target,
            &response.context,
            &response.terms,
            &response.target,
        )?;
        Ok(response.outcome)
    }

    /// Classifies token custody-ATA creation without collapsing uncertainty once.
    ///
    /// # Errors
    ///
    /// Rejects native assets, runtime/participant/echo drift, or transport uncertainty.
    pub async fn classify_finalized_btc_asset_custody_creation_v2(
        &self,
        binding: &BtcLezAssetBridgeBindingV2,
        request_id: RequestId,
        target: FinalizedWitnessedAssetTransactionTargetV2,
        window: DiscoveryWindow,
    ) -> Result<
        FinalizedWitnessedAssetScanOutcomeV2<FinalizedWitnessedAssetCustodyCreationFactsV2>,
        BtcLezAssetBridgeV2Error<T::Error>,
    > {
        self.validate_asset_operation(binding, RequiredRole::EitherParticipant)?;
        if matches!(binding.terms.asset(), WitnessedLezAssetV2::Native(_)) {
            return Err(BtcLezAssetBridgeV2Error::CustodyCreationRequiresCustomToken);
        }
        let context = self.asset_context(request_id);
        let response = self
            .transport
            .classify_finalized_witnessed_asset_custody_creation_v2(
                ClassifyFinalizedWitnessedAssetCustodyCreationV2Request {
                    context: context.clone(),
                    runtime: self.runtime.clone(),
                    terms: binding.terms.clone(),
                    target: target.clone(),
                    window,
                },
            )
            .await
            .map_err(BtcLezAssetBridgeV2Error::Transport)?;
        validate_classifier_echo(
            binding,
            &context,
            &target,
            &response.context,
            &response.terms,
            &response.target,
        )?;
        Ok(response.outcome)
    }

    /// Classifies finalized funding without collapsing uncertainty once.
    ///
    /// # Errors
    ///
    /// Rejects runtime/participant/echo drift or preserves transport uncertainty.
    pub async fn classify_finalized_btc_asset_funding_v2(
        &self,
        binding: &BtcLezAssetBridgeBindingV2,
        request_id: RequestId,
        target: FinalizedWitnessedAssetTransactionTargetV2,
        window: DiscoveryWindow,
    ) -> Result<
        FinalizedWitnessedAssetScanOutcomeV2<FinalizedWitnessedAssetFundingFactsV2>,
        BtcLezAssetBridgeV2Error<T::Error>,
    > {
        self.validate_asset_operation(binding, RequiredRole::EitherParticipant)?;
        let context = self.asset_context(request_id);
        let response = self
            .transport
            .classify_finalized_witnessed_asset_funding_v2(
                ClassifyFinalizedWitnessedAssetFundingV2Request {
                    context: context.clone(),
                    runtime: self.runtime.clone(),
                    terms: binding.terms.clone(),
                    target: target.clone(),
                    window,
                },
            )
            .await
            .map_err(BtcLezAssetBridgeV2Error::Transport)?;
        validate_classifier_echo(
            binding,
            &context,
            &target,
            &response.context,
            &response.terms,
            &response.target,
        )?;
        Ok(response.outcome)
    }

    /// Classifies finalized claim presence without collapsing uncertainty once.
    ///
    /// # Errors
    ///
    /// Rejects runtime/participant/echo/transcript drift or preserves transport uncertainty.
    pub async fn classify_finalized_btc_asset_claim_v2(
        &self,
        binding: &BtcLezAssetBridgeBindingV2,
        request_id: RequestId,
        claim: PreparedWitnessedClaim,
        target: FinalizedWitnessedAssetTransactionTargetV2,
        window: DiscoveryWindow,
    ) -> Result<
        FinalizedWitnessedAssetScanOutcomeV2<FinalizedWitnessedAssetClaimFactsV2>,
        BtcLezAssetBridgeV2Error<T::Error>,
    > {
        self.validate_asset_operation(binding, RequiredRole::EitherParticipant)?;
        let context = self.asset_context(request_id);
        let response = self
            .transport
            .classify_finalized_witnessed_asset_claim_v2(
                ClassifyFinalizedWitnessedAssetClaimV2Request {
                    context: context.clone(),
                    runtime: self.runtime.clone(),
                    terms: binding.terms.clone(),
                    claim: claim.clone(),
                    target: target.clone(),
                    window,
                },
            )
            .await
            .map_err(BtcLezAssetBridgeV2Error::Transport)?;
        validate_classifier_echo(
            binding,
            &context,
            &target,
            &response.context,
            &response.terms,
            &response.target,
        )?;
        if response.claim != claim {
            return Err(BtcLezAssetBridgeV2Error::ResponseClaimMismatch);
        }
        Ok(response.outcome)
    }

    fn asset_context(&self, request_id: RequestId) -> MessageContext {
        MessageContext::new(
            self.run_id.clone(),
            request_id,
            bridge_participant(self.local_participant),
        )
    }

    fn validate_asset_operation(
        &self,
        binding: &BtcLezAssetBridgeBindingV2,
        required_role: RequiredRole,
    ) -> Result<(), BtcLezAssetBridgeV2Error<T::Error>> {
        if self.runtime.compatibility != RuntimeCompatibility::LeeV0_2_0 {
            return Err(BtcLezAssetBridgeV2Error::IncompatibleRuntime);
        }
        if self.runtime.channel_id.as_bytes() != &binding.channel_id
            || self.runtime.genesis_block_hash.as_bytes() != &binding.genesis_block_hash
        {
            return Err(BtcLezAssetBridgeV2Error::ChainIdentityMismatch);
        }
        if self.runtime.escrow_program_id.as_bytes() != &binding.escrow_program_id {
            return Err(BtcLezAssetBridgeV2Error::EscrowProgramMismatch);
        }
        if self.runtime.signer_account_id.as_bytes() != binding.account_id(self.local_participant) {
            return Err(BtcLezAssetBridgeV2Error::SignerAccountMismatch);
        }
        match required_role {
            RequiredRole::Depositor if self.local_participant != binding.depositor => {
                Err(BtcLezAssetBridgeV2Error::WrongDepositor)
            }
            RequiredRole::Claimant if self.local_participant != binding.claimant => {
                Err(BtcLezAssetBridgeV2Error::WrongClaimant)
            }
            RequiredRole::Depositor | RequiredRole::Claimant | RequiredRole::EitherParticipant => {
                Ok(())
            }
        }
    }
}

fn validate_response_echo<E: std::error::Error + 'static>(
    binding: &BtcLezAssetBridgeBindingV2,
    expected_context: &MessageContext,
    actual_context: &MessageContext,
    actual_terms: &WitnessedLezAssetTermsV2,
) -> Result<(), BtcLezAssetBridgeV2Error<E>> {
    if actual_context != expected_context {
        return Err(BtcLezAssetBridgeV2Error::ResponseContextMismatch);
    }
    if actual_terms != &binding.terms {
        return Err(BtcLezAssetBridgeV2Error::ResponseTermsMismatch);
    }
    Ok(())
}

fn validate_classifier_echo<E: std::error::Error + 'static>(
    binding: &BtcLezAssetBridgeBindingV2,
    expected_context: &MessageContext,
    expected_target: &FinalizedWitnessedAssetTransactionTargetV2,
    actual_context: &MessageContext,
    actual_terms: &WitnessedLezAssetTermsV2,
    actual_target: &FinalizedWitnessedAssetTransactionTargetV2,
) -> Result<(), BtcLezAssetBridgeV2Error<E>> {
    validate_response_echo(binding, expected_context, actual_context, actual_terms)?;
    if actual_target != expected_target {
        return Err(BtcLezAssetBridgeV2Error::ResponseTargetMismatch);
    }
    Ok(())
}

const fn bridge_participant(participant: Participant) -> BridgeParticipant {
    match participant {
        Participant::Maker => BridgeParticipant::Maker,
        Participant::Taker => BridgeParticipant::Taker,
    }
}
