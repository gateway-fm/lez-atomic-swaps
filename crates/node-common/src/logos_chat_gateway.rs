//! Session-scoped bridge between owner-local Chat RPC and Logos Chat text messages.
//!
//! The Logos module owns network identity, conversations, and E2EE. This gateway
//! owns no chain authority: it only translates the existing fixed JSON-RPC Chat
//! methods into bounded, role-directed frames. The Maker daemon and its durable
//! stores remain the sole validators and replay authorities.

use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
use jsonrpsee::{RpcModule, core::RegisterMethodError, types::ErrorObjectOwned};
use lez_swap_store::{MakerOfferId, MakerOfferStatus, MakerRouteV1};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use thiserror::Error;
use tokio::sync::oneshot;

use crate::{
    TakerMakerIdentityV1, call_local_chat_rpc, local_rpc::LocalRpcRemoteError,
    run_local_delivery::MAXIMUM_LOGOS_OFFER_ANNOUNCEMENT_BASE64_BYTES,
    verify_logos_offer_announcement,
};

const GATEWAY_SCHEMA_VERSION_V1: u16 = 1;
const MAXIMUM_FRAME_BYTES: usize = 1024 * 1024;
const MAXIMUM_QUEUED_FRAMES: usize = 64;
const MAXIMUM_PENDING_REQUESTS: usize = 32;
const MAXIMUM_INFLIGHT_MAKER_REQUESTS: usize = 32;
const MAXIMUM_CACHED_RESPONSES: usize = 128;
const MAXIMUM_MAKER_SESSIONS: usize = 32;
const MAXIMUM_INDEXED_OFFERS: usize = 1_024;
const MAXIMUM_INDEXED_OFFERS_PER_MAKER: usize = 128;
const MAXIMUM_LISTED_OFFERS: usize = 16;
const MAXIMUM_ADDRESS_BYTES: usize = 16 * 1024;
const MAXIMUM_CONVERSATION_ID_BYTES: usize = 4 * 1024;
const MAXIMUM_REMOTE_FAILURE_MESSAGE_BYTES: usize = 4 * 1024;
// The existing owner-local Chat caller has a 30-second request deadline. Keep
// the peer wait strictly inside it so the proxy can return a correlated error
// and release its pending slot before the outer connection is abandoned.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(25);

const INVALID_PARAMS_CODE: i32 = -32_602;
const DEPENDENCY_UNAVAILABLE_CODE: i32 = -32_010;
const CONFLICT_CODE: i32 = -32_012;
const MAKER_APPLICATION_CONFLICT_CODE: i32 = -32_009;
const MAKER_OFFER_UNAVAILABLE_CODE: i32 = -32_018;

/// The exact application Chat methods allowed through the transport bridge.
pub const LOGOS_CHAT_GATEWAY_METHODS_V1: [&str; 13] = [
    "btc_chat_propose_v1",
    "btc_chat_propose_v2",
    "btc_chat_complete_v1",
    "btc_chat_complete_v2",
    "btc_reserve_v1",
    "btc_prepare_claim_v1",
    "btc_ceremony_reserve_v1",
    "btc_ceremony_nonce_v1",
    "btc_ceremony_partial_v1",
    "zec_chat_propose_v1",
    "zec_chat_complete_v1",
    "xmr_chat_stage_a_v1",
    "xmr_chat_activate_v1",
];

/// Fixed endpoint role for one gateway process.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LogosChatGatewayRoleV1 {
    /// Receives requests from the Taker and invokes the real Maker Chat socket.
    Maker,
    /// Exposes the proxy socket used by the real Taker flow.
    Taker,
}

impl LogosChatGatewayRoleV1 {
    const fn peer(self) -> Self {
        match self {
            Self::Maker => Self::Taker,
            Self::Taker => Self::Maker,
        }
    }
}

/// Bounded, path-free gateway failures.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum LogosChatGatewayError {
    /// The request or Chat frame is malformed, oversized, or unsupported.
    #[error("invalid Logos Chat gateway input")]
    InvalidInput,
    /// A different address or conversation is already bound to this session.
    #[error("conflicting Logos Chat session binding")]
    SessionConflict,
    /// The frame was received before the direct Chat session was bound.
    #[error("Logos Chat session is not bound")]
    SessionUnavailable,
    /// The bounded in-memory queue or request table is full.
    #[error("Logos Chat gateway capacity is exhausted")]
    Capacity,
    /// The peer or the owner-local Maker Chat endpoint did not complete the request.
    #[error("Logos Chat peer operation is unavailable")]
    DependencyUnavailable,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct SessionBindingV1 {
    conversation_id: Box<str>,
    local_address: Box<str>,
    peer_address: Box<str>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
struct RemoteFailureV1 {
    code: i32,
    message: Box<str>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum LogosChatMessageV1 {
    Request {
        nonce: u64,
        method: Box<str>,
        parameter: Value,
    },
    Response {
        request_frame_id: Box<str>,
        result: Option<Value>,
        error: Option<RemoteFailureV1>,
    },
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
struct LogosChatFrameV1 {
    schema_version: u16,
    frame_id: Box<str>,
    sender_role: LogosChatGatewayRoleV1,
    recipient_role: LogosChatGatewayRoleV1,
    message: LogosChatMessageV1,
}

impl LogosChatFrameV1 {
    fn new(
        sender_role: LogosChatGatewayRoleV1,
        message: LogosChatMessageV1,
    ) -> Result<Self, LogosChatGatewayError> {
        validate_message(&message)?;
        let recipient_role = sender_role.peer();
        let frame_id = frame_id(sender_role, recipient_role, &message)?;
        Ok(Self {
            schema_version: GATEWAY_SCHEMA_VERSION_V1,
            frame_id: frame_id.into_boxed_str(),
            sender_role,
            recipient_role,
            message,
        })
    }

    fn from_content(content: &str) -> Result<Self, LogosChatGatewayError> {
        if content.is_empty() || content.len() > MAXIMUM_FRAME_BYTES {
            return Err(LogosChatGatewayError::InvalidInput);
        }
        let frame: Self =
            serde_json::from_str(content).map_err(|_| LogosChatGatewayError::InvalidInput)?;
        frame.validate()?;
        Ok(frame)
    }

    fn validate(&self) -> Result<(), LogosChatGatewayError> {
        if self.schema_version != GATEWAY_SCHEMA_VERSION_V1
            || self.recipient_role != self.sender_role.peer()
            || self.frame_id.len() != 64
            || !self.frame_id.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(LogosChatGatewayError::InvalidInput);
        }
        validate_message(&self.message)?;
        let expected = frame_id(self.sender_role, self.recipient_role, &self.message)?;
        if self.frame_id.as_ref() != expected {
            return Err(LogosChatGatewayError::InvalidInput);
        }
        Ok(())
    }

    fn to_outbox(
        &self,
        conversation_id: Box<str>,
    ) -> Result<LogosChatGatewayOutboxItemV1, LogosChatGatewayError> {
        let content =
            serde_json::to_string(self).map_err(|_| LogosChatGatewayError::InvalidInput)?;
        if content.len() > MAXIMUM_FRAME_BYTES {
            return Err(LogosChatGatewayError::InvalidInput);
        }
        Ok(LogosChatGatewayOutboxItemV1 {
            schema_version: GATEWAY_SCHEMA_VERSION_V1,
            frame_id: self.frame_id.clone(),
            conversation_id,
            content: content.into_boxed_str(),
        })
    }
}

fn validate_message(message: &LogosChatMessageV1) -> Result<(), LogosChatGatewayError> {
    match message {
        LogosChatMessageV1::Request {
            nonce: _,
            method,
            parameter,
        } => {
            if !LOGOS_CHAT_GATEWAY_METHODS_V1.contains(&method.as_ref()) || !parameter.is_object() {
                return Err(LogosChatGatewayError::InvalidInput);
            }
        }
        LogosChatMessageV1::Response {
            request_frame_id,
            result,
            error,
        } => {
            if request_frame_id.len() != 64
                || !request_frame_id
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit())
                || result.is_some() == error.is_some()
                || result.as_ref().is_some_and(Value::is_null)
            {
                return Err(LogosChatGatewayError::InvalidInput);
            }
        }
    }
    Ok(())
}

fn frame_id(
    sender_role: LogosChatGatewayRoleV1,
    recipient_role: LogosChatGatewayRoleV1,
    message: &LogosChatMessageV1,
) -> Result<String, LogosChatGatewayError> {
    let encoded = serde_json::to_vec(&(
        GATEWAY_SCHEMA_VERSION_V1,
        sender_role,
        recipient_role,
        message,
    ))
    .map_err(|_| LogosChatGatewayError::InvalidInput)?;
    Ok(hex::encode(Sha256::digest(encoded)))
}

/// One exact frame waiting for the Basecamp Chat adapter to send it.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LogosChatGatewayOutboxItemV1 {
    /// Fixed frame schema.
    pub schema_version: u16,
    /// Content-addressed frame identifier used for exact acknowledgement.
    pub frame_id: Box<str>,
    /// Exact direct conversation that must carry this frame.
    pub conversation_id: Box<str>,
    /// UTF-8 JSON passed unchanged to `chat_module.send_message`.
    pub content: Box<str>,
}

/// Request to bind the gateway to one direct conversation for this app lifetime.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LogosChatGatewayBindRequestV1 {
    /// Fixed request schema.
    pub schema_version: u16,
    /// Chat module conversation identifier.
    pub conversation_id: Box<str>,
    /// This installation's Chat address.
    pub local_address: Box<str>,
    /// The other direct participant's Chat address.
    pub peer_address: Box<str>,
}

/// Empty request for one outbox peek.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LogosChatGatewayOutboxRequestV1 {
    /// Fixed request schema.
    pub schema_version: u16,
}

/// Exact acknowledgement after `send_message` accepted one frame.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LogosChatGatewayOutboxAckRequestV1 {
    /// Fixed request schema.
    pub schema_version: u16,
    /// Must match the current outbox head.
    pub frame_id: Box<str>,
    /// Must match the current outbox head's exact Chat destination.
    pub conversation_id: Box<str>,
}

