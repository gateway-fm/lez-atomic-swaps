//! Bounded wire types for pinned LEZ runtime compatibility sidecars.
//!
//! The protocol deliberately carries primitive facts. The main process remains
//! responsible for deciding whether those facts prove a swap transition.

#![forbid(unsafe_code)]

mod messages;
mod primitives;
mod xmr_v3;

/// Stable JSON-RPC method for runtime and signer identity discovery.
pub const METHOD_DESCRIBE_RUNTIME: &str = "lez_bridge.v1.describe_runtime";
/// Stable JSON-RPC method for one stable current canonical chain clock.
pub const METHOD_OBSERVE_CURRENT_CLOCK: &str = "lez_bridge.v1.observe_current_clock";
/// Stable JSON-RPC method for one stable genesis-bound finalized chain clock.
pub const METHOD_OBSERVE_FINALIZED_CLOCK: &str = "lez_bridge.v1.observe_finalized_clock";
/// Prepares and durably reserves one bounded local-profile clock transaction.
pub const METHOD_PREPARE_CURRENT_PROFILE_CLOCK: &str =
    "lez_bridge.v1.prepare_current_profile_clock";
/// Verifies one exact submitted local-profile clock transaction without resubmission.
pub const METHOD_VERIFY_CURRENT_PROFILE_CLOCK: &str = "lez_bridge.v1.verify_current_profile_clock";
/// Stable JSON-RPC method for preparing native initialization and funding.
pub const METHOD_PREPARE_NATIVE_ESCROW: &str = "lez_bridge.v1.prepare_native_escrow";
/// Stable JSON-RPC method for preparing aggregate-witness initialization and funding.
pub const METHOD_PREPARE_WITNESSED_ESCROW: &str = "lez_bridge.v1.prepare_witnessed_escrow";
/// Stable JSON-RPC method for observing native initialization and funding.
pub const METHOD_OBSERVE_ESCROW: &str = "lez_bridge.v1.observe_escrow";
/// Stable JSON-RPC method for observing aggregate-witness initialization and funding.
pub const METHOD_OBSERVE_WITNESSED_ESCROW: &str = "lez_bridge.v1.observe_witnessed_escrow";
/// Stable JSON-RPC method for preparing a preimage-revealing claim.
pub const METHOD_PREPARE_REVEALING_CLAIM: &str = "lez_bridge.v1.prepare_revealing_claim";
/// Stable JSON-RPC method for reserving an unsigned aggregate-witness claim message.
pub const METHOD_PREPARE_WITNESSED_CLAIM: &str = "lez_bridge.v1.prepare_witnessed_claim";
/// Stable JSON-RPC method for completing a reservation with an aggregate signature.
pub const METHOD_COMPLETE_WITNESSED_CLAIM: &str = "lez_bridge.v1.complete_witnessed_claim";
/// Stable JSON-RPC method for observing one finalized aggregate-witness funding transaction.
pub const METHOD_OBSERVE_FINALIZED_WITNESSED_FUNDING: &str =
    "lez_bridge.v1.observe_finalized_witnessed_funding";
/// Stable JSON-RPC method for classifying one exact finalized witnessed initialization.
pub const METHOD_CLASSIFY_FINALIZED_WITNESSED_INITIALIZATION: &str =
    "lez_bridge.v1.classify_finalized_witnessed_initialization";
/// Stable JSON-RPC method for classifying finalized aggregate-witness funding.
///
/// This additive v1 method preserves the original found-only observer while
/// permitting affirmative absence only after a complete stable finalized scan.
pub const METHOD_CLASSIFY_FINALIZED_WITNESSED_FUNDING: &str =
    "lez_bridge.v1.classify_finalized_witnessed_funding";
/// Stable JSON-RPC method for observing one exact finalized aggregate-witness claim.
pub const METHOD_OBSERVE_FINALIZED_WITNESSED_CLAIM: &str =
    "lez_bridge.v1.observe_finalized_witnessed_claim";
/// Stable JSON-RPC method for classifying exact witnessed-claim presence.
///
/// This additive v1 method preserves the original found-only observer while
/// giving actors a positive, strictly typed proof of a complete stable absence.
pub const METHOD_CLASSIFY_FINALIZED_WITNESSED_CLAIM: &str =
    "lez_bridge.v1.classify_finalized_witnessed_claim";
