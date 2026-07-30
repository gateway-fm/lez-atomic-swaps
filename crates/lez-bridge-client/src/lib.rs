//! Bounded SDK-side client for the isolated official LEZ compatibility sidecar.
//!
//! This client accepts only an explicit loopback IP literal over plain HTTP.
//! `jsonrpsee`'s direct Hyper connector does not follow redirects or consult
//! proxy environment variables. Every call is attempted exactly once: in
//! particular, randomized transaction preparation and submission are never
//! retried after a timeout or transport failure with an unknown outcome.

#![forbid(unsafe_code)]

use std::{
    collections::{HashMap, HashSet},
    fmt,
    net::IpAddr,
    sync::Mutex,
    time::Duration,
};

use jsonrpsee::{
    core::{ClientError, client::ClientT},
    rpc_params,
};
use jsonrpsee_http_client::{HeaderMap, HeaderValue, HttpClient, HttpClientBuilder};
use lez_bridge_protocol::{
    ChainClock, ChainTip, ClassifyFinalizedNativeXmrEffectV3Request,
    ClassifyFinalizedNativeXmrEffectV3Result, ClassifyFinalizedWitnessedAssetClaimV2Request,
    ClassifyFinalizedWitnessedAssetClaimV2Result,
    ClassifyFinalizedWitnessedAssetCustodyCreationV2Request,
    ClassifyFinalizedWitnessedAssetCustodyCreationV2Result,
    ClassifyFinalizedWitnessedAssetFundingV2Request,
    ClassifyFinalizedWitnessedAssetFundingV2Result,
    ClassifyFinalizedWitnessedAssetInitializationV2Request,
    ClassifyFinalizedWitnessedAssetInitializationV2Result, ClassifyFinalizedWitnessedClaimResult,
    ClassifyFinalizedWitnessedFundingResult, ClassifyFinalizedWitnessedInitializationRequest,
    ClassifyFinalizedWitnessedInitializationResult, CompleteNativeXmrClaimV3Request,
    CompleteNativeXmrClaimV3Result, CompleteNativeXmrRefundV3Request,
    CompleteNativeXmrRefundV3Result, CompleteWitnessedAssetClaimV2Request,
    CompleteWitnessedAssetClaimV2Result, CompleteWitnessedClaimRequest,
    CompleteWitnessedClaimResult, DescribeRuntimeRequest, DescribeRuntimeResult, DiscoveryWindow,
    ErrorCode, ErrorMessage, EscrowState, FinalizedNativeXmrScanOutcomeV3,
    FinalizedNativeXmrTransactionTargetV3, FinalizedWitnessedAssetScanOutcomeV2,
    FinalizedWitnessedAssetTransactionTargetV2, FinalizedWitnessedClaimFacts,
    FinalizedWitnessedClaimObservationTarget, FinalizedWitnessedClaimScanOutcome,
    FinalizedWitnessedFundingObservationTarget, FinalizedWitnessedFundingScanOutcome,
    FinalizedWitnessedInitializationFacts, FinalizedWitnessedInitializationScanOutcome,
    MessageContext, NativeRefundObservationTarget, ObserveCurrentClockRequest,
    ObserveCurrentClockResult, ObserveEscrowRequest, ObserveEscrowResult,
    ObserveFinalizedWitnessedAssetClaimV2Request, ObserveFinalizedWitnessedAssetClaimV2Result,
    ObserveFinalizedWitnessedClaimRequest, ObserveFinalizedWitnessedClaimResult,
    ObserveFinalizedWitnessedFundingRequest, ObserveFinalizedWitnessedFundingResult,
    ObserveNativeRefundRequest, ObserveNativeRefundResult, ObserveRevealingClaimRequest,
    ObserveRevealingClaimResult, ObserveWitnessedAssetEscrowV2Request,
    ObserveWitnessedAssetEscrowV2Result, ObserveWitnessedAssetRefundV2Request,
    ObserveWitnessedAssetRefundV2Result, ObserveWitnessedEscrowRequest,
    ObserveWitnessedEscrowResult, Participant, PrepareNativeEscrowRequest,
    PrepareNativeEscrowResult, PrepareNativeRefundRequest, PrepareNativeRefundResult,
    PrepareNativeXmrClaimAuthorizationV3Request, PrepareNativeXmrClaimAuthorizationV3Result,
    PrepareNativeXmrClaimV3Request, PrepareNativeXmrClaimV3Result, PrepareNativeXmrEscrowV3Request,
    PrepareNativeXmrEscrowV3Result, PrepareNativeXmrPunishV3Request,
    PrepareNativeXmrPunishV3Result, PrepareNativeXmrRefundV3Request,
    PrepareNativeXmrRefundV3Result, PrepareRevealingClaimRequest, PrepareRevealingClaimResult,
    PrepareWitnessedAssetClaimV2Request, PrepareWitnessedAssetClaimV2Result,
    PrepareWitnessedAssetEscrowV2Request, PrepareWitnessedAssetEscrowV2Result,
    PrepareWitnessedAssetRefundV2Request, PrepareWitnessedAssetRefundV2Result,
    PrepareWitnessedClaimRequest, PrepareWitnessedClaimResult, PrepareWitnessedEscrowRequest,
    PrepareWitnessedEscrowResult, PreparedTransaction, PreparedWitnessedClaim, ProtocolErrorReply,
    RequestId, RunId, RuntimeDescriptor, SubmitNativeXmrClaimAuthorizationV3Request,
    SubmitNativeXmrClaimAuthorizationV3Result, SubmitTransactionRequest, SubmitTransactionResult,
    WitnessedAssetPreparedEffectV2, WitnessedAssetRefundObservationV2,
    WitnessedEscrowMetadataFacts, WitnessedLezAssetTermsV2, WitnessedLezAssetV2,
    WitnessedNativeEscrowTerms, XmrNativeEffectV3, XmrNativeEscrowTermsV3,
};
pub use lez_bridge_protocol::{
    MAX_RPC_BODY_BYTES, METHOD_CLASSIFY_FINALIZED_NATIVE_XMR_EFFECT_V3,
    METHOD_CLASSIFY_FINALIZED_WITNESSED_ASSET_CLAIM_V2,
    METHOD_CLASSIFY_FINALIZED_WITNESSED_ASSET_CUSTODY_CREATION_V2,
    METHOD_CLASSIFY_FINALIZED_WITNESSED_ASSET_FUNDING_V2,
    METHOD_CLASSIFY_FINALIZED_WITNESSED_ASSET_INITIALIZATION_V2,
    METHOD_CLASSIFY_FINALIZED_WITNESSED_CLAIM, METHOD_CLASSIFY_FINALIZED_WITNESSED_FUNDING,
    METHOD_CLASSIFY_FINALIZED_WITNESSED_INITIALIZATION, METHOD_COMPLETE_NATIVE_XMR_CLAIM_V3,
    METHOD_COMPLETE_NATIVE_XMR_REFUND_V3, METHOD_COMPLETE_WITNESSED_ASSET_CLAIM_V2,
    METHOD_COMPLETE_WITNESSED_CLAIM, METHOD_DESCRIBE_RUNTIME, METHOD_OBSERVE_CURRENT_CLOCK,
    METHOD_OBSERVE_ESCROW, METHOD_OBSERVE_FINALIZED_WITNESSED_ASSET_CLAIM_V2,
    METHOD_OBSERVE_FINALIZED_WITNESSED_CLAIM, METHOD_OBSERVE_FINALIZED_WITNESSED_FUNDING,
    METHOD_OBSERVE_NATIVE_REFUND, METHOD_OBSERVE_REVEALING_CLAIM,
    METHOD_OBSERVE_WITNESSED_ASSET_ESCROW_V2, METHOD_OBSERVE_WITNESSED_ASSET_REFUND_V2,
    METHOD_OBSERVE_WITNESSED_ESCROW, METHOD_PREPARE_NATIVE_ESCROW, METHOD_PREPARE_NATIVE_REFUND,
    METHOD_PREPARE_NATIVE_XMR_CLAIM_AUTHORIZATION_V3, METHOD_PREPARE_NATIVE_XMR_CLAIM_V3,
    METHOD_PREPARE_NATIVE_XMR_ESCROW_V3, METHOD_PREPARE_NATIVE_XMR_PUNISH_V3,
    METHOD_PREPARE_NATIVE_XMR_REFUND_V3, METHOD_PREPARE_REVEALING_CLAIM,
    METHOD_PREPARE_WITNESSED_ASSET_CLAIM_V2, METHOD_PREPARE_WITNESSED_ASSET_ESCROW_V2,
    METHOD_PREPARE_WITNESSED_ASSET_REFUND_V2, METHOD_PREPARE_WITNESSED_CLAIM,
    METHOD_PREPARE_WITNESSED_ESCROW, METHOD_SUBMIT_NATIVE_XMR_CLAIM_AUTHORIZATION_V3,
    METHOD_SUBMIT_TRANSACTION, RUN_ID_HEADER, SIDECAR_ROLE_HEADER,
};
use secp256k1::{
    Message as SecpMessage, Secp256k1, XOnlyPublicKey, schnorr::Signature as SchnorrSignature,
};
use serde::{Serialize, de::DeserializeOwned};
use sha2::{Digest as _, Sha256};
use thiserror::Error;
use url::{Host, Url};
use zeroize::Zeroize;

/// Longest request timeout accepted in client configuration.
pub const MAX_REQUEST_TIMEOUT: Duration = Duration::from_mins(2);
const MAX_EXACT_TRANSACTION_BYTES: usize = 2_000_000;
const OFFICIAL_PUBLIC_MESSAGE_HASH_PREFIX: &[u8; 32] =
    b"/LEE/v0.3/Message/Public/\x00\x00\x00\x00\x00\x00\x00";
const MIN_CAPABILITY_BYTES: usize = 32;
const MAX_CAPABILITY_BYTES: usize = 128;

/// A bearer capability dedicated to one actor sidecar.
///
/// The value is neither cloneable nor exposed through `Debug` or `Display` and
/// is zeroized when dropped. The sensitive HTTP header copy is owned by the
/// transport and is marked sensitive so middleware cannot print it normally.
pub struct SidecarCapability(String);

impl SidecarCapability {
    /// Validates a bounded capability safe for an HTTP bearer header.
    ///
    /// # Errors
    ///
    /// Rejects values outside 32..=128 ASCII bytes or outside the grammar
    /// `[A-Za-z0-9._-]`.
    pub fn new(value: impl Into<String>) -> Result<Self, CapabilityError> {
        let mut value = value.into();
        if (MIN_CAPABILITY_BYTES..=MAX_CAPABILITY_BYTES).contains(&value.len())
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        {
            Ok(Self(value))
        } else {
            value.zeroize();
            Err(CapabilityError)
        }
    }

    fn authorization_header(&self) -> Result<HeaderValue, CapabilityError> {
        let mut bearer = String::with_capacity("Bearer ".len() + self.0.len());
        bearer.push_str("Bearer ");
        bearer.push_str(&self.0);
        let header = HeaderValue::from_str(&bearer).map_err(|_| CapabilityError);
        bearer.zeroize();
        let mut header = header?;
        header.set_sensitive(true);
        Ok(header)
    }
}

impl Drop for SidecarCapability {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

impl fmt::Debug for SidecarCapability {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SidecarCapability([REDACTED])")
    }
}

/// A capability failed its bounded bearer-token grammar.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
#[error("sidecar capability must be 32..=128 ASCII characters from [A-Za-z0-9._-]")]
pub struct CapabilityError;

/// Immutable connection and expected-identity configuration.
pub struct BridgeClientConfig {
    endpoint: String,
    capability: SidecarCapability,
    expected_run_id: RunId,
    expected_runtime: RuntimeDescriptor,
    request_timeout: Duration,
}

impl BridgeClientConfig {
    /// Creates configuration validated when [`BridgeClient::connect`] is called.
    pub fn new(
        endpoint: impl Into<String>,
        capability: SidecarCapability,
        expected_run_id: RunId,
        expected_runtime: RuntimeDescriptor,
        request_timeout: Duration,
    ) -> Self {
        Self {
            endpoint: endpoint.into(),
            capability,
            expected_run_id,
            expected_runtime,
            request_timeout,
        }
    }
}

impl fmt::Debug for BridgeClientConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("BridgeClientConfig")
            .field("endpoint", &self.endpoint)
            .field("capability", &self.capability)
            .field("expected_run_id", &self.expected_run_id)
            .field("expected_runtime", &self.expected_runtime)
            .field("request_timeout", &self.request_timeout)
            .finish()
    }
}

/// One stable operation used for conservative error classification.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use]
pub enum BridgeOperation {
    /// Runtime identity description.
    DescribeRuntime,
    /// Stable current canonical clock observation.
    ObserveCurrentClock,
    /// Randomized native initialization and funding preparation.
    PrepareNativeEscrow,
    /// Aggregate-witness initialization and funding preparation.
    PrepareWitnessedEscrow,
    /// Native initialization and funding observation.
    ObserveEscrow,
    /// Aggregate-witness initialization and funding observation.
    ObserveWitnessedEscrow,
    /// Finalized aggregate-witness funding observation.
    ObserveFinalizedWitnessedFunding,
    /// Finalized aggregate-witness funding found-or-absent classification.
    ClassifyFinalizedWitnessedFunding,
    /// Exact finalized aggregate-witness initialization three-way classification.
    ClassifyFinalizedWitnessedInitialization,
    /// Randomized revealing-claim preparation.
    PrepareRevealingClaim,
    /// Unsigned aggregate-witness message reservation.
    PrepareWitnessedClaim,
    /// Exact aggregate-witness transaction completion.
    CompleteWitnessedClaim,
    /// Exact finalized aggregate-witness claim observation.
    ObserveFinalizedWitnessedClaim,
    /// Exact finalized aggregate-witness claim presence classification.
    ClassifyFinalizedWitnessedClaim,
    /// Revealing-claim observation.
    ObserveRevealingClaim,
    /// Fixed-destination native refund preparation.
    PrepareNativeRefund,
    /// Native escrow state and refund observation.
    ObserveNativeRefund,
    /// Exact transaction submission.
    SubmitTransaction,
    /// Ordered native-or-token witnessed escrow preparation.
    PrepareWitnessedAssetEscrowV2,
    /// Exact ordered witnessed-asset escrow observation.
    ObserveWitnessedAssetEscrowV2,
    /// Unsigned witnessed-asset claim reservation.
    PrepareWitnessedAssetClaimV2,
    /// Exact witnessed-asset claim completion.
    CompleteWitnessedAssetClaimV2,
    /// Exact finalized witnessed-asset claim observation.
    ObserveFinalizedWitnessedAssetClaimV2,
    /// Fixed-destination witnessed-asset refund preparation.
    PrepareWitnessedAssetRefundV2,
    /// Witnessed-asset state and refund observation.
    ObserveWitnessedAssetRefundV2,
    /// Finalized witnessed-asset initialization classification.
    ClassifyFinalizedWitnessedAssetInitializationV2,
    /// Finalized token custody-ATA creation classification.
    ClassifyFinalizedWitnessedAssetCustodyCreationV2,
    /// Finalized witnessed-asset funding classification.
    ClassifyFinalizedWitnessedAssetFundingV2,
    /// Finalized witnessed-asset claim classification.
    ClassifyFinalizedWitnessedAssetClaimV2,
    /// Exact unsigned XMR-native claim preparation.
    PrepareNativeXmrClaimV3,
    /// Exact aggregate-witness XMR-native claim completion.
    CompleteNativeXmrClaimV3,
    /// Exact unsigned XMR-native refund preparation.
    PrepareNativeXmrRefundV3,
    /// Exact aggregate-witness XMR-native refund completion.
    CompleteNativeXmrRefundV3,
    /// Unilateral post-window XMR-native punishment preparation.
    PrepareNativeXmrPunishV3,
    /// Consecutive XMR-native initialization and funding preparation.
    PrepareNativeXmrEscrowV3,
    /// Committed Taker claim-partial publication preparation.
    PrepareNativeXmrClaimAuthorizationV3,
    /// Exact release-service-owned XMR claim-authorization submission.
    SubmitNativeXmrClaimAuthorizationV3,
    /// Conservative finalized XMR-native effect classification.
    ClassifyFinalizedNativeXmrEffectV3,
}