/// One Chat event submitted by the Basecamp module.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LogosChatGatewayIngestRequestV1 {
    /// Fixed request schema.
    pub schema_version: u16,
    /// Conversation from the `message_received` event.
    pub conversation_id: Box<str>,
    /// Sender address from the `message_received` event.
    pub sender_address: Box<str>,
    /// Exact text content from the event.
    pub content: Box<str>,
}

/// Empty request for a redacted gateway status snapshot.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LogosChatGatewayStatusRequestV1 {
    /// Fixed request schema.
    pub schema_version: u16,
}

/// Empty request to clear an idle app-lifetime session after explicit owner action.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LogosChatGatewayResetRequestV1 {
    /// Fixed request schema.
    pub schema_version: u16,
}

/// Redacted operational status; no payloads, paths, or peer addresses are exposed.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LogosChatGatewayStatusV1 {
    /// Fixed response schema.
    pub schema_version: u16,
    /// Endpoint role.
    pub role: LogosChatGatewayRoleV1,
    /// Whether one exact direct session has been pinned.
    pub session_bound: bool,
    /// Number of exact direct sessions (Maker may serve several Takers).
    pub session_count: u16,
    /// Number of unsent frames.
    pub queued_frames: u16,
    /// Number of Taker calls awaiting a peer response.
    pub pending_requests: u16,
    /// Number of live authenticated offers retained by a Taker endpoint.
    pub discovered_offers: u16,
}

/// Idempotent control acknowledgement.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LogosChatGatewayAckV1 {
    /// Fixed response schema.
    pub schema_version: u16,
    /// Whether the exact operation was already observed.
    pub was_replay: bool,
}

/// Exact Delivery event bytes forwarded by the Basecamp adapter.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LogosOfferIngestRequestV1 {
    /// Fixed request schema.
    pub schema_version: u16,
    /// Standard-Base64 canonical announcement payload from Delivery.
    pub payload_base64: Box<str>,
}

/// Route-filtered read of the bounded app-lifetime offer index.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LogosOfferListRequestV1 {
    /// Fixed request schema.
    pub schema_version: u16,
    /// Optional exact route filter.
    pub route: Option<MakerRouteV1>,
}

/// Exact indexed offer selected before automatic direct-Chat connection.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LogosOfferSelectRequestV1 {
    /// Fixed request schema.
    pub schema_version: u16,
    /// Maker identity displayed in the authenticated order book.
    pub maker_identity: TakerMakerIdentityV1,
    /// Immutable offer identifier displayed in the authenticated order book.
    pub offer_id: MakerOfferId,
}

/// Secret-free active announcement returned to Basecamp.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LogosOfferViewV1 {
    /// Complete immutable offer terms from the nested signed envelope.
    pub offer: lez_swap_store::MakerOfferV1,
    /// Compressed Maker identity authenticating offer and Chat address.
    pub maker_identity: TakerMakerIdentityV1,
    /// Agreement commitment to the exact nested signed offer envelope.
    pub signed_envelope_sha256: [u8; 32],
    /// Current app-lifetime direct Chat address signed by the Maker.
    pub maker_chat_address: Box<str>,
    /// Monotonic durable offer revision.
    pub offer_revision: u64,
    /// Current signed lifecycle projection.
    pub availability: MakerOfferStatus,
    /// Exclusive local-index lease boundary.
    pub valid_until_unix_seconds: u64,
    /// Exact signed announcement proof for owner-service admission.
    pub announcement_base64: Box<str>,
}

/// Bounded active-order-book projection plus visible conflict counters.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LogosOfferListV1 {
    /// Fixed response schema.
    pub schema_version: u16,
    /// Authenticated active entries in deterministic Maker/offer order.
    pub offers: Vec<LogosOfferViewV1>,
    /// Signed non-active entries still inside their short lease.
    pub unavailable_offers: u16,
    /// Offers hidden immediately after a correlated losing negotiation response.
    pub locally_contended_offers: u16,
    /// Additional matching active offers omitted to preserve the RPC response bound.
    pub omitted_offers: u16,
}

/// Selected order-book entry used to connect Chat without address transcription.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LogosOfferSelectionV1 {
    /// Fixed response schema.
    pub schema_version: u16,
    /// Exact active view selected from the authenticated index.
    pub selected: LogosOfferViewV1,
}

#[derive(Clone, Debug)]
struct IndexedOfferV1 {
    announcement: crate::AuthenticatedLogosOfferAnnouncementV1,
}

type OfferIdentityV1 = ([u8; 33], Box<str>);
type OfferIndexV1 = BTreeMap<OfferIdentityV1, IndexedOfferV1>;

#[derive(Debug)]
struct PendingResponse {
    sender: oneshot::Sender<Result<Value, RemoteFailureV1>>,
    selected_offer: Option<OfferIdentityV1>,
}

#[derive(Clone, Debug)]
struct SelectedOfferV1 {
    key: OfferIdentityV1,
    maker_chat_address: Box<str>,
}

#[derive(Clone, Debug)]
struct CachedResponse {
    request_frame_id: Box<str>,
    outbox: LogosChatGatewayOutboxItemV1,
}

struct InflightMakerRequestGuard {
    gateway: Arc<LogosChatGateway>,
    request_key: (Box<str>, Box<str>),
}

impl Drop for InflightMakerRequestGuard {
    fn drop(&mut self) {
        if let Ok(mut inflight) = self.gateway.inflight_maker_requests.lock() {
            inflight.remove(&self.request_key);
        }
    }
}

/// In-memory, session-scoped bridge state shared by the control and Taker proxy sockets.
pub struct LogosChatGateway {
    role: LogosChatGatewayRoleV1,
    maker_chat_socket: Option<PathBuf>,
    lifecycle: Mutex<()>,
    sessions: Mutex<BTreeMap<Box<str>, SessionBindingV1>>,
    outbox: Mutex<VecDeque<LogosChatGatewayOutboxItemV1>>,
    pending: Mutex<BTreeMap<Box<str>, PendingResponse>>,
    inflight_maker_requests: Mutex<BTreeSet<(Box<str>, Box<str>)>>,
    cached_responses: Mutex<VecDeque<CachedResponse>>,
    offer_index: Mutex<OfferIndexV1>,
    locally_unavailable_offers: Mutex<BTreeSet<OfferIdentityV1>>,
    selected_offer: Mutex<Option<SelectedOfferV1>>,
    trusted_clock: Arc<dyn Fn() -> Result<u64, LogosChatGatewayError> + Send + Sync>,
    sequence: AtomicU64,
}

impl std::fmt::Debug for LogosChatGateway {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LogosChatGateway")
            .field("role", &self.role)
            .field("maker_chat_configured", &self.maker_chat_socket.is_some())
            .finish_non_exhaustive()
    }
}

impl LogosChatGateway {
    /// Creates a role-fixed gateway. Maker endpoints require an absolute daemon Chat socket;
    /// Taker endpoints must not receive one.
    ///
    /// # Errors
    ///
    /// Returns [`LogosChatGatewayError::InvalidInput`] when the role and Maker
    /// Chat socket configuration do not form one fixed endpoint.
    pub fn new(
        role: LogosChatGatewayRoleV1,
        maker_chat_socket: Option<PathBuf>,
    ) -> Result<Self, LogosChatGatewayError> {
        Self::new_with_clock(role, maker_chat_socket, || {
            trusted_now().map_err(|()| LogosChatGatewayError::DependencyUnavailable)
        })
    }

    /// Creates a gateway with an injected trusted clock for deterministic local tests.
    /// Production endpoints use [`Self::new`] and the host wall clock.
    #[doc(hidden)]
    pub fn new_with_clock<F>(
        role: LogosChatGatewayRoleV1,
        maker_chat_socket: Option<PathBuf>,
        trusted_clock: F,
    ) -> Result<Self, LogosChatGatewayError>
    where
        F: Fn() -> Result<u64, LogosChatGatewayError> + Send + Sync + 'static,
    {
        let valid = match (role, maker_chat_socket.as_ref()) {
            (LogosChatGatewayRoleV1::Maker, Some(path)) => path.is_absolute(),
            (LogosChatGatewayRoleV1::Taker, None) => true,
            _ => false,
        };
        if !valid {
            return Err(LogosChatGatewayError::InvalidInput);
        }
        Ok(Self {
            role,
            maker_chat_socket,
            lifecycle: Mutex::new(()),
            sessions: Mutex::new(BTreeMap::new()),
            outbox: Mutex::new(VecDeque::new()),
            pending: Mutex::new(BTreeMap::new()),
            inflight_maker_requests: Mutex::new(BTreeSet::new()),
            cached_responses: Mutex::new(VecDeque::new()),
            offer_index: Mutex::new(BTreeMap::new()),
            locally_unavailable_offers: Mutex::new(BTreeSet::new()),
            selected_offer: Mutex::new(None),
            trusted_clock: Arc::new(trusted_clock),
            sequence: AtomicU64::new(0),
        })
    }

