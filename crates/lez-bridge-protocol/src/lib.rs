//! Bounded wire types for pinned LEZ runtime compatibility sidecars.
//!
//! The protocol deliberately carries primitive facts. The main process remains
//! responsible for deciding whether those facts prove a swap transition.

#![forbid(unsafe_code)]

mod messages;
mod primitives;

/// Stable JSON-RPC method for runtime and signer identity discovery.
pub const METHOD_DESCRIBE_RUNTIME: &str = "lez_bridge.v1.describe_runtime";
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

/// HTTP header binding a connection to one composed run.
pub const RUN_ID_HEADER: &str = "x-lez-bridge-run-id";
/// HTTP header binding a connection to one actor's dedicated sidecar.
pub const SIDECAR_ROLE_HEADER: &str = "x-lez-bridge-sidecar-role";
/// Maximum JSON request or response body at the compatibility boundary.
pub const MAX_RPC_BODY_BYTES: u32 = 5_500_000;

pub use messages::{
    ClassifyFinalizedWitnessedClaimResult, CompleteWitnessedClaimRequest,
    CompleteWitnessedClaimResult, DescribeRuntimeRequest, DescribeRuntimeResult,
    EscrowMetadataFacts, EscrowObservationTarget, EscrowState, FinalizedBlockIdentity,
    FinalizedWitnessedClaimFacts, FinalizedWitnessedClaimObservationTarget,
    FinalizedWitnessedClaimScanOutcome, FinalizedWitnessedFundingFacts,
    FinalizedWitnessedFundingObservationTarget, FundingFoundFacts, FundingObservation,
    InitializationFoundFacts, InitializationObservation, NativeClaimInstructionFacts,
    NativeCustodyFacts, NativeEscrowAccountFacts, NativeEscrowAccountObservation,
    NativeFundInstructionFacts, NativeInitializeInstructionFacts, NativeRefundFoundFacts,
    NativeRefundInstructionFacts, NativeRefundMetadataFacts, NativeRefundObservation,
    NativeRefundObservationTarget, NativeRefundTerms, ObserveEscrowRequest, ObserveEscrowResult,
    ObserveFinalizedWitnessedClaimRequest, ObserveFinalizedWitnessedClaimResult,
    ObserveFinalizedWitnessedFundingRequest, ObserveFinalizedWitnessedFundingResult,
    ObserveNativeRefundRequest, ObserveNativeRefundResult, ObserveRevealingClaimRequest,
    ObserveRevealingClaimResult, ObserveWitnessedEscrowRequest, ObserveWitnessedEscrowResult,
    ObservedTransactionFacts, PrepareNativeEscrowRequest, PrepareNativeEscrowResult,
    PrepareNativeRefundRequest, PrepareNativeRefundResult, PrepareRevealingClaimRequest,
    PrepareRevealingClaimResult, PrepareWitnessedClaimRequest, PrepareWitnessedClaimResult,
    PrepareWitnessedEscrowRequest, PrepareWitnessedEscrowResult, PreparedTransaction,
    PreparedWitnessedClaim, ProtocolErrorReply, RevealingClaimFoundFacts,
    RevealingClaimObservation, RevealingClaimObservationTarget, RuntimeCompatibility,
    RuntimeDescriptor, SubmissionOutcome, SubmitTransactionRequest, SubmitTransactionResult,
    WitnessedClaimInstructionFacts, WitnessedEscrowMetadataFacts, WitnessedFundingFoundFacts,
    WitnessedFundingObservation, WitnessedInitializationFoundFacts,
    WitnessedInitializationObservation, WitnessedNativeInitializeInstructionFacts,
};
pub use primitives::{
    AccountIds, AggregateBip340Signature, ChainClock, ChainPosition, ChainTip, DiscoveryWindow,
    ErrorCode, ErrorMessage, ExactMessageBytes, ExactTransactionBytes, Hex32, MAX_DISCOVERY_BLOCKS,
    MessageContext, NativeAmount, NativeEscrowTerms, NativeEscrowTermsInput, Participant,
    ProtocolValueError, RequestId, RevealingPreimage, RunId, SchemaVersion, TransactionId,
    WitnessedNativeEscrowTerms, WitnessedNativeEscrowTermsInput,
};