/// Actor-facing finalized witnessed-funding classification.
///
/// These are the only successful outcomes. Node availability, history, finality,
/// moving-tip, malformed evidence, timeout, and transport failures remain typed
/// [`BridgeClientError`] values and can never become `Absent`.
#[derive(Clone, Debug, Eq, PartialEq)]
#[must_use]
pub enum FinalizedWitnessedFundingPresence {
    /// One exact canonical finalized funding effect was found.
    Found {
        /// Echoed operation context.
        context: MessageContext,
        /// Stable finalized clock covering the exact scan.
        finalized_clock: ChainClock,
        /// Exact same-start finalized prefix scanned inside the caller's authorized range.
        scanned_window: DiscoveryWindow,
        /// Complete independently validated canonical funding facts.
        funding: Box<lez_bridge_protocol::FinalizedWitnessedFundingFacts>,
    },
    /// The exact complete stable finalized scan found no matching funding effect.
    Absent {
        /// Echoed operation context.
        context: MessageContext,
        /// Stable finalized clock covering the exact scan.
        finalized_clock: ChainClock,
        /// Complete caller-authorized range; absence is invalid for a strict prefix.
        scanned_window: DiscoveryWindow,
    },
    /// The available finalized prefix did not yet contain the exact funding effect.
    Uncertain {
        /// Echoed operation context.
        context: MessageContext,
        /// Stable finalized clock covering the exact prefix scan.
        finalized_clock: ChainClock,
        /// Exact same-start finalized prefix scanned inside the authorized range.
        scanned_window: DiscoveryWindow,
    },
}

/// Actor-facing exact witnessed-initialization classification.
///
/// `Absent` is only a protocol capability: the sidecar may return it solely
/// when both finalized history and current/pending state affirmatively prove
/// absence. A finalized miss combined with an upstream `UnknownOrPending`
/// lookup is `Uncertain`, never `Absent`. Transport and malformed evidence
/// remain [`BridgeClientError`] values.
#[derive(Clone, Debug, Eq, PartialEq)]
#[must_use]
pub enum FinalizedWitnessedInitializationPresence {
    /// The caller's exact initialization bytes occur in stable finalized ancestry.
    Found {
        /// Echoed operation context.
        context: MessageContext,
        /// Stable finalized clock covering the scan.
        finalized_clock: ChainClock,
        /// Exact same-start finalized prefix scanned inside the caller's authorized range.
        scanned_window: DiscoveryWindow,
        /// Complete independently validated initialization facts.
        initialization: Box<FinalizedWitnessedInitializationFacts>,
    },
    /// Finalized and current/pending observations both affirmatively proved absence.
    Absent {
        /// Echoed operation context.
        context: MessageContext,
        /// Stable finalized clock covering the scan.
        finalized_clock: ChainClock,
        /// Complete caller-authorized range; absence is invalid for a strict prefix.
        scanned_window: DiscoveryWindow,
    },
    /// Finalized history did not contain the exact initialization, but current
    /// upstream evidence could not distinguish pending presence from absence.
    Uncertain {
        /// Echoed operation context.
        context: MessageContext,
        /// Stable finalized clock covering the scan.
        finalized_clock: ChainClock,
        /// Exact same-start finalized prefix scanned inside the caller's authorized range.
        scanned_window: DiscoveryWindow,
    },
}

/// A stable reason why exact claim presence cannot yet be classified.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use]
pub enum FinalizedWitnessedClaimUnavailable {
    /// The node, finalized tip, requested maturity, or bounded history is unavailable.
    NodeFinalityOrHistory,
    /// The finalized tip moved while the sidecar assembled canonical evidence.
    MovingTip,
}

/// A transport-level reason why the observation outcome is ambiguous.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use]
pub enum FinalizedWitnessedClaimUncertain {
    /// The bounded request deadline elapsed before a response was accepted.
    Timeout,
    /// The authenticated loopback transport failed before a response was accepted.
    Transport,
}

/// Actor-facing exact witnessed-claim presence classification.
///
/// Only `NotFound` authorizes an initial submission attempt. It is returned
/// solely from a strict success response proving the caller's exact bounded
/// window was completely scanned under one stable finalized tip. Every node,
/// history, maturity, moving-tip, timeout, and transport failure is distinct
/// and fails closed.
#[derive(Clone, Debug, Eq, PartialEq)]
#[must_use]
pub enum FinalizedWitnessedClaimPresence {
    /// The exact canonical finalized claim is already present.
    PresentExact {
        /// Echoed operation context.
        context: MessageContext,
        /// Stable finalized tip covering the exact scan.
        finalized_tip: ChainTip,
        /// Exact caller-owned bounded range that was scanned.
        scanned_window: DiscoveryWindow,
        /// Complete independently validated canonical claim facts.
        claim: Box<FinalizedWitnessedClaimFacts>,
    },
    /// The exact complete stable finalized scan found no matching claim.
    NotFound {
        /// Echoed operation context.
        context: MessageContext,
        /// Stable finalized tip covering the exact scan.
        finalized_tip: ChainTip,
        /// Exact caller-owned bounded range that was scanned.
        scanned_window: DiscoveryWindow,
    },
    /// Canonical presence cannot currently be classified from node evidence.
    Unavailable(FinalizedWitnessedClaimUnavailable),
    /// Delivery of the read-only classification request is ambiguous.
    Uncertain(FinalizedWitnessedClaimUncertain),
}

impl FinalizedWitnessedClaimPresence {
    /// Returns whether this evidence authorizes the first exact submission attempt.
    ///
    /// This is only the chain-absence precondition: the actor must also hold its
    /// separate durable public-effect CAS send authority. Existing exact presence
    /// reconciles without sending. Every unavailable or uncertain state returns
    /// false and must be observed again using a fresh request context and an
    /// actor-selected current finalized window.
    #[must_use]
    pub const fn authorizes_initial_submission(&self) -> bool {
        matches!(self, Self::NotFound { .. })
    }
}

/// A persisted witnessed-claim artifact is not the exact official message it names.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
#[error("prepared witnessed claim does not bind its exact official message bytes")]
pub struct PreparedWitnessedClaimValidationError;

/// A validated remote error whose free-form message is redacted by formatting.
pub struct RemoteProtocolError(ProtocolErrorReply);

impl RemoteProtocolError {
    /// Returns the stable remote error category.
    pub const fn code(&self) -> ErrorCode {
        self.0.code
    }

    /// Returns the exact echoed context after client validation.
    pub const fn context(&self) -> &MessageContext {
        &self.0.context
    }

    /// Explicitly exposes bounded diagnostic text to callers that need it.
    ///
    /// Normal `Debug` and `Display` formatting always redact this value because
    /// a faulty remote implementation might include sensitive request data.
    pub const fn message(&self) -> &ErrorMessage {
        &self.0.message
    }
}

impl fmt::Debug for RemoteProtocolError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RemoteProtocolError")
            .field("context", &self.0.context)
            .field("code", &self.0.code)
            .field("message", &"[REDACTED]")
            .finish()
    }
}

impl fmt::Display for RemoteProtocolError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "remote LEZ bridge returned error category {:?}",
            self.0.code
        )
    }
}

impl std::error::Error for RemoteProtocolError {}

/// Fail-closed client error classification without sensitive request contents.
#[derive(Debug, Error)]
pub enum BridgeClientError {
    /// Endpoint, timeout, or HTTP client configuration was invalid.
    #[error("invalid LEZ bridge client configuration: {reason}")]
    InvalidConfiguration {
        /// Stable non-sensitive reason.
        reason: ConfigurationError,
    },
    /// Request run, role, or runtime did not match this dedicated client.
    #[error("LEZ bridge request context does not match the dedicated client for {operation:?}")]
    RequestContextMismatch {
        /// Operation rejected before transport.
        operation: BridgeOperation,
    },
    /// The same protocol request ID was reused on this client.
    #[error("LEZ bridge request id was already used for {operation:?}")]
    RequestIdReused {
        /// Operation rejected before transport.
        operation: BridgeOperation,
    },
    /// Response or typed remote-error context did not exactly echo the request.
    #[error("LEZ bridge response context mismatch for {operation:?}")]
    ResponseContextMismatch {
        /// Operation whose response was rejected.
        operation: BridgeOperation,
    },
    /// Runtime description differed from the complete expected identity.
    #[error("LEZ bridge runtime identity mismatch")]
    RuntimeMismatch,
    /// A prepared transaction result violated structural bounds.
    #[error("LEZ bridge returned a malformed prepared transaction for {operation:?}")]
    MalformedPreparedTransaction {
        /// Preparation operation whose result was rejected.
        operation: BridgeOperation,
    },
    /// A finalized observation result contradicted the exact request or itself.
    #[error("LEZ bridge returned a malformed finalized observation for {operation:?}")]
    MalformedObservation {
        /// Observation operation whose result was rejected.
        operation: BridgeOperation,
    },
    /// Submission did not return the exact persisted transaction ID.
    #[error("LEZ bridge submission returned a different transaction id")]
    SubmitTransactionIdMismatch,
    /// A response or remote error was not valid strict protocol JSON.
    #[error("LEZ bridge returned an invalid typed response for {operation:?}")]
    InvalidResponse {
        /// Operation whose response was invalid.
        operation: BridgeOperation,
    },
    /// A finite request timeout elapsed; delivery outcome is unknown.
    #[error("LEZ bridge request timed out for {operation:?}; delivery outcome is unknown")]
    Timeout {
        /// Operation that timed out.
        operation: BridgeOperation,
    },
    /// HTTP or JSON-RPC transport failed; delivery outcome is unknown.
    #[error("LEZ bridge transport failed for {operation:?}; delivery outcome is unknown")]
    Transport {
        /// Operation whose transport failed.
        operation: BridgeOperation,
    },
    /// The sidecar returned a strict typed error with matching context.
    #[error(transparent)]
    Remote(RemoteProtocolError),
}

/// Stable non-sensitive configuration rejection reasons.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum ConfigurationError {
    /// Only exact loopback IP literals with a nonzero explicit port are accepted.
    #[error(
        "endpoint must be explicit loopback-IP HTTP with a nonzero port and no extra URL parts"
    )]
    NonLoopbackEndpoint,
    /// Timeouts must be finite and at most 120 seconds.
    #[error("request timeout must be greater than zero and at most 120 seconds")]
    InvalidTimeout,
    /// Capability could not be encoded in a sensitive bearer header.
    #[error("capability header is invalid")]
    InvalidCapability,
    /// The bounded HTTP client could not be created.
    #[error("bounded HTTP transport could not be built")]
    TransportBuild,
    /// The release-intended client must bind to a Taker runtime.
    #[error("XMR release client requires a Taker runtime")]
    ReleaseClientRequiresTaker,
}

/// Bounded client dedicated to one run, role, runtime, and bearer capability.
pub struct BridgeClient {
    client: HttpClient,
    expected_run_id: RunId,
    expected_runtime: RuntimeDescriptor,
    used_request_ids: Mutex<HashSet<RequestId>>,
}

impl fmt::Debug for BridgeClient {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("BridgeClient")
            .field("expected_run_id", &self.expected_run_id)
            .field("expected_runtime", &self.expected_runtime)
            .field("capability", &"[REDACTED]")
            .finish_non_exhaustive()
    }
}

impl BridgeClient {
    /// Builds a bounded single-flight client without redirects, proxies, or retries.
    ///
    /// # Errors
    ///
    /// Rejects non-loopback or ambiguous URLs, zero or oversized timeouts, an
    /// invalid capability header, or an HTTP transport build failure.
    pub fn connect(config: BridgeClientConfig) -> Result<Self, BridgeClientError> {
        validate_endpoint(&config.endpoint)?;
        if config.request_timeout.is_zero() || config.request_timeout > MAX_REQUEST_TIMEOUT {
            return Err(configuration(ConfigurationError::InvalidTimeout));
        }

        let mut headers = HeaderMap::new();
        headers.insert(
            "authorization",
            config
                .capability
                .authorization_header()
                .map_err(|_| configuration(ConfigurationError::InvalidCapability))?,
        );
        headers.insert(
            RUN_ID_HEADER,
            HeaderValue::from_str(config.expected_run_id.as_str())
                .map_err(|_| configuration(ConfigurationError::TransportBuild))?,
        );
        headers.insert(
            SIDECAR_ROLE_HEADER,
            HeaderValue::from_static(match config.expected_runtime.sidecar_role {
                Participant::Maker => "maker",
                Participant::Taker => "taker",
            }),
        );

        let client = HttpClientBuilder::default()
            .max_request_size(MAX_RPC_BODY_BYTES)
            .max_response_size(MAX_RPC_BODY_BYTES)
            .request_timeout(config.request_timeout)
            .max_concurrent_requests(1)
            .set_headers(headers)
            .build(&config.endpoint)
            .map_err(|_| configuration(ConfigurationError::TransportBuild))?;

        Ok(Self {
            client,
            expected_run_id: config.expected_run_id,
            expected_runtime: config.expected_runtime,
            used_request_ids: Mutex::new(HashSet::new()),
        })
    }

    /// Describes and validates the sidecar's complete runtime identity.
    ///
    /// # Errors
    ///
    /// Fails closed on any context, transport, strict decoding, or runtime mismatch.
    pub async fn describe_runtime(
        &self,
        request: DescribeRuntimeRequest,
    ) -> Result<DescribeRuntimeResult, BridgeClientError> {
        let operation = BridgeOperation::DescribeRuntime;
        let context = request.context.clone();
        self.reserve_context(operation, &context)?;
        let result: DescribeRuntimeResult = self
            .request(operation, METHOD_DESCRIBE_RUNTIME, request, &context)
            .await?;
        Self::validate_response_context(operation, &context, &result.context)?;
        if result.runtime != self.expected_runtime {
            return Err(BridgeClientError::RuntimeMismatch);
        }
        Ok(result)
    }

    /// Reads one stable current canonical clock from the official LEZ node.
    ///
    /// # Errors
    ///
    /// Fails closed on context/runtime drift, request-ID reuse, a zero block
    /// identity or consensus timestamp, timeout, transport uncertainty, strict decoding,
    /// or typed remote error.
    pub async fn observe_current_clock(
        &self,
        request: ObserveCurrentClockRequest,
    ) -> Result<ObserveCurrentClockResult, BridgeClientError> {
        let operation = BridgeOperation::ObserveCurrentClock;
        let context = request.context.clone();
        self.validate_request_runtime(operation, &context, &request.runtime)?;
        self.reserve_context(operation, &context)?;
        let result: ObserveCurrentClockResult = self
            .request(operation, METHOD_OBSERVE_CURRENT_CLOCK, request, &context)
            .await?;
        Self::validate_response_context(operation, &context, &result.context)?;
        if result.runtime != self.expected_runtime
            || result.clock.block_hash.as_bytes() == &[0; 32]
            || result.clock.timestamp_ms == 0
        {
            return Err(BridgeClientError::MalformedObservation { operation });
        }
        Ok(result)
    }