    /// Pins one direct Chat conversation. Repeating the exact binding is idempotent.
    ///
    /// # Errors
    ///
    /// Returns an input, conflict, or dependency error when validation fails,
    /// another session is already pinned, or gateway state is unavailable.
    pub fn bind_session(
        &self,
        request: &LogosChatGatewayBindRequestV1,
    ) -> Result<LogosChatGatewayAckV1, LogosChatGatewayError> {
        let _lifecycle = self
            .lifecycle
            .lock()
            .map_err(|_| LogosChatGatewayError::DependencyUnavailable)?;
        validate_schema(request.schema_version)?;
        validate_text(&request.conversation_id, MAXIMUM_CONVERSATION_ID_BYTES)?;
        validate_text(&request.local_address, MAXIMUM_ADDRESS_BYTES)?;
        validate_text(&request.peer_address, MAXIMUM_ADDRESS_BYTES)?;
        if request.local_address == request.peer_address {
            return Err(LogosChatGatewayError::InvalidInput);
        }
        let binding = SessionBindingV1 {
            conversation_id: request.conversation_id.clone(),
            local_address: request.local_address.clone(),
            peer_address: request.peer_address.clone(),
        };
        let mut sessions = self
            .sessions
            .lock()
            .map_err(|_| LogosChatGatewayError::DependencyUnavailable)?;
        if let Some(existing) = sessions.get(&request.conversation_id) {
            return if existing == &binding {
                Ok(LogosChatGatewayAckV1 {
                    schema_version: GATEWAY_SCHEMA_VERSION_V1,
                    was_replay: true,
                })
            } else {
                Err(LogosChatGatewayError::SessionConflict)
            };
        }
        if self.role == LogosChatGatewayRoleV1::Taker && !sessions.is_empty()
            || sessions.values().any(|existing| {
                existing.local_address != binding.local_address
                    || existing.peer_address == binding.peer_address
            })
            || sessions.len() >= MAXIMUM_MAKER_SESSIONS
        {
            return Err(LogosChatGatewayError::SessionConflict);
        }
        sessions.insert(binding.conversation_id.clone(), binding);
        Ok(LogosChatGatewayAckV1 {
            schema_version: GATEWAY_SCHEMA_VERSION_V1,
            was_replay: false,
        })
    }

    /// Returns, without removing, the oldest frame awaiting Chat send.
    ///
    /// # Errors
    ///
    /// Returns an input or dependency error for a bad schema or unavailable
    /// gateway state.
    pub fn outbox_peek(
        &self,
        request: LogosChatGatewayOutboxRequestV1,
    ) -> Result<Option<LogosChatGatewayOutboxItemV1>, LogosChatGatewayError> {
        validate_schema(request.schema_version)?;
        Ok(self
            .outbox
            .lock()
            .map_err(|_| LogosChatGatewayError::DependencyUnavailable)?
            .front()
            .cloned())
    }

    /// Removes only the exact current outbox head after Chat accepted it.
    ///
    /// # Errors
    ///
    /// Returns an input, conflict, or dependency error for a bad schema, a
    /// non-head acknowledgement, or unavailable gateway state.
    pub fn outbox_ack(
        &self,
        request: &LogosChatGatewayOutboxAckRequestV1,
    ) -> Result<LogosChatGatewayAckV1, LogosChatGatewayError> {
        validate_schema(request.schema_version)?;
        let mut outbox = self
            .outbox
            .lock()
            .map_err(|_| LogosChatGatewayError::DependencyUnavailable)?;
        let Some(head) = outbox.front() else {
            return Ok(LogosChatGatewayAckV1 {
                schema_version: GATEWAY_SCHEMA_VERSION_V1,
                was_replay: true,
            });
        };
        if head.frame_id != request.frame_id || head.conversation_id != request.conversation_id {
            return Err(LogosChatGatewayError::SessionConflict);
        }
        outbox.pop_front();
        Ok(LogosChatGatewayAckV1 {
            schema_version: GATEWAY_SCHEMA_VERSION_V1,
            was_replay: false,
        })
    }

    /// Moves a temporarily unsendable head behind other conversations.
    ///
    /// # Errors
    ///
    /// Returns an input, conflict, or dependency error for a bad schema, a
    /// non-head request, or unavailable gateway state.
    pub fn outbox_defer(
        &self,
        request: &LogosChatGatewayOutboxAckRequestV1,
    ) -> Result<LogosChatGatewayAckV1, LogosChatGatewayError> {
        validate_schema(request.schema_version)?;
        let mut outbox = self
            .outbox
            .lock()
            .map_err(|_| LogosChatGatewayError::DependencyUnavailable)?;
        let Some(head) = outbox.front() else {
            return Ok(LogosChatGatewayAckV1 {
                schema_version: GATEWAY_SCHEMA_VERSION_V1,
                was_replay: true,
            });
        };
        if head.frame_id != request.frame_id || head.conversation_id != request.conversation_id {
            return Err(LogosChatGatewayError::SessionConflict);
        }
        let was_replay = outbox.len() <= 1;
        if !was_replay {
            let head = outbox
                .pop_front()
                .ok_or(LogosChatGatewayError::DependencyUnavailable)?;
            outbox.push_back(head);
        }
        Ok(LogosChatGatewayAckV1 {
            schema_version: GATEWAY_SCHEMA_VERSION_V1,
            was_replay,
        })
    }

    /// Returns a path-free status snapshot.
    ///
    /// # Errors
    ///
    /// Returns an input or dependency error for a bad schema or unavailable
    /// gateway state.
    pub fn status(
        &self,
        request: LogosChatGatewayStatusRequestV1,
    ) -> Result<LogosChatGatewayStatusV1, LogosChatGatewayError> {
        validate_schema(request.schema_version)?;
        let queued_frames = self
            .outbox
            .lock()
            .map_err(|_| LogosChatGatewayError::DependencyUnavailable)?
            .len();
        let pending_requests = self
            .pending
            .lock()
            .map_err(|_| LogosChatGatewayError::DependencyUnavailable)?
            .len();
        let session_count = self
            .sessions
            .lock()
            .map_err(|_| LogosChatGatewayError::DependencyUnavailable)?
            .len();
        let discovered_offers = self
            .offer_index
            .lock()
            .map_err(|_| LogosChatGatewayError::DependencyUnavailable)?
            .len();
        Ok(LogosChatGatewayStatusV1 {
            schema_version: GATEWAY_SCHEMA_VERSION_V1,
            role: self.role,
            session_bound: session_count > 0,
            session_count: u16::try_from(session_count)
                .map_err(|_| LogosChatGatewayError::DependencyUnavailable)?,
            queued_frames: u16::try_from(queued_frames)
                .map_err(|_| LogosChatGatewayError::DependencyUnavailable)?,
            pending_requests: u16::try_from(pending_requests)
                .map_err(|_| LogosChatGatewayError::DependencyUnavailable)?,
            discovered_offers: u16::try_from(discovered_offers)
                .map_err(|_| LogosChatGatewayError::DependencyUnavailable)?,
        })
    }

    /// Clears an idle session, queued frames, and replay cache after an explicit
    /// owner action. Active peer work must finish before reset can succeed.
    ///
    /// # Errors
    ///
    /// Returns an input, conflict, or dependency error for a bad schema, active
    /// work, or unavailable gateway state.
    pub fn reset_session(
        &self,
        request: LogosChatGatewayResetRequestV1,
    ) -> Result<LogosChatGatewayAckV1, LogosChatGatewayError> {
        validate_schema(request.schema_version)?;
        let _lifecycle = self
            .lifecycle
            .lock()
            .map_err(|_| LogosChatGatewayError::DependencyUnavailable)?;
        if !self
            .pending
            .lock()
            .map_err(|_| LogosChatGatewayError::DependencyUnavailable)?
            .is_empty()
            || !self
                .inflight_maker_requests
                .lock()
                .map_err(|_| LogosChatGatewayError::DependencyUnavailable)?
                .is_empty()
        {
            return Err(LogosChatGatewayError::SessionConflict);
        }
        let was_replay = self
            .sessions
            .lock()
            .map_err(|_| LogosChatGatewayError::DependencyUnavailable)?
            .is_empty();
        self.sessions
            .lock()
            .map_err(|_| LogosChatGatewayError::DependencyUnavailable)?
            .clear();
        self.outbox
            .lock()
            .map_err(|_| LogosChatGatewayError::DependencyUnavailable)?
            .clear();
        self.cached_responses
            .lock()
            .map_err(|_| LogosChatGatewayError::DependencyUnavailable)?
            .clear();
        *self
            .selected_offer
            .lock()
            .map_err(|_| LogosChatGatewayError::DependencyUnavailable)? = None;
        Ok(LogosChatGatewayAckV1 {
            schema_version: GATEWAY_SCHEMA_VERSION_V1,
            was_replay,
        })
    }

    /// Queues one fixed Taker Chat RPC and waits for the correlated Logos response.
    async fn request(&self, method: &str, parameter: Value) -> Result<Value, RemoteFailureV1> {
        let lifecycle = self
            .lifecycle
            .lock()
            .map_err(|_| remote_failure(LogosChatGatewayError::DependencyUnavailable))?;
        let session = self
            .sessions
            .lock()
            .map_err(|_| remote_failure(LogosChatGatewayError::DependencyUnavailable))?
            .values()
            .next()
            .cloned();
        if self.role != LogosChatGatewayRoleV1::Taker || session.is_none() {
            return Err(remote_failure(LogosChatGatewayError::SessionUnavailable));
        }
        let session = session.expect("checked Taker session");
        if !LOGOS_CHAT_GATEWAY_METHODS_V1.contains(&method) || !parameter.is_object() {
            return Err(remote_failure(LogosChatGatewayError::InvalidInput));
        }
        let selected_offer = parameter
            .get("offer_id")
            .and_then(Value::as_str)
            .and_then(|offer_id| self.selected_offer_key_for_peer(&session.peer_address, offer_id));
        let sequence = self.sequence.fetch_add(1, Ordering::Relaxed);
        let message = LogosChatMessageV1::Request {
            nonce: sequence,
            method: method.into(),
            parameter,
        };
        let frame = LogosChatFrameV1::new(self.role, message).map_err(remote_failure)?;
        let outbox = frame
            .to_outbox(session.conversation_id.clone())
            .map_err(remote_failure)?;
        let frame_id = frame.frame_id.clone();
        let (sender, receiver) = oneshot::channel();
        {
            let mut pending = self
                .pending
                .lock()
                .map_err(|_| remote_failure(LogosChatGatewayError::DependencyUnavailable))?;
            if pending.len() >= MAXIMUM_PENDING_REQUESTS {
                return Err(remote_failure(LogosChatGatewayError::Capacity));
            }
            pending.insert(
                frame_id.clone(),
                PendingResponse {
                    sender,
                    selected_offer,
                },
            );
        }
        if let Err(error) = self.enqueue(outbox) {
            if let Ok(mut pending) = self.pending.lock() {
                pending.remove(&frame_id);
            }
            return Err(remote_failure(error));
        }
        drop(lifecycle);
        let response = tokio::time::timeout(REQUEST_TIMEOUT, receiver).await;
        if let Ok(Ok(result)) = response {
            result
        } else {
            if let Ok(mut pending) = self.pending.lock() {
                pending.remove(&frame_id);
            }
            Err(remote_failure(LogosChatGatewayError::DependencyUnavailable))
        }
    }

