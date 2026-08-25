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
    time::Duration,
};

use jsonrpsee::{RpcModule, core::RegisterMethodError, types::ErrorObjectOwned};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use thiserror::Error;
use tokio::sync::oneshot;

use crate::{call_local_chat_rpc, local_rpc::LocalRpcRemoteError};

const GATEWAY_SCHEMA_VERSION_V1: u16 = 1;
const MAXIMUM_FRAME_BYTES: usize = 1024 * 1024;
const MAXIMUM_QUEUED_FRAMES: usize = 64;
const MAXIMUM_PENDING_REQUESTS: usize = 32;
const MAXIMUM_INFLIGHT_MAKER_REQUESTS: usize = 32;
const MAXIMUM_CACHED_RESPONSES: usize = 128;
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

/// The exact application Chat methods allowed through the transport bridge.
pub const LOGOS_CHAT_GATEWAY_METHODS_V1: [&str; 8] = [
    "btc_chat_propose_v1",
    "btc_chat_propose_v2",
    "btc_chat_complete_v1",
    "btc_chat_complete_v2",
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

    fn to_outbox(&self) -> Result<LogosChatGatewayOutboxItemV1, LogosChatGatewayError> {
        let content =
            serde_json::to_string(self).map_err(|_| LogosChatGatewayError::InvalidInput)?;
        if content.len() > MAXIMUM_FRAME_BYTES {
            return Err(LogosChatGatewayError::InvalidInput);
        }
        Ok(LogosChatGatewayOutboxItemV1 {
            schema_version: GATEWAY_SCHEMA_VERSION_V1,
            frame_id: self.frame_id.clone(),
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
    /// Number of unsent frames.
    pub queued_frames: u16,
    /// Number of Taker calls awaiting a peer response.
    pub pending_requests: u16,
}

/// Idempotent control acknowledgement.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LogosChatGatewayAckV1 {
    /// Fixed response schema.
    pub schema_version: u16,
    /// Whether the exact operation was already observed.
    pub was_replay: bool,
}

#[derive(Debug)]
struct PendingResponse {
    sender: oneshot::Sender<Result<Value, RemoteFailureV1>>,
}

#[derive(Clone, Debug)]
struct CachedResponse {
    request_frame_id: Box<str>,
    outbox: LogosChatGatewayOutboxItemV1,
}

/// In-memory, session-scoped bridge state shared by the control and Taker proxy sockets.
pub struct LogosChatGateway {
    role: LogosChatGatewayRoleV1,
    maker_chat_socket: Option<PathBuf>,
    lifecycle: Mutex<()>,
    session: Mutex<Option<SessionBindingV1>>,
    outbox: Mutex<VecDeque<LogosChatGatewayOutboxItemV1>>,
    pending: Mutex<BTreeMap<Box<str>, PendingResponse>>,
    inflight_maker_requests: Mutex<BTreeSet<Box<str>>>,
    cached_responses: Mutex<VecDeque<CachedResponse>>,
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
            session: Mutex::new(None),
            outbox: Mutex::new(VecDeque::new()),
            pending: Mutex::new(BTreeMap::new()),
            inflight_maker_requests: Mutex::new(BTreeSet::new()),
            cached_responses: Mutex::new(VecDeque::new()),
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
        let mut current = self
            .session
            .lock()
            .map_err(|_| LogosChatGatewayError::DependencyUnavailable)?;
        match current.as_ref() {
            Some(existing) if existing == &binding => Ok(LogosChatGatewayAckV1 {
                schema_version: GATEWAY_SCHEMA_VERSION_V1,
                was_replay: true,
            }),
            Some(_) => Err(LogosChatGatewayError::SessionConflict),
            None => {
                *current = Some(binding);
                Ok(LogosChatGatewayAckV1 {
                    schema_version: GATEWAY_SCHEMA_VERSION_V1,
                    was_replay: false,
                })
            }
        }
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
        if head.frame_id != request.frame_id {
            return Err(LogosChatGatewayError::SessionConflict);
        }
        outbox.pop_front();
        Ok(LogosChatGatewayAckV1 {
            schema_version: GATEWAY_SCHEMA_VERSION_V1,
            was_replay: false,
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
        let session_bound = self
            .session
            .lock()
            .map_err(|_| LogosChatGatewayError::DependencyUnavailable)?
            .is_some();
        Ok(LogosChatGatewayStatusV1 {
            schema_version: GATEWAY_SCHEMA_VERSION_V1,
            role: self.role,
            session_bound,
            queued_frames: u16::try_from(queued_frames)
                .map_err(|_| LogosChatGatewayError::DependencyUnavailable)?,
            pending_requests: u16::try_from(pending_requests)
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
            .session
            .lock()
            .map_err(|_| LogosChatGatewayError::DependencyUnavailable)?
            .take()
            .is_none();
        self.outbox
            .lock()
            .map_err(|_| LogosChatGatewayError::DependencyUnavailable)?
            .clear();
        self.cached_responses
            .lock()
            .map_err(|_| LogosChatGatewayError::DependencyUnavailable)?
            .clear();
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
        let session_bound = self
            .session
            .lock()
            .map_err(|_| remote_failure(LogosChatGatewayError::DependencyUnavailable))?
            .is_some();
        if self.role != LogosChatGatewayRoleV1::Taker || !session_bound {
            return Err(remote_failure(LogosChatGatewayError::SessionUnavailable));
        }
        if !LOGOS_CHAT_GATEWAY_METHODS_V1.contains(&method) || !parameter.is_object() {
            return Err(remote_failure(LogosChatGatewayError::InvalidInput));
        }
        let sequence = self.sequence.fetch_add(1, Ordering::Relaxed);
        let message = LogosChatMessageV1::Request {
            nonce: sequence,
            method: method.into(),
            parameter,
        };
        let frame = LogosChatFrameV1::new(self.role, message).map_err(remote_failure)?;
        let outbox = frame.to_outbox().map_err(remote_failure)?;
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
            pending.insert(frame_id.clone(), PendingResponse { sender });
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
            ) => self.accept_maker_request(frame.frame_id, method, parameter),
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
        let session = self
            .session
            .lock()
            .map_err(|_| LogosChatGatewayError::DependencyUnavailable)?;
        let Some(session) = session.as_ref() else {
            return Err(LogosChatGatewayError::SessionUnavailable);
        };
        if session.conversation_id.as_ref() != conversation_id
            || session.peer_address.as_ref() != sender_address
        {
            return Err(LogosChatGatewayError::SessionConflict);
        }
        Ok(())
    }

    fn accept_maker_request(
        self: &Arc<Self>,
        request_frame_id: Box<str>,
        method: Box<str>,
        parameter: Value,
    ) -> Result<LogosChatGatewayAckV1, LogosChatGatewayError> {
        let mut inflight = self
            .inflight_maker_requests
            .lock()
            .map_err(|_| LogosChatGatewayError::DependencyUnavailable)?;
        if inflight.contains(&request_frame_id) {
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
            .find(|cached| cached.request_frame_id == request_frame_id)
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
        inflight.insert(request_frame_id.clone());
        drop(inflight);

        let gateway = Arc::clone(self);
        let _task = tokio::spawn(async move {
            let _ = gateway
                .process_maker_request(request_frame_id.clone(), &method, &parameter)
                .await;
            if let Ok(mut inflight) = gateway.inflight_maker_requests.lock() {
                inflight.remove(&request_frame_id);
            }
        });
        Ok(LogosChatGatewayAckV1 {
            schema_version: GATEWAY_SCHEMA_VERSION_V1,
            was_replay: false,
        })
    }

    async fn process_maker_request(
        &self,
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
        let outbox = match response_frame.to_outbox() {
            Ok(outbox) => outbox,
            Err(LogosChatGatewayError::InvalidInput) => LogosChatFrameV1::new(
                self.role,
                LogosChatMessageV1::Response {
                    request_frame_id: request_frame_id.clone(),
                    result: None,
                    error: Some(remote_failure(LogosChatGatewayError::DependencyUnavailable)),
                },
            )?
            .to_outbox()?,
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

    fn enqueue(&self, frame: LogosChatGatewayOutboxItemV1) -> Result<(), LogosChatGatewayError> {
        let mut outbox = self
            .outbox
            .lock()
            .map_err(|_| LogosChatGatewayError::DependencyUnavailable)?;
        if outbox.len() >= MAXIMUM_QUEUED_FRAMES {
            return Err(LogosChatGatewayError::Capacity);
        }
        if !outbox
            .iter()
            .any(|queued| queued.frame_id == frame.frame_id)
        {
            outbox.push_back(frame);
        }
        Ok(())
    }
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
    use std::sync::Arc;

    use serde_json::{Value, json};

    use crate::local_rpc::LocalRpcRemoteError;

    use super::{
        GATEWAY_SCHEMA_VERSION_V1, LogosChatFrameV1, LogosChatGateway,
        LogosChatGatewayBindRequestV1, LogosChatGatewayError, LogosChatGatewayIngestRequestV1,
        LogosChatGatewayResetRequestV1, LogosChatGatewayRoleV1, LogosChatMessageV1,
        maker_remote_failure,
    };

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
        let outbox = frame.to_outbox().unwrap();
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
            content: frame.to_outbox().unwrap().content,
        };

        assert!(!gateway.ingest(&request).unwrap().was_replay);
        assert!(gateway.ingest(&request).unwrap().was_replay);
    }
}