    /// Prepares exact native initialization and funding transactions once.
    ///
    /// # Errors
    ///
    /// Fails closed on context/runtime mismatch, unknown delivery, strict
    /// decoding failure, duplicate transaction IDs/bytes, or protocol error.
    pub async fn prepare_native_escrow(
        &self,
        request: PrepareNativeEscrowRequest,
    ) -> Result<PrepareNativeEscrowResult, BridgeClientError> {
        let operation = BridgeOperation::PrepareNativeEscrow;
        let context = request.context.clone();
        self.validate_request_runtime(operation, &context, &request.runtime)?;
        self.reserve_context(operation, &context)?;
        let result: PrepareNativeEscrowResult = self
            .request(operation, METHOD_PREPARE_NATIVE_ESCROW, request, &context)
            .await?;
        Self::validate_response_context(operation, &context, &result.context)?;
        validate_prepared(operation, &result.initialization)?;
        validate_prepared(operation, &result.funding)?;
        if result.initialization.transaction_id == result.funding.transaction_id
            || result.initialization.exact_bytes == result.funding.exact_bytes
        {
            return Err(BridgeClientError::MalformedPreparedTransaction { operation });
        }
        Ok(result)
    }

    /// Prepares exact witnessed initialization and funding transactions once.
    ///
    /// # Errors
    ///
    /// Fails closed on context/runtime mismatch, unknown delivery, strict
    /// decoding failure, duplicate transaction IDs/bytes, or protocol error.
    pub async fn prepare_witnessed_escrow(
        &self,
        request: PrepareWitnessedEscrowRequest,
    ) -> Result<PrepareWitnessedEscrowResult, BridgeClientError> {
        let operation = BridgeOperation::PrepareWitnessedEscrow;
        let context = request.context.clone();
        self.validate_request_runtime(operation, &context, &request.runtime)?;
        self.reserve_context(operation, &context)?;
        let result: PrepareWitnessedEscrowResult = self
            .request(
                operation,
                METHOD_PREPARE_WITNESSED_ESCROW,
                request,
                &context,
            )
            .await?;
        Self::validate_response_context(operation, &context, &result.context)?;
        validate_prepared(operation, &result.initialization)?;
        validate_prepared(operation, &result.funding)?;
        if result.initialization.transaction_id == result.funding.transaction_id
            || result.initialization.exact_bytes == result.funding.exact_bytes
        {
            return Err(BridgeClientError::MalformedPreparedTransaction { operation });
        }
        Ok(result)
    }

    /// Observes native initialization and funding facts once.
    ///
    /// # Errors
    ///
    /// Fails closed on context/runtime mismatch, unknown delivery, strict
    /// decoding failure, or typed remote error.
    pub async fn observe_escrow(
        &self,
        request: ObserveEscrowRequest,
    ) -> Result<ObserveEscrowResult, BridgeClientError> {
        let operation = BridgeOperation::ObserveEscrow;
        let context = request.context.clone();
        self.validate_request_runtime(operation, &context, &request.runtime)?;
        self.reserve_context(operation, &context)?;
        let result: ObserveEscrowResult = self
            .request(operation, METHOD_OBSERVE_ESCROW, request, &context)
            .await?;
        Self::validate_response_context(operation, &context, &result.context)?;
        Ok(result)
    }

    /// Observes aggregate-witness initialization, funding, and same-tip account effects once.
    ///
    /// # Errors
    ///
    /// Fails closed on context/runtime mismatch, unknown delivery, strict
    /// decoding failure, or typed remote error.
    pub async fn observe_witnessed_escrow(
        &self,
        request: ObserveWitnessedEscrowRequest,
    ) -> Result<ObserveWitnessedEscrowResult, BridgeClientError> {
        let operation = BridgeOperation::ObserveWitnessedEscrow;
        let context = request.context.clone();
        self.validate_request_runtime(operation, &context, &request.runtime)?;
        self.reserve_context(operation, &context)?;
        let result: ObserveWitnessedEscrowResult = self
            .request(
                operation,
                METHOD_OBSERVE_WITNESSED_ESCROW,
                request,
                &context,
            )
            .await?;
        Self::validate_response_context(operation, &context, &result.context)?;
        Ok(result)
    }

    /// Prepares one ordered native or custom-token witnessed escrow plan exactly once.
    ///
    /// # Errors
    ///
    /// Fails closed on context/runtime/terms drift, request-ID reuse, duplicate
    /// or malformed prepared effects, timeout, transport uncertainty, or strict response errors.
    pub async fn prepare_witnessed_asset_escrow_v2(
        &self,
        request: PrepareWitnessedAssetEscrowV2Request,
    ) -> Result<PrepareWitnessedAssetEscrowV2Result, BridgeClientError> {
        let operation = BridgeOperation::PrepareWitnessedAssetEscrowV2;
        let context = request.context.clone();
        let expected_terms = request.terms.clone();
        self.validate_request_runtime(operation, &context, &request.runtime)?;
        validate_asset_operation_role(
            operation,
            &request.runtime,
            &request.terms,
            AssetOperationRole::Depositor,
        )?;
        self.reserve_context(operation, &context)?;
        let result: PrepareWitnessedAssetEscrowV2Result = self
            .request(
                operation,
                METHOD_PREPARE_WITNESSED_ASSET_ESCROW_V2,
                request,
                &context,
            )
            .await?;
        Self::validate_response_context(operation, &context, &result.context)?;
        validate_terms_echo(operation, &expected_terms, &result.terms)?;
        validate_asset_prepared_effects(operation, &result.effects)?;
        Ok(result)
    }

    /// Observes one exact persisted native or token preparation plan exactly once.
    ///
    /// # Errors
    ///
    /// Fails closed on context/runtime/terms drift, substituted prepared bytes or
    /// IDs, malformed ordered observations, timeout, transport, or strict response errors.
    pub async fn observe_witnessed_asset_escrow_v2(
        &self,
        request: ObserveWitnessedAssetEscrowV2Request,
    ) -> Result<ObserveWitnessedAssetEscrowV2Result, BridgeClientError> {
        let operation = BridgeOperation::ObserveWitnessedAssetEscrowV2;
        let context = request.context.clone();
        let expected_terms = request.terms.clone();
        let expected_effects = request.prepared_effects.clone();
        let expected_window = request.window;
        self.validate_request_runtime(operation, &context, &request.runtime)?;
        validate_asset_operation_role(
            operation,
            &request.runtime,
            &request.terms,
            AssetOperationRole::EitherParticipant,
        )?;
        validate_asset_prepared_effects(operation, &request.prepared_effects)?;
        self.reserve_context(operation, &context)?;
        let result: ObserveWitnessedAssetEscrowV2Result = self
            .request(
                operation,
                METHOD_OBSERVE_WITNESSED_ASSET_ESCROW_V2,
                request,
                &context,
            )
            .await?;
        Self::validate_response_context(operation, &context, &result.context)?;
        validate_terms_echo(operation, &expected_terms, &result.terms)?;
        validate_asset_escrow_observation(operation, expected_window, &result)?;
        if result.effects.len() != expected_effects.len()
            || result
                .effects
                .iter()
                .zip(&expected_effects)
                .any(|(actual, expected)| {
                    actual.step != expected.step
                        || actual.transaction.transaction_id != expected.transaction.transaction_id
                        || actual.transaction.exact_bytes != expected.transaction.exact_bytes
                })
        {
            return Err(BridgeClientError::MalformedObservation { operation });
        }
        Ok(result)
    }

    /// Reserves one exact witnessed native-or-token claim transcript exactly once.
    ///
    /// # Errors
    ///
    /// Fails closed on context/runtime/terms drift, malformed message bytes,
    /// timeout, transport uncertainty, or strict response errors.
    pub async fn prepare_witnessed_asset_claim_v2(
        &self,
        request: PrepareWitnessedAssetClaimV2Request,
    ) -> Result<PrepareWitnessedAssetClaimV2Result, BridgeClientError> {
        let operation = BridgeOperation::PrepareWitnessedAssetClaimV2;
        let context = request.context.clone();
        let expected_terms = request.terms.clone();
        self.validate_request_runtime(operation, &context, &request.runtime)?;
        validate_asset_operation_role(
            operation,
            &request.runtime,
            &request.terms,
            AssetOperationRole::Claimant,
        )?;
        self.reserve_context(operation, &context)?;
        let result: PrepareWitnessedAssetClaimV2Result = self
            .request(
                operation,
                METHOD_PREPARE_WITNESSED_ASSET_CLAIM_V2,
                request,
                &context,
            )
            .await?;
        Self::validate_response_context(operation, &context, &result.context)?;
        validate_terms_echo(operation, &expected_terms, &result.terms)?;
        validate_witnessed_preparation(operation, &result.claim)?;
        Ok(result)
    }

    /// Completes one exact witnessed-asset claim transcript exactly once.
    ///
    /// # Errors
    ///
    /// Fails closed on context/runtime/terms drift, malformed transcript or
    /// transaction bytes, timeout, transport uncertainty, or strict response errors.
    pub async fn complete_witnessed_asset_claim_v2(
        &self,
        request: CompleteWitnessedAssetClaimV2Request,
    ) -> Result<CompleteWitnessedAssetClaimV2Result, BridgeClientError> {
        let operation = BridgeOperation::CompleteWitnessedAssetClaimV2;
        let context = request.context.clone();
        let expected_terms = request.terms.clone();
        self.validate_request_runtime(operation, &context, &request.runtime)?;
        validate_asset_operation_role(
            operation,
            &request.runtime,
            &request.terms,
            AssetOperationRole::Claimant,
        )?;
        validate_witnessed_preparation(operation, &request.claim)?;
        self.reserve_context(operation, &context)?;
        let result: CompleteWitnessedAssetClaimV2Result = self
            .request(
                operation,
                METHOD_COMPLETE_WITNESSED_ASSET_CLAIM_V2,
                request,
                &context,
            )
            .await?;
        Self::validate_response_context(operation, &context, &result.context)?;
        validate_terms_echo(operation, &expected_terms, &result.terms)?;
        validate_prepared(operation, &result.claim)?;
        Ok(result)
    }

    /// Observes one exact finalized witnessed-asset claim exactly once.
    ///
    /// # Errors
    ///
    /// Fails closed on context/runtime/terms/transcript drift, incomplete finalized
    /// coverage, substituted transaction identity, timeout, transport, or strict errors.
    pub async fn observe_finalized_witnessed_asset_claim_v2(
        &self,
        request: ObserveFinalizedWitnessedAssetClaimV2Request,
    ) -> Result<ObserveFinalizedWitnessedAssetClaimV2Result, BridgeClientError> {
        let operation = BridgeOperation::ObserveFinalizedWitnessedAssetClaimV2;
        let context = request.context.clone();
        let expected_terms = request.terms.clone();
        let expected_claim = request.claim.clone();
        let expected_transaction_id = match request.target {
            FinalizedWitnessedClaimObservationTarget::Exact {
                claim_transaction_id,
            } => Some(claim_transaction_id),
            FinalizedWitnessedClaimObservationTarget::DiscoverByTerms => None,
        };
        let expected_window = request.window;
        self.validate_request_runtime(operation, &context, &request.runtime)?;
        validate_asset_operation_role(
            operation,
            &request.runtime,
            &request.terms,
            AssetOperationRole::EitherParticipant,
        )?;
        validate_witnessed_preparation(operation, &request.claim)?;
        self.reserve_context(operation, &context)?;
        let result: ObserveFinalizedWitnessedAssetClaimV2Result = self
            .request(
                operation,
                METHOD_OBSERVE_FINALIZED_WITNESSED_ASSET_CLAIM_V2,
                request,
                &context,
            )
            .await?;
        Self::validate_response_context(operation, &context, &result.context)?;
        validate_terms_echo(operation, &expected_terms, &result.terms)?;
        validate_asset_finalized_claim_echo(
            operation,
            expected_transaction_id,
            &expected_claim,
            expected_window,
            result.finalized_tip,
            &result.claim,
        )?;
        Ok(result)
    }

    /// Prepares one fixed-destination witnessed-asset refund exactly once.
    ///
    /// # Errors
    ///
    /// Fails closed on context/runtime/terms drift, malformed prepared bytes,
    /// timeout, transport uncertainty, or strict response errors.
    pub async fn prepare_witnessed_asset_refund_v2(
        &self,
        request: PrepareWitnessedAssetRefundV2Request,
    ) -> Result<PrepareWitnessedAssetRefundV2Result, BridgeClientError> {
        let operation = BridgeOperation::PrepareWitnessedAssetRefundV2;
        let context = request.context.clone();
        let expected_terms = request.terms.clone();
        self.validate_request_runtime(operation, &context, &request.runtime)?;
        validate_asset_operation_role(
            operation,
            &request.runtime,
            &request.terms,
            AssetOperationRole::EitherParticipant,
        )?;
        self.reserve_context(operation, &context)?;
        let result: PrepareWitnessedAssetRefundV2Result = self
            .request(
                operation,
                METHOD_PREPARE_WITNESSED_ASSET_REFUND_V2,
                request,
                &context,
            )
            .await?;
        Self::validate_response_context(operation, &context, &result.context)?;
        validate_terms_echo(operation, &expected_terms, &result.terms)?;
        validate_prepared(operation, &result.refund)?;
        Ok(result)
    }

    /// Observes witnessed-asset state and optional refund evidence exactly once.
    ///
    /// # Errors
    ///
    /// Fails closed on context/runtime/terms drift, malformed account/effect facts,
    /// timeout, transport uncertainty, or strict response errors.
    pub async fn observe_witnessed_asset_refund_v2(
        &self,
        request: ObserveWitnessedAssetRefundV2Request,
    ) -> Result<ObserveWitnessedAssetRefundV2Result, BridgeClientError> {
        let operation = BridgeOperation::ObserveWitnessedAssetRefundV2;
        let context = request.context.clone();
        let expected_terms = request.terms.clone();
        let expected_target = request.target;
        self.validate_request_runtime(operation, &context, &request.runtime)?;
        validate_asset_operation_role(
            operation,
            &request.runtime,
            &request.terms,
            AssetOperationRole::EitherParticipant,
        )?;
        self.reserve_context(operation, &context)?;
        let result: ObserveWitnessedAssetRefundV2Result = self
            .request(
                operation,
                METHOD_OBSERVE_WITNESSED_ASSET_REFUND_V2,
                request,
                &context,
            )
            .await?;
        Self::validate_response_context(operation, &context, &result.context)?;
        validate_terms_echo(operation, &expected_terms, &result.terms)?;
        validate_asset_refund_observation(operation, expected_target, &result)?;
        Ok(result)
    }