/// Stable JSON-RPC method for observing a preimage-revealing claim.
pub const METHOD_OBSERVE_REVEALING_CLAIM: &str = "lez_bridge.v1.observe_revealing_claim";
/// Stable JSON-RPC method for preparing a fixed-destination native refund.
pub const METHOD_PREPARE_NATIVE_REFUND: &str = "lez_bridge.v1.prepare_native_refund";
/// Stable JSON-RPC method for observing native escrow state and refunds.
pub const METHOD_OBSERVE_NATIVE_REFUND: &str = "lez_bridge.v1.observe_native_refund";
/// Stable JSON-RPC method for submitting exact persisted transaction bytes.
pub const METHOD_SUBMIT_TRANSACTION: &str = "lez_bridge.v1.submit_transaction";

/// Additive v2 method for preparing native or custom-token witnessed escrow effects.
pub const METHOD_PREPARE_WITNESSED_ASSET_ESCROW_V2: &str =
    "lez_bridge.v2.prepare_witnessed_asset_escrow";
/// Additive v2 method for observing exact prepared witnessed-asset effects.
pub const METHOD_OBSERVE_WITNESSED_ASSET_ESCROW_V2: &str =
    "lez_bridge.v2.observe_witnessed_asset_escrow";
/// Additive v2 method for reserving a native or token witnessed-claim transcript.
pub const METHOD_PREPARE_WITNESSED_ASSET_CLAIM_V2: &str =
    "lez_bridge.v2.prepare_witnessed_asset_claim";
/// Additive v2 method for completing a witnessed-asset claim with an aggregate signature.
pub const METHOD_COMPLETE_WITNESSED_ASSET_CLAIM_V2: &str =
    "lez_bridge.v2.complete_witnessed_asset_claim";
/// Additive v2 method for observing one exact finalized witnessed-asset claim.
pub const METHOD_OBSERVE_FINALIZED_WITNESSED_ASSET_CLAIM_V2: &str =
    "lez_bridge.v2.observe_finalized_witnessed_asset_claim";
/// Additive v2 method for preparing a fixed-destination witnessed-asset refund.
pub const METHOD_PREPARE_WITNESSED_ASSET_REFUND_V2: &str =
    "lez_bridge.v2.prepare_witnessed_asset_refund";
/// Additive v2 method for observing witnessed-asset state and refund evidence.
pub const METHOD_OBSERVE_WITNESSED_ASSET_REFUND_V2: &str =
    "lez_bridge.v2.observe_witnessed_asset_refund";
/// Additive v2 method for classifying exact finalized witnessed-asset initialization.
pub const METHOD_CLASSIFY_FINALIZED_WITNESSED_ASSET_INITIALIZATION_V2: &str =
    "lez_bridge.v2.classify_finalized_witnessed_asset_initialization";
/// Additive v2 method for classifying token custody-ATA creation presence.
pub const METHOD_CLASSIFY_FINALIZED_WITNESSED_ASSET_CUSTODY_CREATION_V2: &str =
    "lez_bridge.v2.classify_finalized_witnessed_asset_custody_creation";
/// Additive v2 method for classifying finalized witnessed-asset funding presence.
pub const METHOD_CLASSIFY_FINALIZED_WITNESSED_ASSET_FUNDING_V2: &str =
    "lez_bridge.v2.classify_finalized_witnessed_asset_funding";
/// Additive v2 method for classifying finalized witnessed-asset claim presence.
pub const METHOD_CLASSIFY_FINALIZED_WITNESSED_ASSET_CLAIM_V2: &str =
    "lez_bridge.v2.classify_finalized_witnessed_asset_claim";

/// Additive v3 method for reserving the XMR aggregate-witness claim message.
pub const METHOD_PREPARE_NATIVE_XMR_CLAIM_V3: &str = "lez_bridge.v3.prepare_native_xmr_claim";
/// Additive v3 method for completing an XMR claim with its aggregate signature.
pub const METHOD_COMPLETE_NATIVE_XMR_CLAIM_V3: &str = "lez_bridge.v3.complete_native_xmr_claim";
/// Additive v3 method for reserving the XMR aggregate-witness refund message.
pub const METHOD_PREPARE_NATIVE_XMR_REFUND_V3: &str = "lez_bridge.v3.prepare_native_xmr_refund";
/// Additive v3 method for completing an XMR refund with its aggregate signature.
pub const METHOD_COMPLETE_NATIVE_XMR_REFUND_V3: &str = "lez_bridge.v3.complete_native_xmr_refund";
/// Additive v3 method for preparing the unilateral post-timeout XMR punish transaction.
pub const METHOD_PREPARE_NATIVE_XMR_PUNISH_V3: &str = "lez_bridge.v3.prepare_native_xmr_punish";
/// Additive v3 method for preparing XMR-native initialization and funding.
pub const METHOD_PREPARE_NATIVE_XMR_ESCROW_V3: &str = "lez_bridge.v3.prepare_native_xmr_escrow";
/// Additive v3 method for publishing the committed XMR claim partial.
pub const METHOD_PREPARE_NATIVE_XMR_CLAIM_AUTHORIZATION_V3: &str =
    "lez_bridge.v3.prepare_native_xmr_claim_authorization";