    /// Validates and consumes one exact Chat event.
    ///
    /// # Errors
    ///
    /// Returns an input, session, capacity, or dependency error when the event
    /// cannot be accepted and forwarded for this fixed role and binding.
    pub fn ingest(
        self: &Arc<Self>,
        request: &LogosChatGatewayIngestRequestV1,
    ) -> Result<LogosChatGatewayAckV1, LogosChatGatewayError> {
        let _lifecycle = self
            .lifecycle
            .lock()
            .map_err(|_| LogosChatGatewayError::DependencyUnavailable)?;
        validate_schema(request.schema_version)?;
        validate_text(&request.conversation_id, MAXIMUM_CONVERSATION_ID_BYTES)?;
        validate_text(&request.sender_address, MAXIMUM_ADDRESS_BYTES)?;
        if request.content.len() > MAXIMUM_FRAME_BYTES {
            return Err(LogosChatGatewayError::InvalidInput);
        }
        self.validate_session(&request.conversation_id, &request.sender_address)?;
        let frame = LogosChatFrameV1::from_content(&request.content)?;
        if frame.sender_role != self.role.peer() || frame.recipient_role != self.role {
            return Err(LogosChatGatewayError::InvalidInput);
        }
        match (&self.role, frame.message) {
            (
                LogosChatGatewayRoleV1::Maker,
                LogosChatMessageV1::Request {
                    nonce: _,
                    method,
                    parameter,
                },
            ) => self.accept_maker_request(
                request.conversation_id.clone(),
                frame.frame_id,
                method,
                parameter,
            ),
            (
                LogosChatGatewayRoleV1::Taker,
                LogosChatMessageV1::Response {
                    request_frame_id,
                    result,
                    error,
                },
            ) => self.ingest_taker_response(&request_frame_id, result, error),
            _ => Err(LogosChatGatewayError::InvalidInput),
        }
    }

    fn validate_session(
        &self,
        conversation_id: &str,
        sender_address: &str,
    ) -> Result<(), LogosChatGatewayError> {
        let sessions = self
            .sessions
            .lock()
            .map_err(|_| LogosChatGatewayError::DependencyUnavailable)?;
        let Some(session) = sessions.get(conversation_id) else {
            return Err(LogosChatGatewayError::SessionUnavailable);
        };
        if session.peer_address.as_ref() != sender_address {
            return Err(LogosChatGatewayError::SessionConflict);
        }
        Ok(())
    }

    fn accept_maker_request(
        self: &Arc<Self>,
        conversation_id: Box<str>,
        request_frame_id: Box<str>,
        method: Box<str>,
        parameter: Value,
    ) -> Result<LogosChatGatewayAckV1, LogosChatGatewayError> {
        let mut inflight = self
            .inflight_maker_requests
            .lock()
            .map_err(|_| LogosChatGatewayError::DependencyUnavailable)?;
        let request_key = (conversation_id.clone(), request_frame_id.clone());
        if inflight.contains(&request_key) {
            return Ok(LogosChatGatewayAckV1 {
                schema_version: GATEWAY_SCHEMA_VERSION_V1,
                was_replay: true,
            });
        }
        let cached = self
            .cached_responses
            .lock()
            .map_err(|_| LogosChatGatewayError::DependencyUnavailable)?
            .iter()
            .find(|cached| {
                cached.request_frame_id == request_frame_id
                    && cached.outbox.conversation_id == conversation_id
            })
            .cloned();
        if let Some(cached) = cached {
            drop(inflight);
            self.enqueue(cached.outbox)?;
            return Ok(LogosChatGatewayAckV1 {
                schema_version: GATEWAY_SCHEMA_VERSION_V1,
                was_replay: true,
            });
        }
        if inflight.len() >= MAXIMUM_INFLIGHT_MAKER_REQUESTS {
            return Err(LogosChatGatewayError::Capacity);
        }
        inflight.insert(request_key.clone());
        drop(inflight);

        let gateway = Arc::clone(self);
        let _task = tokio::spawn(async move {
            let _inflight_guard = InflightMakerRequestGuard {
                gateway: Arc::clone(&gateway),
                request_key,
            };
            let _ = gateway
                .process_maker_request(
                    conversation_id,
                    request_frame_id.clone(),
                    &method,
                    &parameter,
                )
                .await;
        });
        Ok(LogosChatGatewayAckV1 {
            schema_version: GATEWAY_SCHEMA_VERSION_V1,
            was_replay: false,
        })
    }

    async fn process_maker_request(
        &self,
        conversation_id: Box<str>,
        request_frame_id: Box<str>,
        method: &str,
        parameter: &Value,
    ) -> Result<(), LogosChatGatewayError> {
        let maker_chat_socket = self
            .maker_chat_socket
            .as_ref()
            .ok_or(LogosChatGatewayError::DependencyUnavailable)?;
        let response =
            match call_local_chat_rpc::<_, Value>(maker_chat_socket, method, parameter).await {
                Ok(result) if !result.is_null() => LogosChatMessageV1::Response {
                    request_frame_id: request_frame_id.clone(),
                    result: Some(result),
                    error: None,
                },
                Ok(_) => LogosChatMessageV1::Response {
                    request_frame_id: request_frame_id.clone(),
                    result: None,
                    error: Some(remote_failure(LogosChatGatewayError::DependencyUnavailable)),
                },
                Err(error) => LogosChatMessageV1::Response {
                    request_frame_id: request_frame_id.clone(),
                    result: None,
                    error: Some(maker_remote_failure(&error)),
                },
            };
        let response_frame = LogosChatFrameV1::new(self.role, response)?;
        let outbox = match response_frame.to_outbox(conversation_id.clone()) {
            Ok(outbox) => outbox,
            Err(LogosChatGatewayError::InvalidInput) => LogosChatFrameV1::new(
                self.role,
                LogosChatMessageV1::Response {
                    request_frame_id: request_frame_id.clone(),
                    result: None,
                    error: Some(remote_failure(LogosChatGatewayError::DependencyUnavailable)),
                },
            )?
            .to_outbox(conversation_id)?,
            Err(error) => return Err(error),
        };
        self.enqueue(outbox.clone())?;
        let mut cache = self
            .cached_responses
            .lock()
            .map_err(|_| LogosChatGatewayError::DependencyUnavailable)?;
        cache.push_back(CachedResponse {
            request_frame_id,
            outbox,
        });
        while cache.len() > MAXIMUM_CACHED_RESPONSES {
            cache.pop_front();
        }
        Ok(())
    }

    fn ingest_taker_response(
        &self,
        request_frame_id: &str,
        result: Option<Value>,
        error: Option<RemoteFailureV1>,
    ) -> Result<LogosChatGatewayAckV1, LogosChatGatewayError> {
        let pending = self
            .pending
            .lock()
            .map_err(|_| LogosChatGatewayError::DependencyUnavailable)?
            .remove(request_frame_id);
        let Some(pending) = pending else {
            return Ok(LogosChatGatewayAckV1 {
                schema_version: GATEWAY_SCHEMA_VERSION_V1,
                was_replay: true,
            });
        };
        if error.as_ref().is_some_and(|failure| {
            failure.code == MAKER_OFFER_UNAVAILABLE_CODE
                || failure.code == MAKER_APPLICATION_CONFLICT_CODE
        }) && let Some(key) = pending.selected_offer.clone()
            && let Ok(index) = self.offer_index.lock()
            && let Ok(mut unavailable) = self.locally_unavailable_offers.lock()
        {
            unavailable.retain(|existing| index.contains_key(existing));
            if index.contains_key(&key)
                && (unavailable.contains(&key) || unavailable.len() < MAXIMUM_INDEXED_OFFERS)
            {
                unavailable.insert(key);
            }
        }
        let response = match (result, error) {
            (Some(result), None) => Ok(result),
            (None, Some(error)) => Err(error),
            _ => return Err(LogosChatGatewayError::InvalidInput),
        };
        let _ = pending.sender.send(response);
        Ok(LogosChatGatewayAckV1 {
            schema_version: GATEWAY_SCHEMA_VERSION_V1,
            was_replay: false,
        })
    }