    /// Classifies finalized witnessed-asset initialization exactly once.
    ///
    /// # Errors
    ///
    /// Fails closed on request/response echoes, malformed exact bytes, incomplete
    /// stable coverage, timeout, transport uncertainty, or strict response errors.
    pub async fn classify_finalized_witnessed_asset_initialization_v2(
        &self,
        request: ClassifyFinalizedWitnessedAssetInitializationV2Request,
    ) -> Result<ClassifyFinalizedWitnessedAssetInitializationV2Result, BridgeClientError> {
        let operation = BridgeOperation::ClassifyFinalizedWitnessedAssetInitializationV2;
        let context = request.context.clone();
        let expected_terms = request.terms.clone();
        let expected_target = request.target.clone();
        let expected_window = request.window;
        self.validate_request_runtime(operation, &context, &request.runtime)?;
        validate_asset_operation_role(
            operation,
            &request.runtime,
            &request.terms,
            AssetOperationRole::EitherParticipant,
        )?;
        validate_asset_target(operation, &request.target)?;
        self.reserve_context(operation, &context)?;
        let result: ClassifyFinalizedWitnessedAssetInitializationV2Result = self
            .request(
                operation,
                METHOD_CLASSIFY_FINALIZED_WITNESSED_ASSET_INITIALIZATION_V2,
                request,
                &context,
            )
            .await?;
        Self::validate_response_context(operation, &context, &result.context)?;
        validate_asset_classifier_echo(
            operation,
            &expected_terms,
            &expected_target,
            expected_window,
            &result.terms,
            &result.target,
            &result.outcome,
        )?;
        Ok(result)
    }

    /// Classifies finalized custom-token custody-ATA creation exactly once.
    ///
    /// # Errors
    ///
    /// Fails closed on native terms, request/response echoes, malformed exact
    /// bytes, incomplete coverage, timeout, transport, or strict response errors.
    pub async fn classify_finalized_witnessed_asset_custody_creation_v2(
        &self,
        request: ClassifyFinalizedWitnessedAssetCustodyCreationV2Request,
    ) -> Result<ClassifyFinalizedWitnessedAssetCustodyCreationV2Result, BridgeClientError> {
        let operation = BridgeOperation::ClassifyFinalizedWitnessedAssetCustodyCreationV2;
        let context = request.context.clone();
        let expected_terms = request.terms.clone();
        let expected_target = request.target.clone();
        let expected_window = request.window;
        self.validate_request_runtime(operation, &context, &request.runtime)?;
        validate_asset_operation_role(
            operation,
            &request.runtime,
            &request.terms,
            AssetOperationRole::EitherParticipant,
        )?;
        validate_asset_target(operation, &request.target)?;
        self.reserve_context(operation, &context)?;
        let result: ClassifyFinalizedWitnessedAssetCustodyCreationV2Result = self
            .request(
                operation,
                METHOD_CLASSIFY_FINALIZED_WITNESSED_ASSET_CUSTODY_CREATION_V2,
                request,
                &context,
            )
            .await?;
        Self::validate_response_context(operation, &context, &result.context)?;
        validate_asset_classifier_echo(
            operation,
            &expected_terms,
            &expected_target,
            expected_window,
            &result.terms,
            &result.target,
            &result.outcome,
        )?;
        Ok(result)
    }

    /// Classifies finalized witnessed-asset funding exactly once.
    ///
    /// # Errors
    ///
    /// Fails closed on request/response echoes, malformed exact bytes, incomplete
    /// stable coverage, timeout, transport uncertainty, or strict response errors.
    pub async fn classify_finalized_witnessed_asset_funding_v2(
        &self,
        request: ClassifyFinalizedWitnessedAssetFundingV2Request,
    ) -> Result<ClassifyFinalizedWitnessedAssetFundingV2Result, BridgeClientError> {
        let operation = BridgeOperation::ClassifyFinalizedWitnessedAssetFundingV2;
        let context = request.context.clone();
        let expected_terms = request.terms.clone();
        let expected_target = request.target.clone();
        let expected_window = request.window;
        self.validate_request_runtime(operation, &context, &request.runtime)?;
        validate_asset_operation_role(
            operation,
            &request.runtime,
            &request.terms,
            AssetOperationRole::EitherParticipant,
        )?;
        validate_asset_target(operation, &request.target)?;
        self.reserve_context(operation, &context)?;
        let result: ClassifyFinalizedWitnessedAssetFundingV2Result = self
            .request(
                operation,
                METHOD_CLASSIFY_FINALIZED_WITNESSED_ASSET_FUNDING_V2,
                request,
                &context,
            )
            .await?;
        Self::validate_response_context(operation, &context, &result.context)?;
        validate_asset_classifier_echo(
            operation,
            &expected_terms,
            &expected_target,
            expected_window,
            &result.terms,
            &result.target,
            &result.outcome,
        )?;
        Ok(result)
    }

    /// Classifies finalized witnessed-asset claim presence exactly once.
    ///
    /// # Errors
    ///
    /// Fails closed on request/response echoes, malformed transcript/exact bytes,
    /// incomplete stable coverage, timeout, transport, or strict response errors.
    pub async fn classify_finalized_witnessed_asset_claim_v2(
        &self,
        request: ClassifyFinalizedWitnessedAssetClaimV2Request,
    ) -> Result<ClassifyFinalizedWitnessedAssetClaimV2Result, BridgeClientError> {
        let operation = BridgeOperation::ClassifyFinalizedWitnessedAssetClaimV2;
        let context = request.context.clone();
        let expected_terms = request.terms.clone();
        let expected_claim = request.claim.clone();
        let expected_target = request.target.clone();
        let expected_window = request.window;
        self.validate_request_runtime(operation, &context, &request.runtime)?;
        validate_asset_operation_role(
            operation,
            &request.runtime,
            &request.terms,
            AssetOperationRole::EitherParticipant,
        )?;
        validate_witnessed_preparation(operation, &request.claim)?;
        validate_asset_target(operation, &request.target)?;
        self.reserve_context(operation, &context)?;
        let result: ClassifyFinalizedWitnessedAssetClaimV2Result = self
            .request(
                operation,
                METHOD_CLASSIFY_FINALIZED_WITNESSED_ASSET_CLAIM_V2,
                request,
                &context,
            )
            .await?;
        Self::validate_response_context(operation, &context, &result.context)?;
        validate_asset_classifier_echo(
            operation,
            &expected_terms,
            &expected_target,
            expected_window,
            &result.terms,
            &result.target,
            &result.outcome,
        )?;
        if result.claim != expected_claim {
            return Err(BridgeClientError::MalformedObservation { operation });
        }
        Ok(result)
    }

    /// Classifies witnessed funding as exact found or affirmative finalized absence.
    ///
    /// # Errors
    ///
    /// Fails closed with typed errors on context/runtime drift, request-ID reuse,
    /// malformed or substituted evidence, unavailable history/finality, moving tip,
    /// timeout, transport uncertainty, or typed remote errors.
    pub async fn classify_finalized_witnessed_funding(
        &self,
        request: ObserveFinalizedWitnessedFundingRequest,
    ) -> Result<FinalizedWitnessedFundingPresence, BridgeClientError> {
        let operation = BridgeOperation::ClassifyFinalizedWitnessedFunding;
        let context = request.context.clone();
        let expected_transaction_id = match request.target {
            FinalizedWitnessedFundingObservationTarget::Exact {
                funding_transaction_id,
            } => Some(funding_transaction_id),
            FinalizedWitnessedFundingObservationTarget::DiscoverByTerms => None,
        };
        let expected_terms = request.terms.clone();
        let expected_program = request.runtime.escrow_program_id;
        let expected_window = request.window;
        self.validate_request_runtime(operation, &context, &request.runtime)?;
        let expected_observer = if request.runtime.sidecar_role == expected_terms.depositor() {
            Some(expected_terms.depositor_account_id())
        } else if request.runtime.sidecar_role == expected_terms.claimant() {
            Some(expected_terms.claimant_account_id())
        } else {
            None
        };
        if expected_observer != Some(request.runtime.signer_account_id) {
            return Err(BridgeClientError::MalformedObservation { operation });
        }
        self.reserve_context(operation, &context)?;
        let result: ClassifyFinalizedWitnessedFundingResult = self
            .request(
                operation,
                METHOD_CLASSIFY_FINALIZED_WITNESSED_FUNDING,
                request,
                &context,
            )
            .await?;
        Self::validate_response_context(operation, &context, &result.context)?;
        let (window_start, window_end, window_complete) = validate_scanned_prefix(
            operation,
            expected_window,
            result.scanned_window,
            result.finalized_clock,
        )?;
        match result.outcome {
            FinalizedWitnessedFundingScanOutcome::Found { funding } => {
                validate_finalized_witnessed_funding_facts(
                    operation,
                    expected_transaction_id,
                    &expected_terms,
                    expected_program,
                    window_start,
                    window_end,
                    ChainTip::new(
                        result.finalized_clock.block_hash,
                        result.finalized_clock.height,
                    ),
                    funding.as_ref(),
                )?;
                Ok(FinalizedWitnessedFundingPresence::Found {
                    context: result.context,
                    finalized_clock: result.finalized_clock,
                    scanned_window: result.scanned_window,
                    funding,
                })
            }
            FinalizedWitnessedFundingScanOutcome::Absent {} => {
                if !window_complete {
                    return Err(BridgeClientError::MalformedObservation { operation });
                }
                Ok(FinalizedWitnessedFundingPresence::Absent {
                    context: result.context,
                    finalized_clock: result.finalized_clock,
                    scanned_window: result.scanned_window,
                })
            }
            FinalizedWitnessedFundingScanOutcome::Uncertain {} => {
                if window_complete {
                    return Err(BridgeClientError::MalformedObservation { operation });
                }
                Ok(FinalizedWitnessedFundingPresence::Uncertain {
                    context: result.context,
                    finalized_clock: result.finalized_clock,
                    scanned_window: result.scanned_window,
                })
            }
        }
    }

    /// Classifies one exact witnessed initialization in stable finalized history.
    ///
    /// A successful `Absent` is accepted only as the sidecar's explicit typed
    /// assertion that finalized and current/pending evidence both proved the
    /// exact transaction absent. `Uncertain`, transport failure, and malformed
    /// evidence never become absence or send authority.
    ///
    /// # Errors
    ///
    /// Fails closed on context/runtime drift, request-ID reuse, substituted
    /// bytes or identity, decoded signer/instruction/account mismatch, unstable
    /// or incomplete finality, timeout, transport uncertainty, or typed remote
    /// error.
    pub async fn classify_finalized_witnessed_initialization(
        &self,
        request: ClassifyFinalizedWitnessedInitializationRequest,
    ) -> Result<FinalizedWitnessedInitializationPresence, BridgeClientError> {
        let operation = BridgeOperation::ClassifyFinalizedWitnessedInitialization;
        let context = request.context.clone();
        let expected_transaction_id = request.initialization.transaction_id;
        let expected_transaction_bytes = request.initialization.exact_bytes.clone();
        let expected_terms = request.terms.clone();
        let expected_program = request.runtime.escrow_program_id;
        let expected_window = request.window;
        self.validate_request_runtime(operation, &context, &request.runtime)?;
        if request.runtime.sidecar_role != expected_terms.depositor()
            || request.runtime.signer_account_id != expected_terms.depositor_account_id()
        {
            return Err(BridgeClientError::MalformedObservation { operation });
        }
        validate_prepared(operation, &request.initialization)?;
        self.reserve_context(operation, &context)?;
        let result: ClassifyFinalizedWitnessedInitializationResult = self
            .request(
                operation,
                METHOD_CLASSIFY_FINALIZED_WITNESSED_INITIALIZATION,
                request,
                &context,
            )
            .await?;
        Self::validate_response_context(operation, &context, &result.context)?;
        let (window_start, window_end, window_complete) = validate_scanned_prefix(
            operation,
            expected_window,
            result.scanned_window,
            result.finalized_clock,
        )?;
        match result.outcome {
            FinalizedWitnessedInitializationScanOutcome::Found { initialization } => {
                validate_finalized_witnessed_initialization_facts(
                    operation,
                    expected_transaction_id,
                    &expected_transaction_bytes,
                    &expected_terms,
                    expected_program,
                    window_start,
                    window_end,
                    ChainTip::new(
                        result.finalized_clock.block_hash,
                        result.finalized_clock.height,
                    ),
                    initialization.as_ref(),
                )?;
                Ok(FinalizedWitnessedInitializationPresence::Found {
                    context: result.context,
                    finalized_clock: result.finalized_clock,
                    scanned_window: result.scanned_window,
                    initialization,
                })
            }
            FinalizedWitnessedInitializationScanOutcome::Absent {} => {
                if !window_complete {
                    return Err(BridgeClientError::MalformedObservation { operation });
                }
                Ok(FinalizedWitnessedInitializationPresence::Absent {
                    context: result.context,
                    finalized_clock: result.finalized_clock,
                    scanned_window: result.scanned_window,
                })
            }
            FinalizedWitnessedInitializationScanOutcome::Uncertain {} => {
                Ok(FinalizedWitnessedInitializationPresence::Uncertain {
                    context: result.context,
                    finalized_clock: result.finalized_clock,
                    scanned_window: result.scanned_window,
                })
            }
        }
    }

    /// Observes one witnessed funding effect in a stable finalized indexer window.
    ///
    /// # Errors
    ///
    /// Fails closed on context/runtime drift, request-ID reuse, strict decoding,
    /// transaction/terms/role mismatch, incoherent block identity, incomplete
    /// finalized-tip coverage, timeout, transport uncertainty, or typed remote error.
    pub async fn observe_finalized_witnessed_funding(
        &self,
        request: ObserveFinalizedWitnessedFundingRequest,
    ) -> Result<ObserveFinalizedWitnessedFundingResult, BridgeClientError> {
        let operation = BridgeOperation::ObserveFinalizedWitnessedFunding;
        let context = request.context.clone();
        let expected_transaction_id = match request.target {
            FinalizedWitnessedFundingObservationTarget::Exact {
                funding_transaction_id,
            } => Some(funding_transaction_id),
            FinalizedWitnessedFundingObservationTarget::DiscoverByTerms => None,
        };
        let expected_terms = request.terms.clone();
        let expected_program = request.runtime.escrow_program_id;
        let window_start = request.window.start_height();
        let window_end = window_start
            .checked_add(u64::from(request.window.max_blocks() - 1))
            .ok_or(BridgeClientError::MalformedObservation { operation })?;
        self.validate_request_runtime(operation, &context, &request.runtime)?;
        let expected_observer = if request.runtime.sidecar_role == expected_terms.depositor() {
            Some(expected_terms.depositor_account_id())
        } else if request.runtime.sidecar_role == expected_terms.claimant() {
            Some(expected_terms.claimant_account_id())
        } else {
            None
        };
        if expected_observer != Some(request.runtime.signer_account_id) {
            return Err(BridgeClientError::MalformedObservation { operation });
        }
        self.reserve_context(operation, &context)?;
        let result: ObserveFinalizedWitnessedFundingResult = self
            .request(
                operation,
                METHOD_OBSERVE_FINALIZED_WITNESSED_FUNDING,
                request,
                &context,
            )
            .await?;
        Self::validate_response_context(operation, &context, &result.context)?;
        validate_finalized_witnessed_funding_facts(
            operation,
            expected_transaction_id,
            &expected_terms,
            expected_program,
            window_start,
            window_end,
            result.finalized_tip,
            &result.funding,
        )?;
        Ok(result)
    }

