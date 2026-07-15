//! Bounded SDK-side client for the isolated official LEZ compatibility sidecar.
//!
//! This client accepts only an explicit loopback IP literal over plain HTTP.
//! `jsonrpsee`'s direct Hyper connector does not follow redirects or consult
//! proxy environment variables. Every call is attempted exactly once: in
//! particular, randomized transaction preparation and submission are never
//! retried after a timeout or transport failure with an unknown outcome.

#![forbid(unsafe_code)]

use std::{collections::HashSet, fmt, net::IpAddr, sync::Mutex, time::Duration};

use jsonrpsee::{
    core::{ClientError, client::ClientT},
    rpc_params,
};
use jsonrpsee_http_client::{HeaderMap, HeaderValue, HttpClient, HttpClientBuilder};
use lez_bridge_protocol::{
    CompleteWitnessedClaimRequest, CompleteWitnessedClaimResult, DescribeRuntimeRequest,
    DescribeRuntimeResult, ErrorCode, ErrorMessage, EscrowState,
    FinalizedWitnessedClaimObservationTarget, MessageContext, ObserveEscrowRequest,
    ObserveEscrowResult, ObserveFinalizedWitnessedClaimRequest,
    ObserveFinalizedWitnessedClaimResult, ObserveNativeRefundRequest, ObserveNativeRefundResult,
    ObserveRevealingClaimRequest, ObserveRevealingClaimResult, ObserveWitnessedEscrowRequest,
    ObserveWitnessedEscrowResult, Participant, PrepareNativeEscrowRequest,
    PrepareNativeEscrowResult, PrepareNativeRefundRequest, PrepareNativeRefundResult,
    PrepareRevealingClaimRequest, PrepareRevealingClaimResult, PrepareWitnessedClaimRequest,
    PrepareWitnessedClaimResult, PrepareWitnessedEscrowRequest, PrepareWitnessedEscrowResult,
    PreparedTransaction, PreparedWitnessedClaim, ProtocolErrorReply, RequestId, RunId,
    RuntimeDescriptor, SubmitTransactionRequest, SubmitTransactionResult,
    WitnessedEscrowMetadataFacts,
};
pub use lez_bridge_protocol::{
    MAX_RPC_BODY_BYTES, METHOD_COMPLETE_WITNESSED_CLAIM, METHOD_DESCRIBE_RUNTIME,
    METHOD_OBSERVE_ESCROW, METHOD_OBSERVE_FINALIZED_WITNESSED_CLAIM, METHOD_OBSERVE_NATIVE_REFUND,
    METHOD_OBSERVE_REVEALING_CLAIM, METHOD_OBSERVE_WITNESSED_ESCROW, METHOD_PREPARE_NATIVE_ESCROW,
    METHOD_PREPARE_NATIVE_REFUND, METHOD_PREPARE_REVEALING_CLAIM, METHOD_PREPARE_WITNESSED_CLAIM,
    METHOD_PREPARE_WITNESSED_ESCROW, METHOD_SUBMIT_TRANSACTION, RUN_ID_HEADER, SIDECAR_ROLE_HEADER,
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
pub const MAX_REQUEST_TIMEOUT: Duration = Duration::from_mins(1);
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
    /// Randomized native initialization and funding preparation.
    PrepareNativeEscrow,
    /// Aggregate-witness initialization and funding preparation.
    PrepareWitnessedEscrow,
    /// Native initialization and funding observation.
    ObserveEscrow,
    /// Aggregate-witness initialization and funding observation.
    ObserveWitnessedEscrow,
    /// Randomized revealing-claim preparation.
    PrepareRevealingClaim,
    /// Unsigned aggregate-witness message reservation.
    PrepareWitnessedClaim,
    /// Exact aggregate-witness transaction completion.
    CompleteWitnessedClaim,
    /// Exact finalized aggregate-witness claim observation.
    ObserveFinalizedWitnessedClaim,
    /// Revealing-claim observation.
    ObserveRevealingClaim,
    /// Fixed-destination native refund preparation.
    PrepareNativeRefund,
    /// Native escrow state and refund observation.
    ObserveNativeRefund,
    /// Exact transaction submission.
    SubmitTransaction,
}

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
    /// Timeouts must be finite and at most 60 seconds.
    #[error("request timeout must be greater than zero and at most 60 seconds")]
    InvalidTimeout,
    /// Capability could not be encoded in a sensitive bearer header.
    #[error("capability header is invalid")]
    InvalidCapability,
    /// The bounded HTTP client could not be created.
    #[error("bounded HTTP transport could not be built")]
    TransportBuild,
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
        let transaction = &result.claim.transaction;
        let instruction = &result.claim.instruction;
        let block = result.claim.containing_block;
        let metadata = &result.claim.metadata;
        let custody = &result.claim.custody;
        let expected_metadata = WitnessedEscrowMetadataFacts::from_witnessed_native_terms(
            metadata.account_id,
            expected_program,
            custody.account_id,
            &expected_terms,
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
            || instruction.claim != expected_claim
            || instruction.ordered_account_ids.as_slice() != expected_accounts
            || metadata != &expected_metadata
            || custody.account_id != metadata.custody_account_id
            || custody.owner_program_id != expected_terms.authenticated_transfer_program_id()
            || custody.balance.as_u128() != 0
            || !aggregate_signature_is_valid(
                result.claim.aggregate_signature.as_bytes(),
                expected_claim.message_hash.as_bytes(),
                expected_terms.aggregate_x_only_public_key().as_bytes(),
            )
            || !exact_signer
            || block.block_id < window_start
            || block.block_id > window_end
            || block.block_id > result.finalized_tip.height
            || (block.block_id == result.finalized_tip.height
                && block.block_hash != result.finalized_tip.block_hash)
            || window_end > result.finalized_tip.height
        {
            return Err(BridgeClientError::MalformedObservation { operation });
        }
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
    let mut hasher = Sha256::new();
    hasher.update(OFFICIAL_PUBLIC_MESSAGE_HASH_PREFIX);
    hasher.update(claim.exact_message_bytes.as_slice());
    let computed: [u8; 32] = hasher.finalize().into();
    if claim.message_hash.as_bytes() != &computed || claim.exact_message_bytes.as_slice().is_empty()
    {
        Err(BridgeClientError::MalformedPreparedTransaction { operation })
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
    use lez_bridge_protocol::{ExactMessageBytes, Hex32};

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

        validate_witnessed_preparation(
            BridgeOperation::PrepareWitnessedClaim,
            &prepared_message(bytes, message_hash),
        )
        .unwrap();
    }

    #[test]
    fn witnessed_preparation_rejects_message_bytes_not_bound_by_returned_hash() {
        let error = validate_witnessed_preparation(
            BridgeOperation::PrepareWitnessedClaim,
            &prepared_message(b"mutated-message", [9; 32]),
        )
        .unwrap_err();

        assert!(matches!(
            error,
            BridgeClientError::MalformedPreparedTransaction {
                operation: BridgeOperation::PrepareWitnessedClaim
            }
        ));
    }
}