    /// Authenticates and indexes one exact Delivery announcement.
    ///
    /// # Errors
    ///
    /// Returns an input, conflict, capacity, or dependency error when the
    /// announcement is invalid, regresses signed state, exceeds a bound, or
    /// gateway state is unavailable.
    pub fn ingest_offer_announcement(
        &self,
        request: &LogosOfferIngestRequestV1,
    ) -> Result<LogosChatGatewayAckV1, LogosChatGatewayError> {
        validate_schema(request.schema_version)?;
        if self.role != LogosChatGatewayRoleV1::Taker
            || request.payload_base64.is_empty()
            || request.payload_base64.len() > MAXIMUM_LOGOS_OFFER_ANNOUNCEMENT_BASE64_BYTES
        {
            return Err(LogosChatGatewayError::InvalidInput);
        }
        let encoded = BASE64_STANDARD
            .decode(request.payload_base64.as_bytes())
            .map_err(|_| LogosChatGatewayError::InvalidInput)?;
        if BASE64_STANDARD.encode(&encoded) != request.payload_base64.as_ref() {
            return Err(LogosChatGatewayError::InvalidInput);
        }
        let now = (self.trusted_clock)()?;
        let announcement = verify_logos_offer_announcement(&encoded, now)
            .map_err(|_| LogosChatGatewayError::InvalidInput)?;
        let key = (
            *announcement.offer().maker_identity(),
            announcement.offer().offer().id().as_str().into(),
        );
        let mut index = self
            .offer_index
            .lock()
            .map_err(|_| LogosChatGatewayError::DependencyUnavailable)?;
        index.retain(|_, entry| entry.announcement.valid_until_unix_seconds() > now);
        let clear_local_unavailable;
        if let Some(existing) = index.get(&key) {
            let immutable_matches = existing.announcement.offer().signed_envelope()
                == announcement.offer().signed_envelope();
            if !immutable_matches {
                return Err(LogosChatGatewayError::SessionConflict);
            }
            let existing_order = (
                existing.announcement.offer_revision(),
                existing.announcement.announced_at_unix_seconds(),
            );
            let new_order = (
                announcement.offer_revision(),
                announcement.announced_at_unix_seconds(),
            );
            if new_order < existing_order {
                return Ok(LogosChatGatewayAckV1 {
                    schema_version: GATEWAY_SCHEMA_VERSION_V1,
                    was_replay: true,
                });
            }
            if new_order == existing_order {
                if existing.announcement.encoded() != announcement.encoded() {
                    return Err(LogosChatGatewayError::SessionConflict);
                }
                return Ok(LogosChatGatewayAckV1 {
                    schema_version: GATEWAY_SCHEMA_VERSION_V1,
                    was_replay: true,
                });
            }
            if existing.announcement.status() != MakerOfferStatus::Active
                && announcement.status() == MakerOfferStatus::Active
            {
                return Err(LogosChatGatewayError::SessionConflict);
            }
            clear_local_unavailable = announcement.status() == MakerOfferStatus::Active;
        } else {
            let same_maker_entries = index.keys().filter(|existing| existing.0 == key.0).count();
            if same_maker_entries >= MAXIMUM_INDEXED_OFFERS_PER_MAKER
                || index.len() >= MAXIMUM_INDEXED_OFFERS
            {
                // Never erase a still-live signed ordering state to admit an
                // unrelated key. Existing entries continue to accept newer
                // signed revisions; lease expiry deterministically frees slots.
                return Err(LogosChatGatewayError::Capacity);
            }
            clear_local_unavailable = announcement.status() == MakerOfferStatus::Active;
            index.insert(key.clone(), IndexedOfferV1 { announcement });
            drop(index);
            if clear_local_unavailable {
                self.locally_unavailable_offers
                    .lock()
                    .map_err(|_| LogosChatGatewayError::DependencyUnavailable)?
                    .remove(&key);
            }
            return Ok(LogosChatGatewayAckV1 {
                schema_version: GATEWAY_SCHEMA_VERSION_V1,
                was_replay: false,
            });
        }
        index.insert(key.clone(), IndexedOfferV1 { announcement });
        drop(index);
        if clear_local_unavailable {
            self.locally_unavailable_offers
                .lock()
                .map_err(|_| LogosChatGatewayError::DependencyUnavailable)?
                .remove(&key);
        }
        Ok(LogosChatGatewayAckV1 {
            schema_version: GATEWAY_SCHEMA_VERSION_V1,
            was_replay: false,
        })
    }

    /// Lists only live, signed, locally uncontended offers.
    ///
    /// # Errors
    ///
    /// Returns an input or dependency error for the wrong fixed role, an
    /// unavailable trusted clock, invalid indexed data, or unavailable state.
    pub fn list_offer_announcements(
        &self,
        request: LogosOfferListRequestV1,
    ) -> Result<LogosOfferListV1, LogosChatGatewayError> {
        validate_schema(request.schema_version)?;
        if self.role != LogosChatGatewayRoleV1::Taker {
            return Err(LogosChatGatewayError::InvalidInput);
        }
        let now = (self.trusted_clock)()?;
        let unavailable_snapshot = self
            .locally_unavailable_offers
            .lock()
            .map_err(|_| LogosChatGatewayError::DependencyUnavailable)?
            .clone();
        let (offers, unavailable_offers, locally_contended_offers, omitted_offers, live_keys) = {
            let mut index = self
                .offer_index
                .lock()
                .map_err(|_| LogosChatGatewayError::DependencyUnavailable)?;
            index.retain(|_, entry| entry.announcement.valid_until_unix_seconds() > now);
            let live_keys = index.keys().cloned().collect::<BTreeSet<_>>();
            let mut unavailable_offers = 0_u16;
            let mut locally_contended_offers = 0_u16;
            let mut active = Vec::new();
            for (key, entry) in index.iter() {
                let announcement = &entry.announcement;
                if request
                    .route
                    .is_some_and(|route| route != announcement.offer().offer().route())
                {
                    continue;
                }
                if announcement.status() != MakerOfferStatus::Active {
                    unavailable_offers = unavailable_offers.saturating_add(1);
                } else if unavailable_snapshot.contains(key) {
                    locally_contended_offers = locally_contended_offers.saturating_add(1);
                } else {
                    active.push(announcement);
                }
            }
            active.sort_by(|left, right| {
                right
                    .offer()
                    .offer()
                    .created_at_unix_seconds()
                    .cmp(&left.offer().offer().created_at_unix_seconds())
                    .then_with(|| {
                        left.offer()
                            .offer()
                            .id()
                            .as_str()
                            .cmp(right.offer().offer().id().as_str())
                    })
                    .then_with(|| {
                        left.offer()
                            .maker_identity()
                            .cmp(right.offer().maker_identity())
                    })
            });
            let omitted_offers = u16::try_from(active.len().saturating_sub(MAXIMUM_LISTED_OFFERS))
                .map_err(|_| LogosChatGatewayError::DependencyUnavailable)?;
            let offers = active
                .into_iter()
                .take(MAXIMUM_LISTED_OFFERS)
                .map(offer_view)
                .collect::<Result<Vec<_>, _>>()?;
            (
                offers,
                unavailable_offers,
                locally_contended_offers,
                omitted_offers,
                live_keys,
            )
        };
        let mut unavailable = self
            .locally_unavailable_offers
            .lock()
            .map_err(|_| LogosChatGatewayError::DependencyUnavailable)?;
        unavailable.retain(|key| live_keys.contains(key));
        Ok(LogosOfferListV1 {
            schema_version: GATEWAY_SCHEMA_VERSION_V1,
            offers,
            unavailable_offers,
            locally_contended_offers,
            omitted_offers,
        })
    }

    /// Resolves an active reviewed offer to its signed current Chat address.
    ///
    /// # Errors
    ///
    /// Returns an input, session, conflict, or dependency error when the offer
    /// is not live and selectable or its Chat binding cannot be used exactly.
    pub fn select_offer_announcement(
        &self,
        request: &LogosOfferSelectRequestV1,
    ) -> Result<LogosOfferSelectionV1, LogosChatGatewayError> {
        validate_schema(request.schema_version)?;
        if self.role != LogosChatGatewayRoleV1::Taker {
            return Err(LogosChatGatewayError::InvalidInput);
        }
        let now = (self.trusted_clock)()?;
        let key = (
            *request.maker_identity.as_bytes(),
            request.offer_id.as_str().into(),
        );
        if self
            .locally_unavailable_offers
            .lock()
            .map_err(|_| LogosChatGatewayError::DependencyUnavailable)?
            .contains(&key)
        {
            return Err(LogosChatGatewayError::SessionConflict);
        }
        let index = self
            .offer_index
            .lock()
            .map_err(|_| LogosChatGatewayError::DependencyUnavailable)?;
        let announcement = &index
            .get(&key)
            .ok_or(LogosChatGatewayError::SessionUnavailable)?
            .announcement;
        if announcement.status() != MakerOfferStatus::Active
            || announcement.valid_until_unix_seconds() <= now
        {
            return Err(LogosChatGatewayError::SessionUnavailable);
        }
        let selected = offer_view(announcement)?;
        let marker = SelectedOfferV1 {
            key,
            maker_chat_address: announcement.maker_chat_address().into(),
        };
        drop(index);
        if self
            .sessions
            .lock()
            .map_err(|_| LogosChatGatewayError::DependencyUnavailable)?
            .values()
            .next()
            .is_some_and(|session| session.peer_address != marker.maker_chat_address)
        {
            return Err(LogosChatGatewayError::SessionConflict);
        }
        *self
            .selected_offer
            .lock()
            .map_err(|_| LogosChatGatewayError::DependencyUnavailable)? = Some(marker);
        Ok(LogosOfferSelectionV1 {
            schema_version: GATEWAY_SCHEMA_VERSION_V1,
            selected,
        })
    }

    fn selected_offer_key_for_peer(
        &self,
        peer_address: &str,
        offer_id: &str,
    ) -> Option<([u8; 33], Box<str>)> {
        let selected = self.selected_offer.lock().ok()?.clone()?;
        (selected.maker_chat_address.as_ref() == peer_address
            && selected.key.1.as_ref() == offer_id)
            .then_some(selected.key)
    }

    fn enqueue(&self, frame: LogosChatGatewayOutboxItemV1) -> Result<(), LogosChatGatewayError> {
        let mut outbox = self
            .outbox
            .lock()
            .map_err(|_| LogosChatGatewayError::DependencyUnavailable)?;
        if outbox.len() >= MAXIMUM_QUEUED_FRAMES {
            return Err(LogosChatGatewayError::Capacity);
        }
        if !outbox.iter().any(|queued| {
            queued.frame_id == frame.frame_id && queued.conversation_id == frame.conversation_id
        }) {
            outbox.push_back(frame);
        }
        Ok(())
    }
}

fn offer_view(
    announcement: &crate::AuthenticatedLogosOfferAnnouncementV1,
) -> Result<LogosOfferViewV1, LogosChatGatewayError> {
    let maker_identity = TakerMakerIdentityV1::new(*announcement.offer().maker_identity())
        .map_err(|_| LogosChatGatewayError::InvalidInput)?;
    Ok(LogosOfferViewV1 {
        offer: announcement.offer().offer().clone(),
        maker_identity,
        signed_envelope_sha256: announcement.offer().commitment(),
        maker_chat_address: announcement.maker_chat_address().into(),
        offer_revision: announcement.offer_revision(),
        availability: announcement.status(),
        valid_until_unix_seconds: announcement.valid_until_unix_seconds(),
        announcement_base64: BASE64_STANDARD
            .encode(announcement.encoded())
            .into_boxed_str(),
    })
}