    /// Prepares one exact preimage-revealing claim without retries.
    ///
    /// # Errors
    ///
    /// Fails closed on context/runtime mismatch, unknown delivery, malformed
    /// prepared bytes, strict decoding failure, or typed remote error.
    pub async fn prepare_revealing_claim(
        &self,
        request: PrepareRevealingClaimRequest,
    ) -> Result<PrepareRevealingClaimResult, BridgeClientError> {
        let operation = BridgeOperation::PrepareRevealingClaim;
        let context = request.context.clone();
        self.validate_request_runtime(operation, &context, &request.runtime)?;
        self.reserve_context(operation, &context)?;
        let result: PrepareRevealingClaimResult = self
            .request(operation, METHOD_PREPARE_REVEALING_CLAIM, request, &context)
            .await?;
        Self::validate_response_context(operation, &context, &result.context)?;
        validate_prepared(operation, &result.claim)?;
        Ok(result)
    }

    /// Reserves one exact unsigned LEZ message for external aggregate signing.
    ///
    /// # Errors
    ///
    /// Fails closed on context/runtime drift, request-ID reuse, malformed
    /// message bytes, typed remote error, timeout, or transport uncertainty.
    pub async fn prepare_witnessed_claim(
        &self,
        request: PrepareWitnessedClaimRequest,
    ) -> Result<PrepareWitnessedClaimResult, BridgeClientError> {
        let operation = BridgeOperation::PrepareWitnessedClaim;
        let context = request.context.clone();
        self.validate_request_runtime(operation, &context, &request.runtime)?;
        self.reserve_context(operation, &context)?;
        let result: PrepareWitnessedClaimResult = self
            .request(operation, METHOD_PREPARE_WITNESSED_CLAIM, request, &context)
            .await?;
        Self::validate_response_context(operation, &context, &result.context)?;
        validate_witnessed_preparation(operation, &result.claim)?;
        Ok(result)
    }

    /// Completes a reserved message with one external aggregate signature.
    ///
    /// The returned exact transaction remains inspectable and must be passed
    /// separately to [`Self::submit_transaction`], preserving durable
    /// persist-before-effect and unknown-outcome semantics.
    ///
    /// # Errors
    ///
    /// Fails closed on context/runtime drift, request-ID reuse, malformed
    /// completed bytes, typed remote error, timeout, or transport uncertainty.
    pub async fn complete_witnessed_claim(
        &self,
        request: CompleteWitnessedClaimRequest,
    ) -> Result<CompleteWitnessedClaimResult, BridgeClientError> {
        let operation = BridgeOperation::CompleteWitnessedClaim;
        let context = request.context.clone();
        self.validate_request_runtime(operation, &context, &request.runtime)?;
        validate_witnessed_preparation(operation, &request.claim)?;
        self.reserve_context(operation, &context)?;
        let result: CompleteWitnessedClaimResult = self
            .request(
                operation,
                METHOD_COMPLETE_WITNESSED_CLAIM,
                request,
                &context,
            )
            .await?;
        Self::validate_response_context(operation, &context, &result.context)?;
        validate_prepared(operation, &result.claim)?;
        Ok(result)
    }

    /// Classifies exact witnessed-claim presence in one caller-owned finalized window.
    ///
    /// The caller may choose a later fresh window on a later poll; this method
    /// never reuses or widens the funding-observation window. Only a strict
    /// `NotFound` success covering the exact requested range authorizes an
    /// initial submission attempt through
    /// [`FinalizedWitnessedClaimPresence::authorizes_initial_submission`].
    ///
    /// # Errors
    ///
    /// Rejects request/runtime drift, request-ID reuse, malformed or substituted
    /// response evidence, invalid exact claim facts, and nonavailability remote
    /// error categories. Expected node/finality/history failures and ambiguous
    /// local delivery are returned as typed non-authorizing presence states.
    pub async fn classify_finalized_witnessed_claim(
        &self,
        request: ObserveFinalizedWitnessedClaimRequest,
    ) -> Result<FinalizedWitnessedClaimPresence, BridgeClientError> {
        let operation = BridgeOperation::ClassifyFinalizedWitnessedClaim;
        let context = request.context.clone();
        let expected_transaction_id = match request.target {
            FinalizedWitnessedClaimObservationTarget::Exact {
                claim_transaction_id,
            } => Some(claim_transaction_id),
            FinalizedWitnessedClaimObservationTarget::DiscoverByTerms => None,
        };
        let expected_claim = request.claim.clone();
        let expected_terms = request.terms.clone();
        let expected_program = request.runtime.escrow_program_id;
        let expected_window = request.window;
        let window_start = expected_window.start_height();
        let window_end = window_start
            .checked_add(u64::from(expected_window.max_blocks() - 1))
            .ok_or(BridgeClientError::MalformedObservation { operation })?;
        self.validate_request_runtime(operation, &context, &request.runtime)?;
        validate_witnessed_preparation(operation, &request.claim)?;
        self.reserve_context(operation, &context)?;
        let result: ClassifyFinalizedWitnessedClaimResult = match self
            .request(
                operation,
                METHOD_CLASSIFY_FINALIZED_WITNESSED_CLAIM,
                request,
                &context,
            )
            .await
        {
            Ok(result) => result,
            Err(BridgeClientError::Remote(remote)) => {
                return match remote.code() {
                    ErrorCode::Unavailable => Ok(FinalizedWitnessedClaimPresence::Unavailable(
                        FinalizedWitnessedClaimUnavailable::NodeFinalityOrHistory,
                    )),
                    ErrorCode::MovingTip => Ok(FinalizedWitnessedClaimPresence::Unavailable(
                        FinalizedWitnessedClaimUnavailable::MovingTip,
                    )),
                    _ => Err(BridgeClientError::Remote(remote)),
                };
            }
            Err(BridgeClientError::Timeout { .. }) => {
                return Ok(FinalizedWitnessedClaimPresence::Uncertain(
                    FinalizedWitnessedClaimUncertain::Timeout,
                ));
            }
            Err(BridgeClientError::Transport { .. }) => {
                return Ok(FinalizedWitnessedClaimPresence::Uncertain(
                    FinalizedWitnessedClaimUncertain::Transport,
                ));
            }
            Err(error) => return Err(error),
        };
        Self::validate_response_context(operation, &context, &result.context)?;
        if result.scanned_window != expected_window || window_end > result.finalized_tip.height {
            return Err(BridgeClientError::MalformedObservation { operation });
        }
        match result.outcome {
            FinalizedWitnessedClaimScanOutcome::PresentExact { claim } => {
                validate_finalized_witnessed_claim_facts(
                    operation,
                    expected_transaction_id,
                    &expected_claim,
                    &expected_terms,
                    expected_program,
                    window_start,
                    window_end,
                    result.finalized_tip,
                    claim.as_ref(),
                )?;
                Ok(FinalizedWitnessedClaimPresence::PresentExact {
                    context: result.context,
                    finalized_tip: result.finalized_tip,
                    scanned_window: result.scanned_window,
                    claim,
                })
            }
            FinalizedWitnessedClaimScanOutcome::NotFound => {
                Ok(FinalizedWitnessedClaimPresence::NotFound {
                    context: result.context,
                    finalized_tip: result.finalized_tip,
                    scanned_window: result.scanned_window,
                })
            }
        }
    }

    /// Observes one exact aggregate-witness claim in a stable finalized indexer window.
    ///
    /// # Errors
    ///
    /// Fails closed on context/runtime drift, request-ID reuse, strict decoding,
    /// transaction/message/role mismatch, incoherent block identity, incomplete
    /// finalized-tip coverage, timeout, transport uncertainty, or typed remote error.
    pub async fn observe_finalized_witnessed_claim(
        &self,
        request: ObserveFinalizedWitnessedClaimRequest,
    ) -> Result<ObserveFinalizedWitnessedClaimResult, BridgeClientError> {
        let operation = BridgeOperation::ObserveFinalizedWitnessedClaim;
        let context = request.context.clone();
        let expected_transaction_id = match request.target {
            FinalizedWitnessedClaimObservationTarget::Exact {
                claim_transaction_id,
            } => Some(claim_transaction_id),
            FinalizedWitnessedClaimObservationTarget::DiscoverByTerms => None,
        };
        let expected_claim = request.claim.clone();
        let expected_terms = request.terms.clone();
        let expected_program = request.runtime.escrow_program_id;
        let window_start = request.window.start_height();
        let window_end = window_start
            .checked_add(u64::from(request.window.max_blocks() - 1))
            .ok_or(BridgeClientError::MalformedObservation { operation })?;
        self.validate_request_runtime(operation, &context, &request.runtime)?;
        validate_witnessed_preparation(operation, &request.claim)?;
        self.reserve_context(operation, &context)?;
        let result: ObserveFinalizedWitnessedClaimResult = self
            .request(
                operation,
                METHOD_OBSERVE_FINALIZED_WITNESSED_CLAIM,
                request,
                &context,
            )
            .await?;
        Self::validate_response_context(operation, &context, &result.context)?;
        validate_finalized_witnessed_claim_facts(
            operation,
            expected_transaction_id,
            &expected_claim,
            &expected_terms,
            expected_program,
            window_start,
            window_end,
            result.finalized_tip,
            &result.claim,
        )?;
        Ok(result)
    }

    /// Observes one exact or terms-discovered revealing claim.
    ///
    /// # Errors
    ///
    /// Fails closed on context/runtime mismatch, unknown delivery, strict
    /// decoding failure, or typed remote error.
    pub async fn observe_revealing_claim(
        &self,
        request: ObserveRevealingClaimRequest,
    ) -> Result<ObserveRevealingClaimResult, BridgeClientError> {
        let operation = BridgeOperation::ObserveRevealingClaim;
        let context = request.context.clone();
        self.validate_request_runtime(operation, &context, &request.runtime)?;
        self.reserve_context(operation, &context)?;
        let result: ObserveRevealingClaimResult = self
            .request(operation, METHOD_OBSERVE_REVEALING_CLAIM, request, &context)
            .await?;
        Self::validate_response_context(operation, &context, &result.context)?;
        Ok(result)
    }

    /// Prepares one exact fixed-destination native refund without retries.
    ///
    /// # Errors
    ///
    /// Fails closed on context/runtime mismatch, unknown delivery, malformed
    /// prepared bytes, strict decoding failure, or typed remote error.
    pub async fn prepare_native_refund(
        &self,
        request: PrepareNativeRefundRequest,
    ) -> Result<PrepareNativeRefundResult, BridgeClientError> {
        let operation = BridgeOperation::PrepareNativeRefund;
        let context = request.context.clone();
        self.validate_request_runtime(operation, &context, &request.runtime)?;
        self.reserve_context(operation, &context)?;
        let result: PrepareNativeRefundResult = self
            .request(operation, METHOD_PREPARE_NATIVE_REFUND, request, &context)
            .await?;
        Self::validate_response_context(operation, &context, &result.context)?;
        validate_prepared(operation, &result.refund)?;
        Ok(result)
    }

    /// Prepares the exact unsigned XMR-native claim once.
    ///
    /// # Errors
    ///
    /// Fails closed unless the dedicated Maker runtime, agreement, response
    /// echo, and returned official message are all exact.
    pub async fn prepare_native_xmr_claim_v3(
        &self,
        request: PrepareNativeXmrClaimV3Request,
    ) -> Result<PrepareNativeXmrClaimV3Result, BridgeClientError> {
        let operation = BridgeOperation::PrepareNativeXmrClaimV3;
        let context = request.context.clone();
        let expected_terms = request.terms;
        self.validate_request_runtime(operation, &context, &request.runtime)?;
        validate_xmr_request_binding(
            operation,
            &context,
            &request.runtime,
            &request.terms,
            XmrOperationRole::Maker,
        )?;
        self.reserve_context(operation, &context)?;
        let result: PrepareNativeXmrClaimV3Result = self
            .request(
                operation,
                METHOD_PREPARE_NATIVE_XMR_CLAIM_V3,
                request,
                &context,
            )
            .await?;
        Self::validate_response_context(operation, &context, &result.context)?;
        validate_xmr_terms_echo(operation, &expected_terms, &result.terms)?;
        validate_witnessed_preparation(operation, &result.claim)?;
        Ok(result)
    }

    /// Completes the exact XMR-native claim once.
    ///
    /// # Errors
    ///
    /// Fails closed unless the Maker runtime, retained unsigned transcript,
    /// response echo, and completed transaction are structurally exact.
    pub async fn complete_native_xmr_claim_v3(
        &self,
        request: CompleteNativeXmrClaimV3Request,
    ) -> Result<CompleteNativeXmrClaimV3Result, BridgeClientError> {
        let operation = BridgeOperation::CompleteNativeXmrClaimV3;
        let context = request.context.clone();
        let expected_terms = request.terms;
        self.validate_request_runtime(operation, &context, &request.runtime)?;
        validate_xmr_request_binding(
            operation,
            &context,
            &request.runtime,
            &request.terms,
            XmrOperationRole::Maker,
        )?;
        validate_witnessed_preparation(operation, &request.claim)?;
        self.reserve_context(operation, &context)?;
        let result: CompleteNativeXmrClaimV3Result = self
            .request(
                operation,
                METHOD_COMPLETE_NATIVE_XMR_CLAIM_V3,
                request,
                &context,
            )
            .await?;
        Self::validate_response_context(operation, &context, &result.context)?;
        validate_xmr_terms_echo(operation, &expected_terms, &result.terms)?;
        validate_prepared(operation, &result.claim)?;
        Ok(result)
    }

    /// Prepares the exact unsigned XMR-native refund once.
    ///
    /// # Errors
    ///
    /// Fails closed unless the dedicated Taker runtime, agreement, response
    /// echo, and returned official message are all exact.
    pub async fn prepare_native_xmr_refund_v3(
        &self,
        request: PrepareNativeXmrRefundV3Request,
    ) -> Result<PrepareNativeXmrRefundV3Result, BridgeClientError> {
        let operation = BridgeOperation::PrepareNativeXmrRefundV3;
        let context = request.context.clone();
        let expected_terms = request.terms;
        self.validate_request_runtime(operation, &context, &request.runtime)?;
        validate_xmr_request_binding(
            operation,
            &context,
            &request.runtime,
            &request.terms,
            XmrOperationRole::Taker,
        )?;
        self.reserve_context(operation, &context)?;
        let result: PrepareNativeXmrRefundV3Result = self
            .request(
                operation,
                METHOD_PREPARE_NATIVE_XMR_REFUND_V3,
                request,
                &context,
            )
            .await?;
        Self::validate_response_context(operation, &context, &result.context)?;
        validate_xmr_terms_echo(operation, &expected_terms, &result.terms)?;
        validate_witnessed_preparation(operation, &result.refund)?;
        Ok(result)
    }

