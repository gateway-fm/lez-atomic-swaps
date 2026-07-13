//! Bounded wire types for the LEZ v0.1.2 compatibility sidecar.
//!
//! The protocol deliberately carries primitive facts. The main process remains
//! responsible for deciding whether those facts prove a swap transition.

#![forbid(unsafe_code)]

mod messages;
mod primitives;

pub use messages::{
    DescribeRuntimeRequest, DescribeRuntimeResult, EscrowMetadataFacts, EscrowObservationTarget,
    EscrowState, FundingFoundFacts, FundingObservation, InitializationFoundFacts,
    InitializationObservation, NativeClaimInstructionFacts, NativeCustodyFacts,
    NativeFundInstructionFacts, NativeInitializeInstructionFacts, ObserveEscrowRequest,
    ObserveEscrowResult, ObserveRevealingClaimRequest, ObserveRevealingClaimResult,
    ObservedTransactionFacts, PrepareNativeEscrowRequest, PrepareNativeEscrowResult,
    PrepareRevealingClaimRequest, PrepareRevealingClaimResult, PreparedTransaction,
    ProtocolErrorReply, RevealingClaimFoundFacts, RevealingClaimObservation,
    RevealingClaimObservationTarget, RuntimeCompatibility, RuntimeDescriptor, SubmissionOutcome,
    SubmitTransactionRequest, SubmitTransactionResult,
};
pub use primitives::{
    AccountIds, ChainPosition, ChainTip, DiscoveryWindow, ErrorCode, ErrorMessage,
    ExactTransactionBytes, Hex32, MAX_DISCOVERY_BLOCKS, MessageContext, NativeAmount,
    NativeEscrowTerms, NativeEscrowTermsInput, Participant, ProtocolValueError, RequestId,
    RevealingPreimage, RunId, SchemaVersion, TransactionId,
};