fn trusted_now() -> Result<u64, ()> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|_| ())
}

fn validate_schema(schema_version: u16) -> Result<(), LogosChatGatewayError> {
    if schema_version == GATEWAY_SCHEMA_VERSION_V1 {
        Ok(())
    } else {
        Err(LogosChatGatewayError::InvalidInput)
    }
}

fn validate_text(value: &str, maximum: usize) -> Result<(), LogosChatGatewayError> {
    if value.is_empty() || value.len() > maximum || value.chars().any(char::is_control) {
        Err(LogosChatGatewayError::InvalidInput)
    } else {
        Ok(())
    }
}

fn remote_failure(error: LogosChatGatewayError) -> RemoteFailureV1 {
    let (code, message) = match error {
        LogosChatGatewayError::InvalidInput => (INVALID_PARAMS_CODE, "Invalid Chat gateway input"),
        LogosChatGatewayError::SessionConflict => (CONFLICT_CODE, "Chat session conflict"),
        LogosChatGatewayError::SessionUnavailable => {
            (DEPENDENCY_UNAVAILABLE_CODE, "Chat session unavailable")
        }
        LogosChatGatewayError::Capacity => (
            DEPENDENCY_UNAVAILABLE_CODE,
            "Chat gateway capacity exhausted",
        ),
        LogosChatGatewayError::DependencyUnavailable => (
            DEPENDENCY_UNAVAILABLE_CODE,
            "Chat peer operation unavailable",
        ),
    };
    RemoteFailureV1 {
        code,
        message: message.into(),
    }
}

fn maker_remote_failure(error: &anyhow::Error) -> RemoteFailureV1 {
    let Some(remote) = error.downcast_ref::<LocalRpcRemoteError>() else {
        return remote_failure(LogosChatGatewayError::DependencyUnavailable);
    };
    if validate_text(&remote.message, MAXIMUM_REMOTE_FAILURE_MESSAGE_BYTES).is_err() {
        return remote_failure(LogosChatGatewayError::DependencyUnavailable);
    }
    RemoteFailureV1 {
        code: remote.code,
        message: remote.message.clone(),
    }
}

fn rpc_error(error: LogosChatGatewayError) -> ErrorObjectOwned {
    let remote = remote_failure(error);
    ErrorObjectOwned::owned(
        remote.code,
        remote.message,
        Some(json!({ "category": "logos_chat_gateway" })),
    )
}

/// Builds the fixed Basecamp-to-gateway control surface.
///
/// # Errors
///
/// Returns a registration error if a fixed method name cannot be installed.
pub fn logos_chat_gateway_control_rpc_module(
    gateway: Arc<LogosChatGateway>,
) -> Result<RpcModule<Arc<LogosChatGateway>>, RegisterMethodError> {
    let mut module = RpcModule::new(gateway);
    module.register_method("logos_chat_bind_session_v1", |params, gateway, _| {
        let request: LogosChatGatewayBindRequestV1 = params
            .one()
            .map_err(|_| rpc_error(LogosChatGatewayError::InvalidInput))?;
        gateway.bind_session(&request).map_err(rpc_error)
    })?;
    module.register_method("logos_chat_outbox_peek_v1", |params, gateway, _| {
        let request: LogosChatGatewayOutboxRequestV1 = params
            .one()
            .map_err(|_| rpc_error(LogosChatGatewayError::InvalidInput))?;
        gateway.outbox_peek(request).map_err(rpc_error)
    })?;
    module.register_method("logos_chat_outbox_ack_v1", |params, gateway, _| {
        let request: LogosChatGatewayOutboxAckRequestV1 = params
            .one()
            .map_err(|_| rpc_error(LogosChatGatewayError::InvalidInput))?;
        gateway.outbox_ack(&request).map_err(rpc_error)
    })?;
    module.register_method("logos_chat_outbox_defer_v1", |params, gateway, _| {
        let request: LogosChatGatewayOutboxAckRequestV1 = params
            .one()
            .map_err(|_| rpc_error(LogosChatGatewayError::InvalidInput))?;
        gateway.outbox_defer(&request).map_err(rpc_error)
    })?;
    module.register_method("logos_chat_ingest_v1", |params, gateway, _| {
        let request: LogosChatGatewayIngestRequestV1 = params
            .one()
            .map_err(|_| rpc_error(LogosChatGatewayError::InvalidInput))?;
        gateway.ingest(&request).map_err(rpc_error)
    })?;
    module.register_method("logos_chat_status_v1", |params, gateway, _| {
        let request: LogosChatGatewayStatusRequestV1 = params
            .one()
            .map_err(|_| rpc_error(LogosChatGatewayError::InvalidInput))?;
        gateway.status(request).map_err(rpc_error)
    })?;
    module.register_method("logos_chat_reset_session_v1", |params, gateway, _| {
        let request: LogosChatGatewayResetRequestV1 = params
            .one()
            .map_err(|_| rpc_error(LogosChatGatewayError::InvalidInput))?;
        gateway.reset_session(request).map_err(rpc_error)
    })?;
    module.register_method("logos_offer_ingest_v1", |params, gateway, _| {
        let request: LogosOfferIngestRequestV1 = params
            .one()
            .map_err(|_| rpc_error(LogosChatGatewayError::InvalidInput))?;
        gateway
            .ingest_offer_announcement(&request)
            .map_err(rpc_error)
    })?;
    module.register_method("logos_offer_list_v1", |params, gateway, _| {
        let request: LogosOfferListRequestV1 = params
            .one()
            .map_err(|_| rpc_error(LogosChatGatewayError::InvalidInput))?;
        gateway.list_offer_announcements(request).map_err(rpc_error)
    })?;
    module.register_method("logos_offer_select_v1", |params, gateway, _| {
        let request: LogosOfferSelectRequestV1 = params
            .one()
            .map_err(|_| rpc_error(LogosChatGatewayError::InvalidInput))?;
        gateway
            .select_offer_announcement(&request)
            .map_err(rpc_error)
    })?;
    Ok(module)
}

/// Builds the fixed Taker-facing proxy surface. The real Taker CLI/service uses
/// this socket exactly as it previously used the Maker's direct Chat socket.
///
/// # Errors
///
/// Returns a registration error for a Maker endpoint or if a fixed method name
/// cannot be installed.
pub fn logos_chat_gateway_proxy_rpc_module(
    gateway: Arc<LogosChatGateway>,
) -> Result<RpcModule<Arc<LogosChatGateway>>, RegisterMethodError> {
    if gateway.role != LogosChatGatewayRoleV1::Taker {
        return Err(RegisterMethodError::AlreadyRegistered(
            "maker_gateway_has_no_proxy".to_owned(),
        ));
    }
    let mut module = RpcModule::new(gateway);
    for method in LOGOS_CHAT_GATEWAY_METHODS_V1 {
        module.register_async_method(method, move |params, gateway, _| async move {
            let parameter: Value = params
                .one()
                .map_err(|_| rpc_error(LogosChatGatewayError::InvalidInput))?;
            gateway.request(method, parameter).await.map_err(|failure| {
                ErrorObjectOwned::owned(failure.code, failure.message, None::<()>)
            })
        })?;
    }
    Ok(module)
}