    /// Completes the exact XMR-native refund once.
    ///
    /// # Errors
    ///
    /// Fails closed unless the Taker runtime, retained unsigned transcript,
    /// response echo, and completed transaction are structurally exact.
    pub async fn complete_native_xmr_refund_v3(
        &self,
        request: CompleteNativeXmrRefundV3Request,
    ) -> Result<CompleteNativeXmrRefundV3Result, BridgeClientError> {
        let operation = BridgeOperation::CompleteNativeXmrRefundV3;
        let context = request.context.clone();
        let expected_terms = request.terms;
        self.validate_request_runtime(operation, &context, &request.runtime)?;
        validate_xmr_request_binding(
            operation,
            &context,
            &request.runtime,
            &request.terms,
            XmrOperationRole::Taker,
        )?;
        validate_witnessed_preparation(operation, &request.refund)?;
        self.reserve_context(operation, &context)?;
        let result: CompleteNativeXmrRefundV3Result = self
            .request(
                operation,
                METHOD_COMPLETE_NATIVE_XMR_REFUND_V3,
                request,
                &context,
            )
            .await?;
        Self::validate_response_context(operation, &context, &result.context)?;
        validate_xmr_terms_echo(operation, &expected_terms, &result.terms)?;
        validate_prepared(operation, &result.refund)?;
        Ok(result)
    }

    /// Prepares the unilateral post-window XMR-native punishment once.
    ///
    /// # Errors
    ///
    /// Fails closed unless the dedicated Maker runtime, agreement, response
    /// echo, and returned transaction are all exact.
    pub async fn prepare_native_xmr_punish_v3(
        &self,
        request: PrepareNativeXmrPunishV3Request,
    ) -> Result<PrepareNativeXmrPunishV3Result, BridgeClientError> {
        let operation = BridgeOperation::PrepareNativeXmrPunishV3;
        let context = request.context.clone();
        let expected_terms = request.terms;
        self.validate_request_runtime(operation, &context, &request.runtime)?;
        validate_xmr_request_binding(
            operation,
            &context,
            &request.runtime,
            &request.terms,
            XmrOperationRole::Maker,
        )?;
        self.reserve_context(operation, &context)?;
        let result: PrepareNativeXmrPunishV3Result = self
            .request(
                operation,
                METHOD_PREPARE_NATIVE_XMR_PUNISH_V3,
                request,
                &context,
            )
            .await?;
        Self::validate_response_context(operation, &context, &result.context)?;
        validate_xmr_terms_echo(operation, &expected_terms, &result.terms)?;
        validate_prepared(operation, &result.punish)?;
        Ok(result)
    }

    /// Prepares consecutive XMR-native initialization and funding once.
    ///
    /// # Errors
    ///
    /// Fails closed unless the Taker runtime and agreement are exact and the
    /// two returned transactions are nonempty and distinct.
    pub async fn prepare_native_xmr_escrow_v3(
        &self,
        request: PrepareNativeXmrEscrowV3Request,
    ) -> Result<PrepareNativeXmrEscrowV3Result, BridgeClientError> {
        let operation = BridgeOperation::PrepareNativeXmrEscrowV3;
        let context = request.context.clone();
        let expected_terms = request.terms;
        self.validate_request_runtime(operation, &context, &request.runtime)?;
        validate_xmr_request_binding(
            operation,
            &context,
            &request.runtime,
            &request.terms,
            XmrOperationRole::Taker,
        )?;
        self.reserve_context(operation, &context)?;
        let result: PrepareNativeXmrEscrowV3Result = self
            .request(
                operation,
                METHOD_PREPARE_NATIVE_XMR_ESCROW_V3,
                request,
                &context,
            )
            .await?;
        Self::validate_response_context(operation, &context, &result.context)?;
        validate_xmr_terms_echo(operation, &expected_terms, &result.terms)?;
        validate_prepared(operation, &result.initialization)?;
        validate_prepared(operation, &result.funding)?;
        if result.initialization.transaction_id == result.funding.transaction_id
            || result.initialization.exact_bytes == result.funding.exact_bytes
        {
            return Err(BridgeClientError::MalformedPreparedTransaction { operation });
        }
        Ok(result)
    }

    /// Prepares publication of the committed Taker claim partial once.
    ///
    /// # Errors
    ///
    /// Fails closed unless the dedicated Taker runtime, agreement, response
    /// echo, and returned authorization transaction are exact.
    pub async fn prepare_native_xmr_claim_authorization_v3(
        &self,
        request: PrepareNativeXmrClaimAuthorizationV3Request,
    ) -> Result<PrepareNativeXmrClaimAuthorizationV3Result, BridgeClientError> {
        let operation = BridgeOperation::PrepareNativeXmrClaimAuthorizationV3;
        let context = request.context.clone();
        let expected_terms = request.terms;
        self.validate_request_runtime(operation, &context, &request.runtime)?;
        validate_xmr_request_binding(
            operation,
            &context,
            &request.runtime,
            &request.terms,
            XmrOperationRole::Taker,
        )?;
        self.reserve_context(operation, &context)?;
        let result: PrepareNativeXmrClaimAuthorizationV3Result = self
            .request(
                operation,
                METHOD_PREPARE_NATIVE_XMR_CLAIM_AUTHORIZATION_V3,
                request,
                &context,
            )
            .await?;
        Self::validate_response_context(operation, &context, &result.context)?;
        validate_xmr_terms_echo(operation, &expected_terms, &result.terms)?;
        validate_prepared(operation, &result.authorization)?;
        Ok(result)
    }

    /// Classifies one finalized XMR-native effect exactly once.
    ///
    /// # Errors
    ///
    /// Fails closed on runtime, terms, effect, target, coverage, response,
    /// timeout, transport, or finalized-evidence drift.
    pub async fn classify_finalized_native_xmr_effect_v3(
        &self,
        request: ClassifyFinalizedNativeXmrEffectV3Request,
    ) -> Result<ClassifyFinalizedNativeXmrEffectV3Result, BridgeClientError> {
        let operation = BridgeOperation::ClassifyFinalizedNativeXmrEffectV3;
        let context = request.context.clone();
        let expected_terms = request.terms;
        let expected_effect = request.effect;
        let expected_target = request.target.clone();
        let expected_window = request.window;
        self.validate_request_runtime(operation, &context, &request.runtime)?;
        validate_xmr_request_binding(
            operation,
            &context,
            &request.runtime,
            &request.terms,
            XmrOperationRole::Either,
        )?;
        validate_xmr_target(operation, &request.target)?;
        self.reserve_context(operation, &context)?;
        let result: ClassifyFinalizedNativeXmrEffectV3Result = self
            .request(
                operation,
                METHOD_CLASSIFY_FINALIZED_NATIVE_XMR_EFFECT_V3,
                request,
                &context,
            )
            .await?;
        Self::validate_response_context(operation, &context, &result.context)?;
        validate_xmr_classifier_echo(
            operation,
            &expected_terms,
            expected_effect,
            &expected_target,
            expected_window,
            &result,
        )?;
        Ok(result)
    }

    /// Observes canonical native escrow state and an optional refund lookup once.
    ///
    /// # Errors
    ///
    /// Fails closed on context/runtime mismatch, unknown delivery, strict
    /// decoding failure, or typed remote error.
    pub async fn observe_native_refund(
        &self,
        request: ObserveNativeRefundRequest,
    ) -> Result<ObserveNativeRefundResult, BridgeClientError> {
        let operation = BridgeOperation::ObserveNativeRefund;
        let context = request.context.clone();
        self.validate_request_runtime(operation, &context, &request.runtime)?;
        self.reserve_context(operation, &context)?;
        let result: ObserveNativeRefundResult = self
            .request(operation, METHOD_OBSERVE_NATIVE_REFUND, request, &context)
            .await?;
        Self::validate_response_context(operation, &context, &result.context)?;
        Ok(result)
    }

    /// Submits exact persisted transaction bytes once without retrying.
    ///
    /// # Errors
    ///
    /// Fails closed on context/runtime mismatch, unknown delivery, malformed
    /// exact bytes, strict decoding failure, typed remote error, or a returned
    /// transaction ID different from the persisted request ID.
    pub async fn submit_transaction(
        &self,
        request: SubmitTransactionRequest,
    ) -> Result<SubmitTransactionResult, BridgeClientError> {
        let operation = BridgeOperation::SubmitTransaction;
        let context = request.context.clone();
        let expected_transaction_id = request.transaction.transaction_id;
        self.validate_request_runtime(operation, &context, &request.runtime)?;
        validate_prepared(operation, &request.transaction)?;
        self.reserve_context(operation, &context)?;
        let result: SubmitTransactionResult = self
            .request(operation, METHOD_SUBMIT_TRANSACTION, request, &context)
            .await?;
        Self::validate_response_context(operation, &context, &result.context)?;
        if result.transaction_id != expected_transaction_id {
            return Err(BridgeClientError::SubmitTransactionIdMismatch);
        }
        Ok(result)
    }

    fn validate_request_runtime(
        &self,
        operation: BridgeOperation,
        context: &MessageContext,
        runtime: &RuntimeDescriptor,
    ) -> Result<(), BridgeClientError> {
        if runtime != &self.expected_runtime
            || context.run_id != self.expected_run_id
            || context.sidecar_role != self.expected_runtime.sidecar_role
        {
            return Err(BridgeClientError::RequestContextMismatch { operation });
        }
        Ok(())
    }

    fn reserve_context(
        &self,
        operation: BridgeOperation,
        context: &MessageContext,
    ) -> Result<(), BridgeClientError> {
        if context.run_id != self.expected_run_id
            || context.sidecar_role != self.expected_runtime.sidecar_role
        {
            return Err(BridgeClientError::RequestContextMismatch { operation });
        }
        let mut used = self
            .used_request_ids
            .lock()
            .map_err(|_| BridgeClientError::Transport { operation })?;
        if !used.insert(context.request_id.clone()) {
            return Err(BridgeClientError::RequestIdReused { operation });
        }
        Ok(())
    }

    fn validate_response_context(
        operation: BridgeOperation,
        request: &MessageContext,
        response: &MessageContext,
    ) -> Result<(), BridgeClientError> {
        if request != response {
            return Err(BridgeClientError::ResponseContextMismatch { operation });
        }
        Ok(())
    }

    async fn request<Request, Response>(
        &self,
        operation: BridgeOperation,
        method: &'static str,
        request: Request,
        context: &MessageContext,
    ) -> Result<Response, BridgeClientError>
    where
        Request: Serialize,
        Response: DeserializeOwned,
    {
        self.client
            .request(method, rpc_params![request])
            .await
            .map_err(|error| map_client_error(operation, context, error))
    }
}

/// Narrow client intended for exclusive ownership by the XMR release service.
///
/// Unlike [`BridgeClient`], this surface exposes only the dedicated,
/// durable-ownership-checked claim-authorization submission route. It
/// encapsulates one sidecar bearer capability and does not expose the generic
/// bridge API.
///
/// This Rust type is not an authorization boundary. Exclusive process
/// ownership of the bearer and network path must be enforced separately; any
/// bearer holder that can issue raw RPC can call the dedicated route.
pub struct XmrReleaseClient {
    inner: BridgeClient,
}

impl fmt::Debug for XmrReleaseClient {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("XmrReleaseClient")
            .field("expected_run_id", &self.inner.expected_run_id)
            .field("expected_runtime", &self.inner.expected_runtime)
            .field("capability", &"[REDACTED]")
            .finish_non_exhaustive()
    }
}

impl XmrReleaseClient {
    /// Builds a bounded, single-flight release client for one Taker runtime.
    ///
    /// # Errors
    ///
    /// Rejects every non-Taker runtime before building a transport, as well as
    /// any configuration rejected by [`BridgeClient::connect`].
    pub fn connect(config: BridgeClientConfig) -> Result<Self, BridgeClientError> {
        if config.expected_runtime.sidecar_role != Participant::Taker {
            return Err(configuration(
                ConfigurationError::ReleaseClientRequiresTaker,
            ));
        }
        Ok(Self {
            inner: BridgeClient::connect(config)?,
        })
    }

    /// Returns the non-secret run identity bound to this dedicated client.
    pub const fn expected_run_id(&self) -> &RunId {
        &self.inner.expected_run_id
    }

    /// Returns the non-secret runtime identity bound to this dedicated client.
    ///
    /// Release authorities use this before their durable send CAS so a
    /// misconfigured client cannot consume the unique publication attempt.
    pub const fn expected_runtime(&self) -> &RuntimeDescriptor {
        &self.inner.expected_runtime
    }

    /// Submits one exact, durably owned claim authorization without retrying.
    ///
    /// # Errors
    ///
    /// Fails closed before transport on run, role, runtime, terms, or prepared
    /// transaction drift. A response must exactly echo the context and terms
    /// and return the authorization ID supplied by the caller. Transport
    /// uncertainty is never retried.
    pub async fn submit_native_xmr_claim_authorization_v3(
        &self,
        request: SubmitNativeXmrClaimAuthorizationV3Request,
    ) -> Result<SubmitNativeXmrClaimAuthorizationV3Result, BridgeClientError> {
        let operation = BridgeOperation::SubmitNativeXmrClaimAuthorizationV3;
        let context = request.context.clone();
        let expected_terms = request.terms;
        let expected_transaction_id = request.authorization.transaction_id;
        self.inner
            .validate_request_runtime(operation, &context, &request.runtime)?;
        validate_xmr_request_binding(
            operation,
            &context,
            &request.runtime,
            &request.terms,
            XmrOperationRole::Taker,
        )?;
        validate_prepared(operation, &request.authorization)?;
        self.inner.reserve_context(operation, &context)?;
        let result: SubmitNativeXmrClaimAuthorizationV3Result = self
            .inner
            .request(
                operation,
                METHOD_SUBMIT_NATIVE_XMR_CLAIM_AUTHORIZATION_V3,
                request,
                &context,
            )
            .await?;
        BridgeClient::validate_response_context(operation, &context, &result.context)?;
        validate_xmr_terms_echo(operation, &expected_terms, &result.terms)?;
        if result.authorization_transaction_id != expected_transaction_id {
            return Err(BridgeClientError::SubmitTransactionIdMismatch);
        }
        Ok(result)
    }
}