/// Additive v3 method for submitting one exact, durably owned XMR claim authorization.
pub const METHOD_SUBMIT_NATIVE_XMR_CLAIM_AUTHORIZATION_V3: &str =
    "lez_bridge.v3.submit_native_xmr_claim_authorization";
/// Additive v3 method for conservatively classifying one finalized XMR effect.
pub const METHOD_CLASSIFY_FINALIZED_NATIVE_XMR_EFFECT_V3: &str =
    "lez_bridge.v3.classify_finalized_native_xmr_effect";

/// HTTP header binding a connection to one composed run.
pub const RUN_ID_HEADER: &str = "x-lez-bridge-run-id";
/// HTTP header binding a connection to one actor's dedicated sidecar.
pub const SIDECAR_ROLE_HEADER: &str = "x-lez-bridge-sidecar-role";
/// Maximum JSON request or response body at the compatibility boundary.
pub const MAX_RPC_BODY_BYTES: u32 = 5_500_000;

pub use messages::{
    ClassifyFinalizedWitnessedAssetClaimV2Request, ClassifyFinalizedWitnessedAssetClaimV2Result,
    ClassifyFinalizedWitnessedAssetCustodyCreationV2Request,
    ClassifyFinalizedWitnessedAssetCustodyCreationV2Result,
    ClassifyFinalizedWitnessedAssetFundingV2Request,
    ClassifyFinalizedWitnessedAssetFundingV2Result,
    ClassifyFinalizedWitnessedAssetInitializationV2Request,
    ClassifyFinalizedWitnessedAssetInitializationV2Result, ClassifyFinalizedWitnessedClaimResult,
    ClassifyFinalizedWitnessedFundingResult, ClassifyFinalizedWitnessedInitializationRequest,
    ClassifyFinalizedWitnessedInitializationResult, CompleteWitnessedAssetClaimV2Request,
    CompleteWitnessedAssetClaimV2Result, CompleteWitnessedClaimRequest,
    CompleteWitnessedClaimResult, DescribeRuntimeRequest, DescribeRuntimeResult,
    EscrowMetadataFacts, EscrowObservationTarget, EscrowState, FinalizedBlockIdentity,
    FinalizedWitnessedAssetClaimFactsV2, FinalizedWitnessedAssetCustodyCreationFactsV2,
    FinalizedWitnessedAssetFundingFactsV2, FinalizedWitnessedAssetInitializationFactsV2,
    FinalizedWitnessedAssetScanOutcomeV2, FinalizedWitnessedAssetTransactionTargetV2,
    FinalizedWitnessedAssetUnavailableReasonV2, FinalizedWitnessedClaimFacts,
    FinalizedWitnessedClaimObservationTarget, FinalizedWitnessedClaimScanOutcome,
    FinalizedWitnessedFundingFacts, FinalizedWitnessedFundingObservationTarget,
    FinalizedWitnessedFundingScanOutcome, FinalizedWitnessedInitializationFacts,
    FinalizedWitnessedInitializationScanOutcome, FundingFoundFacts, FundingObservation,
    InitializationFoundFacts, InitializationObservation, NativeClaimInstructionFacts,
    NativeCustodyFacts, NativeEscrowAccountFacts, NativeEscrowAccountObservation,
    NativeFundInstructionFacts, NativeInitializeInstructionFacts, NativeRefundFoundFacts,
    NativeRefundInstructionFacts, NativeRefundMetadataFacts, NativeRefundObservation,
    NativeRefundObservationTarget, NativeRefundTerms, ObserveCurrentClockRequest,
    ObserveCurrentClockResult, ObserveEscrowRequest, ObserveEscrowResult,
    ObserveFinalizedClockRequest, ObserveFinalizedClockResult,
    ObserveFinalizedWitnessedAssetClaimV2Request, ObserveFinalizedWitnessedAssetClaimV2Result,
    ObserveFinalizedWitnessedClaimRequest, ObserveFinalizedWitnessedClaimResult,
    ObserveFinalizedWitnessedFundingRequest, ObserveFinalizedWitnessedFundingResult,
    ObserveNativeRefundRequest, ObserveNativeRefundResult, ObserveRevealingClaimRequest,
    ObserveRevealingClaimResult, ObserveWitnessedAssetEscrowV2Request,
    ObserveWitnessedAssetEscrowV2Result, ObserveWitnessedAssetRefundV2Request,
    ObserveWitnessedAssetRefundV2Result, ObserveWitnessedEscrowRequest,
    ObserveWitnessedEscrowResult, ObservedTransactionFacts, PrepareNativeEscrowRequest,
    PrepareNativeEscrowResult, PrepareNativeRefundRequest, PrepareNativeRefundResult,
    PrepareRevealingClaimRequest, PrepareRevealingClaimResult, PrepareWitnessedAssetClaimV2Request,
    PrepareWitnessedAssetClaimV2Result, PrepareWitnessedAssetEscrowV2Request,
    PrepareWitnessedAssetEscrowV2Result, PrepareWitnessedAssetRefundV2Request,
    PrepareWitnessedAssetRefundV2Result, PrepareWitnessedClaimRequest, PrepareWitnessedClaimResult,
    PrepareWitnessedEscrowRequest, PrepareWitnessedEscrowResult, PreparedTransaction,
    PreparedWitnessedClaim, ProtocolErrorReply, RevealingClaimFoundFacts,
    RevealingClaimObservation, RevealingClaimObservationTarget, RuntimeCompatibility,
    RuntimeDescriptor, SubmissionOutcome, SubmitTransactionRequest, SubmitTransactionResult,
    TokenHoldingFactsV2, WitnessedAssetClaimInstructionFactsV2, WitnessedAssetCustodyFactsV2,
    WitnessedAssetEffectInstructionFactsV2, WitnessedAssetInitializationCustodyFactsV2,
    WitnessedAssetObservedPrepareEffectV2, WitnessedAssetPrepareStepV2,
    WitnessedAssetPreparedEffectV2, WitnessedAssetRefundFoundFactsV2,
    WitnessedAssetRefundInstructionFactsV2, WitnessedAssetRefundObservationV2,
    WitnessedClaimInstructionFacts, WitnessedEscrowMetadataFacts, WitnessedFundingFoundFacts,
    WitnessedFundingObservation, WitnessedInitializationFoundFacts,
    WitnessedInitializationObservation, WitnessedNativeInitializeInstructionFacts,
};
pub use primitives::{
    AccountIds, AggregateBip340Signature, ChainClock, ChainPosition, ChainTip, DiscoveryWindow,
    ErrorCode, ErrorMessage, ExactMessageBytes, ExactTransactionBytes, Hex32, MAX_DISCOVERY_BLOCKS,
    MessageContext, NativeAmount, NativeEscrowTerms, NativeEscrowTermsInput, Participant,
    ProtocolValueError, RequestId, RevealingPreimage, RunId, SchemaVersion, TransactionId,
    WITNESSED_LEZ_ASSET_TERMS_VERSION, WitnessedLezAssetTermsV2, WitnessedLezAssetV2,
    WitnessedNativeEscrowTerms, WitnessedNativeEscrowTermsInput, WitnessedTokenEscrowTermsV2,
    WitnessedTokenEscrowTermsV2Input,
};
pub use xmr_v3::{
    ClassifyFinalizedNativeXmrEffectV3Request, ClassifyFinalizedNativeXmrEffectV3Result,
    CompleteNativeXmrClaimV3Request, CompleteNativeXmrClaimV3Result,
    CompleteNativeXmrRefundV3Request, CompleteNativeXmrRefundV3Result,
    CurrentProfileClockAccountSnapshot, FinalizedNativeXmrEffectFactsV3,
    FinalizedNativeXmrScanOutcomeV3, FinalizedNativeXmrTransactionTargetV3,
    FinalizedNativeXmrUnavailableReasonV3, PrepareCurrentProfileClockRequest,
    PrepareCurrentProfileClockResult, PrepareNativeXmrClaimAuthorizationV3Request,
    PrepareNativeXmrClaimAuthorizationV3Result, PrepareNativeXmrClaimV3Request,
    PrepareNativeXmrClaimV3Result, PrepareNativeXmrEscrowV3Request, PrepareNativeXmrEscrowV3Result,
    PrepareNativeXmrPunishV3Request, PrepareNativeXmrPunishV3Result,
    PrepareNativeXmrRefundV3Request, PrepareNativeXmrRefundV3Result,
    SubmitNativeXmrClaimAuthorizationV3Request, SubmitNativeXmrClaimAuthorizationV3Result,
    VerifyCurrentProfileClockRequest, VerifyCurrentProfileClockResult,
    XMR_NATIVE_ESCROW_TERMS_VERSION, XmrClaimPartialV3, XmrNativeEffectV3,
    XmrNativeEscrowMetadataFactsV3, XmrNativeEscrowStateV3, XmrNativeEscrowTermsV3,
    XmrNativeEscrowTermsV3Input, XmrNativeInstructionFactsV3,
};