#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeSet,
        sync::{
            Arc,
            atomic::{AtomicU64, Ordering},
        },
        time::{Duration, SystemTime, UNIX_EPOCH},
    };

    use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
    use lez_bridge_protocol::RequestId;
    use lez_swap_core::{Pair, SwapDirection};
    use lez_swap_store::{
        LocalPriceV1, MakerOfferId, MakerPairConfigurationV1, MakerPriceSourceKind, MakerRouteV1,
        SqliteSwapStore,
    };
    use secp256k1::{PublicKey, Secp256k1, SecretKey};
    use serde_json::{Value, json};
    use tokio::sync::oneshot;

    use crate::{
        RunLocalDelivery, TakerMakerIdentityV1, local_rpc::LocalRpcRemoteError,
        run_local_delivery::MAXIMUM_LOGOS_OFFER_ANNOUNCEMENT_BASE64_BYTES,
        verify_logos_offer_announcement,
    };

    use super::{
        GATEWAY_SCHEMA_VERSION_V1, IndexedOfferV1, LogosChatFrameV1, LogosChatGateway,
        LogosChatGatewayBindRequestV1, LogosChatGatewayError, LogosChatGatewayIngestRequestV1,
        LogosChatGatewayOutboxAckRequestV1, LogosChatGatewayOutboxRequestV1,
        LogosChatGatewayResetRequestV1, LogosChatGatewayRoleV1, LogosChatMessageV1,
        LogosOfferIngestRequestV1, LogosOfferSelectRequestV1, MAKER_OFFER_UNAVAILABLE_CODE,
        MAXIMUM_INDEXED_OFFERS, PendingResponse, RemoteFailureV1, maker_remote_failure,
    };

    fn request_id(value: &str) -> RequestId {
        RequestId::new(value).unwrap()
    }

    type ActiveAnnouncementsFixture = (
        tempfile::TempDir,
        Vec<u8>,
        Vec<u8>,
        Vec<u8>,
        [u8; 33],
        MakerOfferId,
        u64,
    );

    fn active_announcements_with_key(signing_key_byte: u8) -> ActiveAnnouncementsFixture {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let directory = tempfile::tempdir().unwrap();
        let mut store = SqliteSwapStore::open(directory.path().join("offers.sqlite3")).unwrap();
        let route = MakerRouteV1::new(Pair::Zcash, SwapDirection::TakerSellsLez).unwrap();
        let disabled = MakerPairConfigurationV1::new(
            route,
            false,
            MakerPriceSourceKind::Local,
            10,
            10_000,
            300,
        )
        .unwrap();
        store
            .configure_maker_pair(&request_id("loser-pair-create"), None, &disabled)
            .unwrap();
        store
            .set_local_price(
                &request_id("loser-price-create"),
                None,
                &LocalPriceV1::new(route, 5, 2).unwrap(),
            )
            .unwrap();
        let enabled = MakerPairConfigurationV1::new(
            route,
            true,
            MakerPriceSourceKind::Local,
            10,
            10_000,
            300,
        )
        .unwrap();
        store
            .configure_maker_pair(&request_id("loser-pair-enable"), Some(1), &enabled)
            .unwrap();
        let offer_id = MakerOfferId::new("loser-marker-offer-001").unwrap();
        store
            .publish_local_offer(&request_id("loser-publish"), &offer_id, route, now)
            .unwrap();
        let record = store.list_maker_offer_history(now).unwrap().remove(0);
        let signing_key = SecretKey::from_slice(&[signing_key_byte; 32]).unwrap();
        let maker_identity =
            PublicKey::from_secret_key(&Secp256k1::signing_only(), &signing_key).serialize();
        let publisher =
            RunLocalDelivery::publisher(directory.path().join("delivery"), signing_key).unwrap();
        let announcement = publisher
            .sign_logos_offer_announcement(&record, "logos://maker-recovered", now)
            .unwrap();
        let refreshed = publisher
            .sign_logos_offer_announcement(&record, "logos://maker-recovered", now + 1)
            .unwrap();
        let reinserted = publisher
            .sign_logos_offer_announcement(&record, "logos://maker-recovered", now + 31)
            .unwrap();
        (
            directory,
            announcement,
            refreshed,
            reinserted,
            maker_identity,
            offer_id,
            now,
        )
    }

    fn active_announcements() -> ActiveAnnouncementsFixture {
        active_announcements_with_key(77)
    }

    #[test]
    fn frame_is_content_addressed_and_rejects_mutation() {
        let frame = LogosChatFrameV1::new(
            LogosChatGatewayRoleV1::Taker,
            LogosChatMessageV1::Request {
                nonce: 0,
                method: "btc_chat_propose_v2".into(),
                parameter: json!({ "schema_version": 2, "wire": [1, 2, 3] }),
            },
        )
        .unwrap();
        let outbox = frame.to_outbox("conversation-1".into()).unwrap();
        assert_eq!(
            LogosChatFrameV1::from_content(&outbox.content).unwrap(),
            frame
        );

        let mut value: serde_json::Value = serde_json::from_str(&outbox.content).unwrap();
        value["message"]["parameter"]["wire"] = json!([1, 2, 4]);
        assert_eq!(
            LogosChatFrameV1::from_content(&serde_json::to_string(&value).unwrap()),
            Err(LogosChatGatewayError::InvalidInput)
        );
    }

    #[test]
    fn response_frame_rejects_an_ambiguous_json_null_result() {
        assert_eq!(
            LogosChatFrameV1::new(
                LogosChatGatewayRoleV1::Maker,
                LogosChatMessageV1::Response {
                    request_frame_id: "0".repeat(64).into_boxed_str(),
                    result: Some(Value::Null),
                    error: None,
                },
            ),
            Err(LogosChatGatewayError::InvalidInput)
        );
    }

    #[test]
    fn failed_conversation_head_defers_behind_another_peer() {
        let directory = tempfile::tempdir().unwrap();
        let gateway = LogosChatGateway::new(
            LogosChatGatewayRoleV1::Maker,
            Some(directory.path().join("maker-chat.sock")),
        )
        .unwrap();
        for (nonce, conversation) in [(1, "conversation-a"), (2, "conversation-b")] {
            let frame = LogosChatFrameV1::new(
                LogosChatGatewayRoleV1::Maker,
                LogosChatMessageV1::Response {
                    request_frame_id: format!("{nonce:064x}").into(),
                    result: Some(json!({"nonce": nonce})),
                    error: None,
                },
            )
            .unwrap()
            .to_outbox(conversation.into())
            .unwrap();
            gateway.enqueue(frame).unwrap();
        }
        let first = gateway
            .outbox_peek(LogosChatGatewayOutboxRequestV1 {
                schema_version: GATEWAY_SCHEMA_VERSION_V1,
            })
            .unwrap()
            .unwrap();
        assert_eq!(first.conversation_id.as_ref(), "conversation-a");
        gateway
            .outbox_defer(&LogosChatGatewayOutboxAckRequestV1 {
                schema_version: GATEWAY_SCHEMA_VERSION_V1,
                frame_id: first.frame_id,
                conversation_id: first.conversation_id,
            })
            .unwrap();
        assert_eq!(
            gateway
                .outbox_peek(LogosChatGatewayOutboxRequestV1 {
                    schema_version: GATEWAY_SCHEMA_VERSION_V1,
                })
                .unwrap()
                .unwrap()
                .conversation_id
                .as_ref(),
            "conversation-b"
        );
    }

    #[test]
    fn bounded_maker_rpc_error_keeps_its_diagnostic() {
        let error = anyhow::Error::new(LocalRpcRemoteError {
            code: -32_045,
            message: "requires Chat completion v2".into(),
        });
        let failure = maker_remote_failure(&error);
        assert_eq!(failure.code, -32_045);
        assert_eq!(failure.message.as_ref(), "requires Chat completion v2");

        let oversized = anyhow::Error::new(LocalRpcRemoteError {
            code: -32_045,
            message: "x".repeat(4 * 1024 + 1).into_boxed_str(),
        });
        assert_ne!(
            maker_remote_failure(&oversized).message.as_ref(),
            oversized
                .downcast_ref::<LocalRpcRemoteError>()
                .unwrap()
                .message
                .as_ref()
        );
    }

    #[test]
    fn one_direct_session_is_idempotent_but_conflicts_fail_closed() {
        let gateway = Arc::new(LogosChatGateway::new(LogosChatGatewayRoleV1::Taker, None).unwrap());
        let binding = LogosChatGatewayBindRequestV1 {
            schema_version: GATEWAY_SCHEMA_VERSION_V1,
            conversation_id: "conversation-1".into(),
            local_address: "local://taker".into(),
            peer_address: "local://maker".into(),
        };
        assert!(!gateway.bind_session(&binding).unwrap().was_replay);
        assert!(gateway.bind_session(&binding).unwrap().was_replay);
        let conflicting = LogosChatGatewayBindRequestV1 {
            conversation_id: "conversation-2".into(),
            ..binding
        };
        assert_eq!(
            gateway.bind_session(&conflicting),
            Err(LogosChatGatewayError::SessionConflict)
        );
        assert!(
            !gateway
                .reset_session(LogosChatGatewayResetRequestV1 {
                    schema_version: GATEWAY_SCHEMA_VERSION_V1,
                })
                .unwrap()
                .was_replay
        );
        assert!(!gateway.bind_session(&conflicting).unwrap().was_replay);
    }

    #[test]
    #[allow(
        clippy::too_many_lines,
        reason = "one scenario preserves correlated loser state across refresh, expiry, and reinsertion"
    )]
    fn active_reinsert_after_lease_expiry_clears_a_correlated_local_loser_marker() {
        let (_directory, announcement, refreshed, reinserted, maker_identity, offer_id, now) =
            active_announcements();
        let clock = Arc::new(AtomicU64::new(now));
        let gateway_clock = Arc::clone(&clock);
        let gateway =
            LogosChatGateway::new_with_clock(LogosChatGatewayRoleV1::Taker, None, move || {
                Ok(gateway_clock.load(Ordering::Relaxed))
            })
            .unwrap();
        let payload_base64 = BASE64_STANDARD.encode(&announcement).into_boxed_str();
        gateway
            .ingest_offer_announcement(&LogosOfferIngestRequestV1 {
                schema_version: GATEWAY_SCHEMA_VERSION_V1,
                payload_base64: payload_base64.clone(),
            })
            .unwrap();
        let key = (maker_identity, offer_id.as_str().into());
        let (sender, _receiver) = oneshot::channel();
        gateway.pending.lock().unwrap().insert(
            "correlated-frame".into(),
            PendingResponse {
                sender,
                selected_offer: Some(key.clone()),
            },
        );
        gateway
            .ingest_taker_response(
                "correlated-frame",
                None,
                Some(RemoteFailureV1 {
                    code: MAKER_OFFER_UNAVAILABLE_CODE,
                    message: "offer unavailable".into(),
                }),
            )
            .unwrap();
        assert!(
            gateway
                .locally_unavailable_offers
                .lock()
                .unwrap()
                .contains(&key)
        );
        assert!(matches!(
            gateway.select_offer_announcement(&LogosOfferSelectRequestV1 {
                schema_version: GATEWAY_SCHEMA_VERSION_V1,
                maker_identity: TakerMakerIdentityV1::new(maker_identity).unwrap(),
                offer_id: offer_id.clone(),
            }),
            Err(LogosChatGatewayError::SessionConflict)
        ));

        gateway
            .ingest_offer_announcement(&LogosOfferIngestRequestV1 {
                schema_version: GATEWAY_SCHEMA_VERSION_V1,
                payload_base64: BASE64_STANDARD.encode(&refreshed).into(),
            })
            .unwrap();
        assert!(
            !gateway
                .locally_unavailable_offers
                .lock()
                .unwrap()
                .contains(&key)
        );

        let (sender, _receiver) = oneshot::channel();
        gateway.pending.lock().unwrap().insert(
            "second-correlated-frame".into(),
            PendingResponse {
                sender,
                selected_offer: Some(key.clone()),
            },
        );
        gateway
            .ingest_taker_response(
                "second-correlated-frame",
                None,
                Some(RemoteFailureV1 {
                    code: MAKER_OFFER_UNAVAILABLE_CODE,
                    message: "offer unavailable".into(),
                }),
            )
            .unwrap();

        clock.store(now + 31, Ordering::Relaxed);
        let expired = gateway
            .list_offer_announcements(super::LogosOfferListRequestV1 {
                schema_version: GATEWAY_SCHEMA_VERSION_V1,
                route: None,
            })
            .unwrap();
        assert!(expired.offers.is_empty());
        assert!(
            !gateway
                .locally_unavailable_offers
                .lock()
                .unwrap()
                .contains(&key)
        );
        gateway
            .ingest_offer_announcement(&LogosOfferIngestRequestV1 {
                schema_version: GATEWAY_SCHEMA_VERSION_V1,
                payload_base64: BASE64_STANDARD.encode(reinserted).into(),
            })
            .unwrap();
        assert!(
            !gateway
                .locally_unavailable_offers
                .lock()
                .unwrap()
                .contains(&key)
        );
        gateway
            .select_offer_announcement(&LogosOfferSelectRequestV1 {
                schema_version: GATEWAY_SCHEMA_VERSION_V1,
                maker_identity: TakerMakerIdentityV1::new(maker_identity).unwrap(),
                offer_id,
            })
            .unwrap();
    }

    #[test]
    fn full_index_preserves_live_ordering_state_and_rejects_unrelated_keys() {
        let (_directory, announcement, _, _, maker_identity, offer_id, now) =
            active_announcements();
        let gateway =
            LogosChatGateway::new_with_clock(LogosChatGatewayRoleV1::Taker, None, move || Ok(now))
                .unwrap();
        let indexed = IndexedOfferV1 {
            announcement: verify_logos_offer_announcement(&announcement, now).unwrap(),
        };
        {
            let mut index = gateway.offer_index.lock().unwrap();
            for marker in 0..MAXIMUM_INDEXED_OFFERS {
                let mut synthetic_maker = [0_u8; 33];
                synthetic_maker[0] = 2;
                synthetic_maker[1..9].copy_from_slice(&(marker as u64).to_be_bytes());
                assert_ne!(synthetic_maker, maker_identity);
                index.insert(
                    (
                        synthetic_maker,
                        format!("synthetic-offer-{marker:04}").into(),
                    ),
                    indexed.clone(),
                );
            }
        }
        assert_eq!(
            gateway.offer_index.lock().unwrap().len(),
            MAXIMUM_INDEXED_OFFERS
        );
        assert_eq!(
            gateway.ingest_offer_announcement(&LogosOfferIngestRequestV1 {
                schema_version: GATEWAY_SCHEMA_VERSION_V1,
                payload_base64: BASE64_STANDARD.encode(&announcement).into(),
            }),
            Err(LogosChatGatewayError::Capacity)
        );
        let index = gateway.offer_index.lock().unwrap();
        assert_eq!(index.len(), MAXIMUM_INDEXED_OFFERS);
        assert!(!index.contains_key(&(maker_identity, offer_id.as_str().into())));
    }

    #[test]
    fn offer_ingest_bounds_and_canonicalizes_base64_before_indexing() {
        let gateway = LogosChatGateway::new_with_clock(LogosChatGatewayRoleV1::Taker, None, || {
            Ok(2_000_000_000)
        })
        .unwrap();
        assert_eq!(
            gateway.ingest_offer_announcement(&LogosOfferIngestRequestV1 {
                schema_version: GATEWAY_SCHEMA_VERSION_V1,
                payload_base64: "A"
                    .repeat(MAXIMUM_LOGOS_OFFER_ANNOUNCEMENT_BASE64_BYTES + 1)
                    .into(),
            }),
            Err(LogosChatGatewayError::InvalidInput)
        );
        assert_eq!(
            gateway.ingest_offer_announcement(&LogosOfferIngestRequestV1 {
                schema_version: GATEWAY_SCHEMA_VERSION_V1,
                payload_base64: "AB==".into(),
            }),
            Err(LogosChatGatewayError::InvalidInput)
        );
        assert!(gateway.offer_index.lock().unwrap().is_empty());
    }

    #[test]
    fn exact_selected_identity_survives_a_chat_address_and_offer_id_collision() {
        let (_victim_dir, victim, _, _, victim_identity, offer_id, now) =
            active_announcements_with_key(77);
        let (_attacker_dir, attacker, _, _, attacker_identity, _, attacker_now) =
            active_announcements_with_key(78);
        assert_ne!(victim_identity, attacker_identity);
        let gateway =
            LogosChatGateway::new_with_clock(LogosChatGatewayRoleV1::Taker, None, move || {
                Ok(now.max(attacker_now))
            })
            .unwrap();
        for announcement in [victim, attacker] {
            gateway
                .ingest_offer_announcement(&LogosOfferIngestRequestV1 {
                    schema_version: GATEWAY_SCHEMA_VERSION_V1,
                    payload_base64: BASE64_STANDARD.encode(announcement).into(),
                })
                .unwrap();
        }
        gateway
            .select_offer_announcement(&LogosOfferSelectRequestV1 {
                schema_version: GATEWAY_SCHEMA_VERSION_V1,
                maker_identity: TakerMakerIdentityV1::new(victim_identity).unwrap(),
                offer_id: offer_id.clone(),
            })
            .unwrap();
        gateway
            .bind_session(&LogosChatGatewayBindRequestV1 {
                schema_version: GATEWAY_SCHEMA_VERSION_V1,
                conversation_id: "collision-resistant-selection".into(),
                local_address: "logos://taker-collision-test".into(),
                peer_address: "logos://maker-recovered".into(),
            })
            .unwrap();
        assert_eq!(
            gateway.selected_offer_key_for_peer("logos://maker-recovered", offer_id.as_str()),
            Some((victim_identity, offer_id.as_str().into()))
        );
    }

    #[tokio::test]
    async fn duplicate_maker_frame_is_replay_while_owner_call_is_inflight() {
        let directory = tempfile::tempdir().unwrap();
        let maker_socket = directory.path().join("maker-chat.sock");
        let _nonresponsive_maker = tokio::net::UnixListener::bind(&maker_socket).unwrap();
        let gateway = Arc::new(
            LogosChatGateway::new(LogosChatGatewayRoleV1::Maker, Some(maker_socket)).unwrap(),
        );
        gateway
            .bind_session(&LogosChatGatewayBindRequestV1 {
                schema_version: GATEWAY_SCHEMA_VERSION_V1,
                conversation_id: "conversation-1".into(),
                local_address: "local://maker".into(),
                peer_address: "local://taker".into(),
            })
            .unwrap();
        let frame = LogosChatFrameV1::new(
            LogosChatGatewayRoleV1::Taker,
            LogosChatMessageV1::Request {
                nonce: 0,
                method: "btc_chat_propose_v2".into(),
                parameter: json!({ "schema_version": 2 }),
            },
        )
        .unwrap();
        let request = LogosChatGatewayIngestRequestV1 {
            schema_version: GATEWAY_SCHEMA_VERSION_V1,
            conversation_id: "conversation-1".into(),
            sender_address: "local://taker".into(),
            content: frame.to_outbox("conversation-1".into()).unwrap().content,
        };

        assert!(!gateway.ingest(&request).unwrap().was_replay);
        assert!(gateway.ingest(&request).unwrap().was_replay);
    }

    #[tokio::test]
    async fn maker_routes_concurrent_taker_failures_to_their_exact_conversations() {
        let directory = tempfile::tempdir().unwrap();
        let gateway = Arc::new(
            LogosChatGateway::new(
                LogosChatGatewayRoleV1::Maker,
                Some(directory.path().join("absent-maker-chat.sock")),
            )
            .unwrap(),
        );
        for marker in ["a", "b"] {
            gateway
                .bind_session(&LogosChatGatewayBindRequestV1 {
                    schema_version: GATEWAY_SCHEMA_VERSION_V1,
                    conversation_id: format!("conversation-{marker}").into(),
                    local_address: "local://maker".into(),
                    peer_address: format!("local://taker-{marker}").into(),
                })
                .unwrap();
            let frame = LogosChatFrameV1::new(
                LogosChatGatewayRoleV1::Taker,
                LogosChatMessageV1::Request {
                    nonce: u64::from(marker.as_bytes()[0]),
                    method: "btc_chat_propose_v2".into(),
                    parameter: json!({
                        "schema_version": 2,
                        "offer_id": "concurrent-offer-001",
                        "request_marker": marker,
                    }),
                },
            )
            .unwrap();
            gateway
                .ingest(&LogosChatGatewayIngestRequestV1 {
                    schema_version: GATEWAY_SCHEMA_VERSION_V1,
                    conversation_id: format!("conversation-{marker}").into(),
                    sender_address: format!("local://taker-{marker}").into(),
                    content: frame
                        .to_outbox(format!("conversation-{marker}").into())
                        .unwrap()
                        .content,
                })
                .unwrap();
        }

        let mut targets = BTreeSet::new();
        for _ in 0..100 {
            if let Some(item) = gateway
                .outbox_peek(LogosChatGatewayOutboxRequestV1 {
                    schema_version: GATEWAY_SCHEMA_VERSION_V1,
                })
                .unwrap()
            {
                targets.insert(item.conversation_id.clone());
                let wrong_conversation = if item.conversation_id.as_ref() == "conversation-a" {
                    "conversation-b"
                } else {
                    "conversation-a"
                };
                assert_eq!(
                    gateway.outbox_ack(&LogosChatGatewayOutboxAckRequestV1 {
                        schema_version: GATEWAY_SCHEMA_VERSION_V1,
                        frame_id: item.frame_id.clone(),
                        conversation_id: wrong_conversation.into(),
                    }),
                    Err(LogosChatGatewayError::SessionConflict)
                );
                gateway
                    .outbox_ack(&LogosChatGatewayOutboxAckRequestV1 {
                        schema_version: GATEWAY_SCHEMA_VERSION_V1,
                        frame_id: item.frame_id,
                        conversation_id: item.conversation_id,
                    })
                    .unwrap();
                if targets.len() == 2 {
                    break;
                }
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(
            targets,
            BTreeSet::from([
                Box::<str>::from("conversation-a"),
                Box::<str>::from("conversation-b"),
            ])
        );
    }
}