fn validate_terms_echo(
    operation: BridgeOperation,
    expected: &WitnessedLezAssetTermsV2,
    actual: &WitnessedLezAssetTermsV2,
) -> Result<(), BridgeClientError> {
    if expected != actual {
        return Err(BridgeClientError::MalformedObservation { operation });
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum XmrOperationRole {
    Maker,
    Taker,
    Either,
}

fn validate_xmr_request_binding(
    operation: BridgeOperation,
    context: &MessageContext,
    runtime: &RuntimeDescriptor,
    terms: &XmrNativeEscrowTermsV3,
    required: XmrOperationRole,
) -> Result<(), BridgeClientError> {
    let role_matches = match required {
        XmrOperationRole::Maker => runtime.sidecar_role == Participant::Maker,
        XmrOperationRole::Taker => runtime.sidecar_role == Participant::Taker,
        XmrOperationRole::Either => true,
    };
    if terms.validate_runtime_binding(context, runtime).is_err() || !role_matches {
        return Err(BridgeClientError::MalformedObservation { operation });
    }
    Ok(())
}

fn validate_xmr_terms_echo(
    operation: BridgeOperation,
    expected: &XmrNativeEscrowTermsV3,
    actual: &XmrNativeEscrowTermsV3,
) -> Result<(), BridgeClientError> {
    if expected != actual {
        return Err(BridgeClientError::MalformedObservation { operation });
    }
    Ok(())
}

fn validate_xmr_target(
    operation: BridgeOperation,
    target: &FinalizedNativeXmrTransactionTargetV3,
) -> Result<(), BridgeClientError> {
    if let FinalizedNativeXmrTransactionTargetV3::Exact { transaction } = target {
        validate_prepared(operation, transaction)?;
    }
    Ok(())
}

fn validate_xmr_classifier_echo(
    operation: BridgeOperation,
    expected_terms: &XmrNativeEscrowTermsV3,
    expected_effect: XmrNativeEffectV3,
    expected_target: &FinalizedNativeXmrTransactionTargetV3,
    expected_window: DiscoveryWindow,
    result: &ClassifyFinalizedNativeXmrEffectV3Result,
) -> Result<(), BridgeClientError> {
    if &result.terms != expected_terms
        || result.effect != expected_effect
        || &result.target != expected_target
    {
        return Err(BridgeClientError::MalformedObservation { operation });
    }
    let coverage = match &result.outcome {
        FinalizedNativeXmrScanOutcomeV3::Found {
            finalized_clock,
            scanned_window,
            ..
        }
        | FinalizedNativeXmrScanOutcomeV3::Absent {
            finalized_clock,
            scanned_window,
        }
        | FinalizedNativeXmrScanOutcomeV3::Uncertain {
            finalized_clock,
            scanned_window,
        } => Some((*finalized_clock, *scanned_window)),
        FinalizedNativeXmrScanOutcomeV3::Unavailable { .. } => None,
    };
    if let Some((clock, window)) = coverage {
        let window_end = expected_window
            .start_height()
            .checked_add(u64::from(expected_window.max_blocks().saturating_sub(1)));
        if window != expected_window
            || clock.timestamp_ms == 0
            || window_end.is_none_or(|end| clock.height < end)
        {
            return Err(BridgeClientError::MalformedObservation { operation });
        }
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AssetOperationRole {
    Depositor,
    Claimant,
    EitherParticipant,
}

fn validate_asset_operation_role(
    operation: BridgeOperation,
    runtime: &RuntimeDescriptor,
    terms: &WitnessedLezAssetTermsV2,
    required_role: AssetOperationRole,
) -> Result<(), BridgeClientError> {
    let (depositor, depositor_account, claimant, claimant_account) = match terms.asset() {
        WitnessedLezAssetV2::Native(terms) => (
            terms.depositor(),
            terms.depositor_account_id(),
            terms.claimant(),
            terms.claimant_account_id(),
        ),
        WitnessedLezAssetV2::CustomToken(terms) => (
            terms.depositor(),
            terms.depositor_owner_account_id(),
            terms.claimant(),
            terms.claimant_owner_account_id(),
        ),
    };
    let depositor_valid =
        runtime.sidecar_role == depositor && runtime.signer_account_id == depositor_account;
    let claimant_valid =
        runtime.sidecar_role == claimant && runtime.signer_account_id == claimant_account;
    let valid = match required_role {
        AssetOperationRole::Depositor => depositor_valid,
        AssetOperationRole::Claimant => claimant_valid,
        AssetOperationRole::EitherParticipant => depositor_valid || claimant_valid,
    };
    if !valid {
        return Err(BridgeClientError::MalformedObservation { operation });
    }
    Ok(())
}

fn validate_asset_prepared_effects(
    operation: BridgeOperation,
    effects: &[WitnessedAssetPreparedEffectV2],
) -> Result<(), BridgeClientError> {
    let mut transaction_ids = HashSet::with_capacity(effects.len());
    let mut exact_bytes = HashSet::with_capacity(effects.len());
    for effect in effects {
        validate_prepared(operation, &effect.transaction)?;
        if !transaction_ids.insert(effect.transaction.transaction_id)
            || !exact_bytes.insert(effect.transaction.exact_bytes.as_slice())
        {
            return Err(BridgeClientError::MalformedPreparedTransaction { operation });
        }
    }
    Ok(())
}

fn discovery_window_end(
    operation: BridgeOperation,
    window: DiscoveryWindow,
) -> Result<u64, BridgeClientError> {
    window
        .start_height()
        .checked_add(u64::from(window.max_blocks().saturating_sub(1)))
        .ok_or(BridgeClientError::MalformedObservation { operation })
}

fn validate_asset_escrow_observation(
    operation: BridgeOperation,
    expected_window: DiscoveryWindow,
    result: &ObserveWitnessedAssetEscrowV2Result,
) -> Result<(), BridgeClientError> {
    validate_asset_effect_positions(
        operation,
        expected_window,
        result.tip_before,
        result.tip_after,
        result
            .effects
            .iter()
            .map(|effect| (effect.transaction.is_public, effect.transaction.position)),
    )
}

fn validate_asset_effect_positions(
    operation: BridgeOperation,
    expected_window: DiscoveryWindow,
    tip_before: ChainTip,
    tip_after: ChainTip,
    effects: impl IntoIterator<Item = (bool, lez_bridge_protocol::ChainPosition)>,
) -> Result<(), BridgeClientError> {
    if tip_before != tip_after {
        return Err(BridgeClientError::MalformedObservation { operation });
    }
    let window_end = discovery_window_end(operation, expected_window)?;
    let mut block_hashes = HashMap::new();
    for (is_public, position) in effects {
        if !is_public
            || position.height < expected_window.start_height()
            || position.height > window_end
            || position.height > tip_after.height
            || (position.height == tip_after.height && position.block_hash != tip_after.block_hash)
        {
            return Err(BridgeClientError::MalformedObservation { operation });
        }
        if block_hashes
            .insert(position.height, position.block_hash)
            .is_some_and(|expected| expected != position.block_hash)
        {
            return Err(BridgeClientError::MalformedObservation { operation });
        }
    }
    Ok(())
}

fn validate_asset_refund_observation(
    operation: BridgeOperation,
    expected_target: NativeRefundObservationTarget,
    result: &ObserveWitnessedAssetRefundV2Result,
) -> Result<(), BridgeClientError> {
    if result.clock_before != result.clock_after {
        return Err(BridgeClientError::MalformedObservation { operation });
    }
    validate_asset_refund_target(
        operation,
        expected_target,
        result.clock_after,
        &result.refund,
    )
}

fn validate_asset_refund_target(
    operation: BridgeOperation,
    expected_target: NativeRefundObservationTarget,
    clock: ChainClock,
    refund: &WitnessedAssetRefundObservationV2,
) -> Result<(), BridgeClientError> {
    match (expected_target, refund) {
        (
            NativeRefundObservationTarget::StateOnly,
            WitnessedAssetRefundObservationV2::NotRequested,
        )
        | (
            NativeRefundObservationTarget::Exact { .. }
            | NativeRefundObservationTarget::DiscoverByTerms { .. },
            WitnessedAssetRefundObservationV2::UnknownOrPending,
        ) => Ok(()),
        (
            NativeRefundObservationTarget::DiscoverByTerms { window },
            WitnessedAssetRefundObservationV2::Absent,
        ) => {
            if clock.height < discovery_window_end(operation, window)? {
                return Err(BridgeClientError::MalformedObservation { operation });
            }
            Ok(())
        }
        (
            NativeRefundObservationTarget::Exact {
                refund_transaction_id,
                window,
            },
            WitnessedAssetRefundObservationV2::Found(facts),
        ) => {
            if facts.transaction.transaction_id != refund_transaction_id {
                return Err(BridgeClientError::MalformedObservation { operation });
            }
            validate_asset_refund_found(operation, window, clock, facts)
        }
        (
            NativeRefundObservationTarget::DiscoverByTerms { window },
            WitnessedAssetRefundObservationV2::Found(facts),
        ) => validate_asset_refund_found(operation, window, clock, facts),
        _ => Err(BridgeClientError::MalformedObservation { operation }),
    }
}

fn validate_asset_refund_found(
    operation: BridgeOperation,
    window: DiscoveryWindow,
    clock: ChainClock,
    facts: &lez_bridge_protocol::WitnessedAssetRefundFoundFactsV2,
) -> Result<(), BridgeClientError> {
    let position = facts.transaction.position;
    if !facts.transaction.is_public
        || position.height < window.start_height()
        || position.height > discovery_window_end(operation, window)?
        || position.height > clock.height
        || (position.height == clock.height && position.block_hash != clock.block_hash)
    {
        return Err(BridgeClientError::MalformedObservation { operation });
    }
    Ok(())
}

fn validate_asset_target(
    operation: BridgeOperation,
    target: &FinalizedWitnessedAssetTransactionTargetV2,
) -> Result<(), BridgeClientError> {
    if let FinalizedWitnessedAssetTransactionTargetV2::Exact { transaction } = target {
        validate_prepared(operation, transaction)?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn validate_asset_classifier_echo<T>(
    operation: BridgeOperation,
    expected_terms: &WitnessedLezAssetTermsV2,
    expected_target: &FinalizedWitnessedAssetTransactionTargetV2,
    expected_window: DiscoveryWindow,
    actual_terms: &WitnessedLezAssetTermsV2,
    actual_target: &FinalizedWitnessedAssetTransactionTargetV2,
    outcome: &FinalizedWitnessedAssetScanOutcomeV2<T>,
) -> Result<(), BridgeClientError> {
    validate_terms_echo(operation, expected_terms, actual_terms)?;
    if expected_target != actual_target {
        return Err(BridgeClientError::MalformedObservation { operation });
    }
    let coverage = match outcome {
        FinalizedWitnessedAssetScanOutcomeV2::Found {
            finalized_clock,
            scanned_window,
            ..
        }
        | FinalizedWitnessedAssetScanOutcomeV2::Absent {
            finalized_clock,
            scanned_window,
        }
        | FinalizedWitnessedAssetScanOutcomeV2::Uncertain {
            finalized_clock,
            scanned_window,
        } => Some((*finalized_clock, *scanned_window)),
        FinalizedWitnessedAssetScanOutcomeV2::Unavailable { .. } => None,
    };
    if let Some((clock, window)) = coverage {
        let window_end = expected_window
            .start_height()
            .checked_add(u64::from(expected_window.max_blocks().saturating_sub(1)));
        if window != expected_window
            || clock.timestamp_ms == 0
            || window_end.is_none_or(|end| clock.height < end)
        {
            return Err(BridgeClientError::MalformedObservation { operation });
        }
    }
    Ok(())
}

fn validate_asset_finalized_claim_echo(
    operation: BridgeOperation,
    expected_transaction_id: Option<lez_bridge_protocol::TransactionId>,
    expected_claim: &PreparedWitnessedClaim,
    expected_window: DiscoveryWindow,
    finalized_tip: ChainTip,
    facts: &lez_bridge_protocol::FinalizedWitnessedAssetClaimFactsV2,
) -> Result<(), BridgeClientError> {
    let window_end = expected_window
        .start_height()
        .checked_add(u64::from(expected_window.max_blocks().saturating_sub(1)))
        .ok_or(BridgeClientError::MalformedObservation { operation })?;
    if finalized_tip.height < window_end
        || facts.transaction.position.height < expected_window.start_height()
        || facts.transaction.position.height > window_end
        || facts.transaction.position.block_hash != facts.containing_block.block_hash
        || facts.instruction.claim != *expected_claim
        || expected_transaction_id
            .is_some_and(|expected| facts.transaction.transaction_id != expected)
    {
        return Err(BridgeClientError::MalformedObservation { operation });
    }
    Ok(())
}

fn validate_scanned_prefix(
    operation: BridgeOperation,
    authorized_window: DiscoveryWindow,
    scanned_window: DiscoveryWindow,
    finalized_clock: ChainClock,
) -> Result<(u64, u64, bool), BridgeClientError> {
    let authorized_start = authorized_window.start_height();
    let authorized_end = authorized_start
        .checked_add(u64::from(authorized_window.max_blocks() - 1))
        .ok_or(BridgeClientError::MalformedObservation { operation })?;
    let scanned_start = scanned_window.start_height();
    let scanned_end = scanned_start
        .checked_add(u64::from(scanned_window.max_blocks() - 1))
        .ok_or(BridgeClientError::MalformedObservation { operation })?;
    if scanned_start != authorized_start
        || scanned_end > authorized_end
        || scanned_end > finalized_clock.height
        || finalized_clock.timestamp_ms == 0
    {
        return Err(BridgeClientError::MalformedObservation { operation });
    }
    Ok((
        scanned_start,
        scanned_end,
        scanned_window == authorized_window,
    ))
}

#[allow(clippy::too_many_arguments)]
fn validate_finalized_witnessed_funding_facts(
    operation: BridgeOperation,
    expected_transaction_id: Option<lez_bridge_protocol::TransactionId>,
    expected_terms: &WitnessedNativeEscrowTerms,
    expected_program: lez_bridge_protocol::Hex32,
    window_start: u64,
    window_end: u64,
    finalized_tip: ChainTip,
    funding: &lez_bridge_protocol::FinalizedWitnessedFundingFacts,
) -> Result<(), BridgeClientError> {
    let transaction = &funding.transaction;
    let instruction = &funding.instruction;
    let block = funding.containing_block;
    let metadata = &funding.metadata;
    let custody = &funding.custody;
    let expected_metadata = WitnessedEscrowMetadataFacts::from_witnessed_native_terms(
        metadata.account_id,
        expected_program,
        custody.account_id,
        expected_terms,
        EscrowState::Funded,
    );
    let expected_accounts = [
        metadata.account_id,
        custody.account_id,
        expected_terms.depositor_account_id(),
    ];
    let exact_signer =
        transaction.signer_account_ids.as_slice() == [expected_terms.depositor_account_id()];
    if expected_transaction_id.is_some_and(|expected| transaction.transaction_id != expected)
        || !transaction.is_public
        || transaction.position.height != block.block_id
        || transaction.position.block_hash != block.block_hash
        || instruction.program_id != expected_program
        || instruction.swap_id != expected_terms.swap_id()
        || instruction.ordered_account_ids.as_slice() != expected_accounts
        || metadata != &expected_metadata
        || custody.account_id != metadata.custody_account_id
        || custody.owner_program_id != expected_terms.authenticated_transfer_program_id()
        || custody.balance != expected_terms.amount()
        || !exact_signer
        || block.block_id < window_start
        || block.block_id > window_end
        || block.block_id > finalized_tip.height
        || (block.block_id == finalized_tip.height && block.block_hash != finalized_tip.block_hash)
        || window_end > finalized_tip.height
    {
        return Err(BridgeClientError::MalformedObservation { operation });
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn validate_finalized_witnessed_initialization_facts(
    operation: BridgeOperation,
    expected_transaction_id: lez_bridge_protocol::TransactionId,
    expected_transaction_bytes: &lez_bridge_protocol::ExactTransactionBytes,
    expected_terms: &WitnessedNativeEscrowTerms,
    expected_program: lez_bridge_protocol::Hex32,
    window_start: u64,
    window_end: u64,
    finalized_tip: ChainTip,
    initialization: &FinalizedWitnessedInitializationFacts,
) -> Result<(), BridgeClientError> {
    let transaction = &initialization.transaction;
    let instruction = &initialization.instruction;
    let block = initialization.containing_block;
    let metadata = &initialization.metadata;
    let custody = &initialization.custody;
    let expected_metadata = WitnessedEscrowMetadataFacts::from_witnessed_native_terms(
        metadata.account_id,
        expected_program,
        custody.account_id,
        expected_terms,
        EscrowState::Empty,
    );
    let expected_accounts = [
        metadata.account_id,
        custody.account_id,
        expected_terms.depositor_account_id(),
        expected_terms.claimant_account_id(),
        expected_terms.aggregate_authority_account_id(),
    ];
    if transaction.transaction_id != expected_transaction_id
        || &transaction.exact_bytes != expected_transaction_bytes
        || !transaction.is_public
        || transaction.position.height != block.block_id
        || transaction.position.block_hash != block.block_hash
        || transaction.signer_account_ids.as_slice() != [expected_terms.depositor_account_id()]
        || instruction.program_id != expected_program
        || &instruction.terms != expected_terms
        || instruction.ordered_account_ids.as_slice() != expected_accounts
        || metadata != &expected_metadata
        || custody.account_id != metadata.custody_account_id
        || custody.owner_program_id != expected_terms.authenticated_transfer_program_id()
        || custody.balance.as_u128() != 0
        || block.timestamp_ms == 0
        || block.block_id < window_start
        || block.block_id > window_end
        || block.block_id > finalized_tip.height
        || (block.block_id == finalized_tip.height && block.block_hash != finalized_tip.block_hash)
        || window_end > finalized_tip.height
    {
        return Err(BridgeClientError::MalformedObservation { operation });
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn validate_finalized_witnessed_claim_facts(
    operation: BridgeOperation,
    expected_transaction_id: Option<lez_bridge_protocol::TransactionId>,
    expected_claim: &PreparedWitnessedClaim,
    expected_terms: &WitnessedNativeEscrowTerms,
    expected_program: lez_bridge_protocol::Hex32,
    window_start: u64,
    window_end: u64,
    finalized_tip: ChainTip,
    claim: &FinalizedWitnessedClaimFacts,
) -> Result<(), BridgeClientError> {
    let transaction = &claim.transaction;
    let instruction = &claim.instruction;
    let block = claim.containing_block;
    let metadata = &claim.metadata;
    let custody = &claim.custody;
    let expected_metadata = WitnessedEscrowMetadataFacts::from_witnessed_native_terms(
        metadata.account_id,
        expected_program,
        custody.account_id,
        expected_terms,
        EscrowState::Claimed,
    );
    let expected_accounts = [
        metadata.account_id,
        custody.account_id,
        expected_terms.claimant_account_id(),
        expected_terms.aggregate_authority_account_id(),
    ];
    let exact_signer = transaction.signer_account_ids.as_slice()
        == [expected_terms.aggregate_authority_account_id()];
    if expected_transaction_id.is_some_and(|expected| transaction.transaction_id != expected)
        || !transaction.is_public
        || transaction.position.height != block.block_id
        || transaction.position.block_hash != block.block_hash
        || instruction.program_id != expected_program
        || instruction.swap_id != expected_terms.swap_id()
        || instruction.claimant_account_id != expected_terms.claimant_account_id()
        || instruction.aggregate_authority_account_id
            != expected_terms.aggregate_authority_account_id()
        || &instruction.claim != expected_claim
        || instruction.ordered_account_ids.as_slice() != expected_accounts
        || metadata != &expected_metadata
        || custody.account_id != metadata.custody_account_id
        || custody.owner_program_id != expected_terms.authenticated_transfer_program_id()
        || custody.balance.as_u128() != 0
        || !aggregate_signature_is_valid(
            claim.aggregate_signature.as_bytes(),
            expected_claim.message_hash.as_bytes(),
            expected_terms.aggregate_x_only_public_key().as_bytes(),
        )
        || !exact_signer
        || block.block_id < window_start
        || block.block_id > window_end
        || block.block_id > finalized_tip.height
        || (block.block_id == finalized_tip.height && block.block_hash != finalized_tip.block_hash)
        || window_end > finalized_tip.height
    {
        return Err(BridgeClientError::MalformedObservation { operation });
    }
    Ok(())
}

fn validate_endpoint(endpoint: &str) -> Result<(), BridgeClientError> {
    let url =
        Url::parse(endpoint).map_err(|_| configuration(ConfigurationError::NonLoopbackEndpoint))?;
    let loopback_literal = match url.host() {
        Some(Host::Ipv4(address)) => IpAddr::V4(address).is_loopback(),
        Some(Host::Ipv6(address)) => IpAddr::V6(address).is_loopback(),
        Some(Host::Domain(_)) | None => false,
    };
    if url.scheme() != "http"
        || !loopback_literal
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_none_or(|port| port == 0)
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(configuration(ConfigurationError::NonLoopbackEndpoint));
    }
    Ok(())
}

fn validate_prepared(
    operation: BridgeOperation,
    transaction: &PreparedTransaction,
) -> Result<(), BridgeClientError> {
    if !(1..=MAX_EXACT_TRANSACTION_BYTES).contains(&transaction.exact_bytes.as_slice().len()) {
        return Err(BridgeClientError::MalformedPreparedTransaction { operation });
    }
    Ok(())
}

fn validate_witnessed_preparation(
    operation: BridgeOperation,
    claim: &PreparedWitnessedClaim,
) -> Result<(), BridgeClientError> {
    validate_prepared_witnessed_claim(claim)
        .map_err(|_| BridgeClientError::MalformedPreparedTransaction { operation })
}

/// Validates a persisted unsigned witnessed-claim artifact without transport.
///
/// This is the same pure official-message hash and nonempty-byte check used by
/// preparation, completion, and finalized observation. It lets a fresh actor
/// reject a changed public artifact before opening a chain client without
/// copying the pinned LEZ message-domain constant.
///
/// # Errors
///
/// Returns [`PreparedWitnessedClaimValidationError`] when the exact official
/// message bytes are empty or do not hash to the retained message identity.
pub fn validate_prepared_witnessed_claim(
    claim: &PreparedWitnessedClaim,
) -> Result<(), PreparedWitnessedClaimValidationError> {
    let mut hasher = Sha256::new();
    hasher.update(OFFICIAL_PUBLIC_MESSAGE_HASH_PREFIX);
    hasher.update(claim.exact_message_bytes.as_slice());
    let computed: [u8; 32] = hasher.finalize().into();
    if claim.message_hash.as_bytes() != &computed || claim.exact_message_bytes.as_slice().is_empty()
    {
        Err(PreparedWitnessedClaimValidationError)
    } else {
        Ok(())
    }
}

fn aggregate_signature_is_valid(
    signature_bytes: &[u8; 64],
    message_hash: &[u8; 32],
    x_only_public_key: &[u8; 32],
) -> bool {
    let Ok(signature) = SchnorrSignature::from_slice(signature_bytes) else {
        return false;
    };
    let Ok(public_key) = XOnlyPublicKey::from_slice(x_only_public_key) else {
        return false;
    };
    Secp256k1::verification_only()
        .verify_schnorr(
            &signature,
            &SecpMessage::from_digest(*message_hash),
            &public_key,
        )
        .is_ok()
}

fn map_client_error(
    operation: BridgeOperation,
    context: &MessageContext,
    error: ClientError,
) -> BridgeClientError {
    match error {
        ClientError::RequestTimeout => BridgeClientError::Timeout { operation },
        ClientError::ParseError(_) => BridgeClientError::InvalidResponse { operation },
        ClientError::Call(error) => {
            let Some(data) = error.data() else {
                return BridgeClientError::InvalidResponse { operation };
            };
            let Ok(reply) = serde_json::from_str::<ProtocolErrorReply>(data.get()) else {
                return BridgeClientError::InvalidResponse { operation };
            };
            if &reply.context != context {
                return BridgeClientError::ResponseContextMismatch { operation };
            }
            BridgeClientError::Remote(RemoteProtocolError(reply))
        }
        ClientError::Transport(_)
        | ClientError::RestartNeeded(_)
        | ClientError::InvalidSubscriptionId
        | ClientError::InvalidRequestId(_)
        | ClientError::Custom(_)
        | ClientError::HttpNotImplemented
        | ClientError::EmptyBatchRequest(_)
        | ClientError::RegisterMethod(_)
        | ClientError::ServiceDisconnect => BridgeClientError::Transport { operation },
    }
}

const fn configuration(reason: ConfigurationError) -> BridgeClientError {
    BridgeClientError::InvalidConfiguration { reason }
}

#[cfg(test)]
mod tests {
    use lez_bridge_protocol::{
        ChainPosition, ExactMessageBytes, Hex32, RuntimeCompatibility, TransactionId,
        WitnessedNativeEscrowTermsInput,
    };

    use super::*;

    fn prepared_message(bytes: &[u8], message_hash: [u8; 32]) -> PreparedWitnessedClaim {
        PreparedWitnessedClaim::new(
            RequestId::new("witnessed-prepare-0001").unwrap(),
            Hex32::from_bytes(message_hash),
            ExactMessageBytes::new(bytes.to_vec()).unwrap(),
        )
    }

    #[test]
    fn witnessed_preparation_hashes_the_exact_returned_message_bytes() {
        let bytes = b"canonical-official-message";
        let mut hasher = Sha256::new();
        hasher.update(OFFICIAL_PUBLIC_MESSAGE_HASH_PREFIX);
        hasher.update(bytes);
        let message_hash = hasher.finalize().into();

        validate_prepared_witnessed_claim(&prepared_message(bytes, message_hash)).unwrap();
    }

    #[test]
    fn witnessed_preparation_rejects_message_bytes_not_bound_by_returned_hash() {
        let error =
            validate_prepared_witnessed_claim(&prepared_message(b"mutated-message", [9; 32]))
                .unwrap_err();

        assert_eq!(error, PreparedWitnessedClaimValidationError);
    }

    #[test]
    fn asset_operation_roles_accept_only_the_required_bound_signer() {
        let depositor_account = Hex32::from_bytes([1; 32]);
        let claimant_account = Hex32::from_bytes([2; 32]);
        let terms = WitnessedLezAssetTermsV2::native(
            WitnessedNativeEscrowTerms::new(WitnessedNativeEscrowTermsInput {
                swap_id: Hex32::from_bytes([3; 32]),
                terms_hash: Hex32::from_bytes([4; 32]),
                depositor: Participant::Maker,
                depositor_account_id: depositor_account,
                claimant: Participant::Taker,
                claimant_account_id: claimant_account,
                aggregate_authority_account_id: Hex32::from_bytes([5; 32]),
                aggregate_x_only_public_key: Hex32::from_bytes([6; 32]),
                amount: 7,
                refund_at_ms: 8,
                authenticated_transfer_program_id: Hex32::from_bytes([9; 32]),
            })
            .unwrap(),
        );
        let runtime = |role, signer_account_id| {
            RuntimeDescriptor::new(
                role,
                RuntimeCompatibility::LeeV0_2_0,
                Hex32::from_bytes([10; 32]),
                Hex32::from_bytes([11; 32]),
                Hex32::from_bytes([12; 32]),
                Hex32::from_bytes([13; 32]),
                signer_account_id,
            )
        };
        let validates = |operation, role, signer_account_id, required_role| {
            validate_asset_operation_role(
                operation,
                &runtime(role, signer_account_id),
                &terms,
                required_role,
            )
        };

        assert!(
            validates(
                BridgeOperation::PrepareWitnessedAssetEscrowV2,
                Participant::Maker,
                depositor_account,
                AssetOperationRole::Depositor,
            )
            .is_ok()
        );
        assert!(
            validates(
                BridgeOperation::PrepareWitnessedAssetEscrowV2,
                Participant::Taker,
                claimant_account,
                AssetOperationRole::Depositor,
            )
            .is_err()
        );
        assert!(
            validates(
                BridgeOperation::PrepareWitnessedAssetClaimV2,
                Participant::Taker,
                claimant_account,
                AssetOperationRole::Claimant,
            )
            .is_ok()
        );
        assert!(
            validates(
                BridgeOperation::PrepareWitnessedAssetClaimV2,
                Participant::Maker,
                depositor_account,
                AssetOperationRole::Claimant,
            )
            .is_err()
        );
        assert!(
            validates(
                BridgeOperation::PrepareWitnessedAssetRefundV2,
                Participant::Maker,
                depositor_account,
                AssetOperationRole::EitherParticipant,
            )
            .is_ok()
        );
        assert!(
            validates(
                BridgeOperation::PrepareWitnessedAssetRefundV2,
                Participant::Taker,
                claimant_account,
                AssetOperationRole::EitherParticipant,
            )
            .is_ok()
        );
        assert!(
            validates(
                BridgeOperation::PrepareWitnessedAssetRefundV2,
                Participant::Taker,
                Hex32::from_bytes([14; 32]),
                AssetOperationRole::EitherParticipant,
            )
            .is_err()
        );
    }

    #[test]
    fn asset_escrow_effect_positions_require_one_stable_public_canonical_window() {
        let operation = BridgeOperation::ObserveWitnessedAssetEscrowV2;
        let window = DiscoveryWindow::new(10, 3).unwrap();
        let tip = ChainTip::new(Hex32::from_bytes([12; 32]), 12);
        let position = |hash, height| ChainPosition::new(Hex32::from_bytes([hash; 32]), height, 0);

        assert!(
            validate_asset_effect_positions(
                operation,
                window,
                tip,
                tip,
                [(true, position(10, 10)), (true, position(12, 12))],
            )
            .is_ok()
        );
        assert!(
            validate_asset_effect_positions(
                operation,
                window,
                ChainTip::new(Hex32::from_bytes([11; 32]), 11),
                tip,
                [(true, position(10, 10))],
            )
            .is_err()
        );
        assert!(
            validate_asset_effect_positions(
                operation,
                window,
                tip,
                tip,
                [(false, position(10, 10))],
            )
            .is_err()
        );
        assert!(
            validate_asset_effect_positions(
                operation,
                window,
                tip,
                tip,
                [(true, position(13, 13))],
            )
            .is_err()
        );
        assert!(
            validate_asset_effect_positions(
                operation,
                window,
                tip,
                tip,
                [(true, position(21, 11)), (true, position(22, 11))],
            )
            .is_err()
        );
    }

    #[test]
    fn asset_refund_target_modes_do_not_overstate_lookup_evidence() {
        let operation = BridgeOperation::ObserveWitnessedAssetRefundV2;
        let window = DiscoveryWindow::new(10, 3).unwrap();
        let clock = ChainClock::new(Hex32::from_bytes([12; 32]), 12, 1_000);
        let exact = NativeRefundObservationTarget::Exact {
            refund_transaction_id: TransactionId::from_bytes([21; 32]),
            window,
        };
        let discovery = NativeRefundObservationTarget::DiscoverByTerms { window };

        assert!(
            validate_asset_refund_target(
                operation,
                NativeRefundObservationTarget::StateOnly,
                clock,
                &WitnessedAssetRefundObservationV2::NotRequested,
            )
            .is_ok()
        );
        assert!(
            validate_asset_refund_target(
                operation,
                NativeRefundObservationTarget::StateOnly,
                clock,
                &WitnessedAssetRefundObservationV2::UnknownOrPending,
            )
            .is_err()
        );
        assert!(
            validate_asset_refund_target(
                operation,
                exact,
                clock,
                &WitnessedAssetRefundObservationV2::UnknownOrPending,
            )
            .is_ok()
        );
        assert!(
            validate_asset_refund_target(
                operation,
                exact,
                clock,
                &WitnessedAssetRefundObservationV2::Absent,
            )
            .is_err()
        );
        assert!(
            validate_asset_refund_target(
                operation,
                discovery,
                clock,
                &WitnessedAssetRefundObservationV2::Absent,
            )
            .is_ok()
        );
        assert!(
            validate_asset_refund_target(
                operation,
                discovery,
                ChainClock::new(Hex32::from_bytes([11; 32]), 11, 900),
                &WitnessedAssetRefundObservationV2::Absent,
            )
            .is_err()
        );
    }
}
