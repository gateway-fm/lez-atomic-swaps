use std::{
    collections::BTreeMap,
    fmt,
    fs::{self, File},
    future::Future,
    io::{self, Write as _},
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::Arc,
};

use jsonrpsee::{RpcModule, server::ServerBuilder, types::ErrorObjectOwned};
use lez_bridge_protocol::{
    CompleteWitnessedClaimRequest, CompleteWitnessedClaimResult, DescribeRuntimeRequest,
    DescribeRuntimeResult, ErrorCode, ErrorMessage, MAX_RPC_BODY_BYTES,
    METHOD_COMPLETE_WITNESSED_CLAIM, METHOD_DESCRIBE_RUNTIME, METHOD_OBSERVE_ESCROW,
    METHOD_OBSERVE_FINALIZED_WITNESSED_CLAIM, METHOD_OBSERVE_FINALIZED_WITNESSED_FUNDING,
    METHOD_OBSERVE_NATIVE_REFUND, METHOD_OBSERVE_REVEALING_CLAIM, METHOD_OBSERVE_WITNESSED_ESCROW,
    METHOD_PREPARE_NATIVE_ESCROW, METHOD_PREPARE_NATIVE_REFUND, METHOD_PREPARE_REVEALING_CLAIM,
    METHOD_PREPARE_WITNESSED_CLAIM, METHOD_PREPARE_WITNESSED_ESCROW, METHOD_SUBMIT_TRANSACTION,
    MessageContext, ObserveEscrowRequest, ObserveFinalizedWitnessedClaimRequest,
    ObserveFinalizedWitnessedFundingRequest, ObserveNativeRefundRequest,
    ObserveRevealingClaimRequest, ObserveWitnessedEscrowRequest, Participant,
    PrepareNativeEscrowRequest, PrepareNativeEscrowResult, PrepareNativeRefundRequest,
    PrepareRevealingClaimRequest, PrepareRevealingClaimResult, PrepareWitnessedClaimRequest,
    PrepareWitnessedClaimResult, PrepareWitnessedEscrowRequest, PrepareWitnessedEscrowResult,
    ProtocolErrorReply, RUN_ID_HEADER, RunId, SIDECAR_ROLE_HEADER, SubmitTransactionRequest,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest as _, Sha256};
use tokio::sync::Mutex;
use tower::ServiceBuilder;
use tower_http::validate_request::ValidateRequestHeaderLayer;
use zeroize::Zeroize as _;

use crate::{BridgeRuntime, BridgeRuntimeError};

const MIN_CAPABILITY_BYTES: usize = 32;
const MAX_CAPABILITY_BYTES: usize = 128;
const STORE_SCHEMA_VERSION: u16 = 1;
const MAX_STORE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_STORE_ENTRIES: usize = 4_096;
const PROTOCOL_ERROR_NUMBER: i32 = -32_010;
const PROTOCOL_ERROR_SUMMARY: &str = "LEZ bridge request failed";
const MAX_REQUEST_ERROR_EVENT_BYTES: usize = 256;

/// A bounded bearer capability dedicated to one local v0.2 sidecar.
pub struct BridgeServerCapability(String);

impl BridgeServerCapability {
    /// Creates a capability in the bridge client's safe HTTP-header grammar.
    ///
    /// # Errors
    ///
    /// Rejects values outside 32..=128 bytes or the bounded header grammar.
    pub fn new(value: impl Into<String>) -> Result<Self, BridgeServerCapabilityError> {
        let mut value = value.into();
        if (MIN_CAPABILITY_BYTES..=MAX_CAPABILITY_BYTES).contains(&value.len())
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        {
            Ok(Self(value))
        } else {
            value.zeroize();
            Err(BridgeServerCapabilityError)
        }
    }
}

impl Drop for BridgeServerCapability {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

impl fmt::Debug for BridgeServerCapability {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("BridgeServerCapability([REDACTED])")
    }
}

/// A server capability failed its bounded grammar.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[error("sidecar capability must be 32..=128 ASCII characters from [A-Za-z0-9._-]")]
pub struct BridgeServerCapabilityError;

/// Immutable composed-run, authentication, and durable-cache configuration.
pub struct BridgeServerConfig {
    run_id: RunId,
    capability: BridgeServerCapability,
    idempotency_path: PathBuf,
    listen_address: SocketAddr,
}

impl BridgeServerConfig {
    /// Binds one run and one owner-only durable actor state file.
    /// A zero loopback port requests an OS-assigned collision-free port.
    #[must_use]
    pub const fn new(
        run_id: RunId,
        capability: BridgeServerCapability,
        idempotency_path: PathBuf,
        listen_address: SocketAddr,
    ) -> Self {
        Self {
            run_id,
            capability,
            idempotency_path,
            listen_address,
        }
    }
}

impl fmt::Debug for BridgeServerConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("BridgeServerConfig")
            .field("run_id", &self.run_id)
            .field("capability", &self.capability)
            .field("idempotency_path", &"[REDACTED]")
            .field("listen_address", &self.listen_address)
            .finish()
    }
}

/// Durable state, listener, runtime-health, or shutdown failure.
#[derive(Debug, thiserror::Error)]
pub enum BridgeServerError {
    /// Durable request state could not be safely read or replaced.
    #[error("durable bridge request state is unavailable")]
    DurableState(#[source] io::Error),
    /// Existing state is malformed or belongs to another run/runtime.
    #[error("durable bridge request state does not match this sidecar")]
    InvalidDurableState,
    /// The official runtime health gate failed before binding.
    #[error("official runtime health gate failed")]
    Runtime,
    /// The authenticated loopback listener could not be constructed.
    #[error("authenticated loopback bridge listener is unavailable")]
    Bind,
    /// The running listener could not be stopped cleanly.
    #[error("bridge server could not be stopped")]
    Stop,
}

/// Running authenticated loopback bridge server.
pub struct BridgeServerHandle {
    endpoint: String,
    handle: jsonrpsee::server::ServerHandle,
}

impl fmt::Debug for BridgeServerHandle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("BridgeServerHandle")
            .field("endpoint", &self.endpoint)
            .finish_non_exhaustive()
    }
}

impl BridgeServerHandle {
    /// Returns the literal-loopback ephemeral HTTP endpoint.
    #[must_use]
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// Stops the listener and waits for all server tasks.
    ///
    /// # Errors
    ///
    /// Returns an error if the listener was already stopped elsewhere.
    pub async fn stop(self) -> Result<(), BridgeServerError> {
        self.handle.stop().map_err(|_| BridgeServerError::Stop)?;
        self.handle.stopped().await;
        Ok(())
    }
}

#[derive(Clone)]
struct ServerState {
    run_id: RunId,
    runtime: Arc<BridgeRuntime>,
    store: Arc<Mutex<DurableStore>>,
}

impl fmt::Debug for ServerState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ServerState")
            .field("run_id", &self.run_id)
            .field("runtime", &self.runtime)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct PersistedStore {
    schema_version: u16,
    run_id: RunId,
    runtime: lez_bridge_protocol::RuntimeDescriptor,
    entries: BTreeMap<String, PersistedEntry>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct PersistedEntry {
    method: String,
    request_sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    replay_request: Option<Value>,
    outcome: PersistedOutcome,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
enum PersistedOutcome {
    Success(Value),
    Error(ProtocolErrorReply),
    SubmissionInFlight(ProtocolErrorReply),
    Repeatable,
}

#[derive(Debug)]
struct DurableStore {
    path: PathBuf,
    persisted: PersistedStore,
}

type RestoredRequests = (
    Option<(PrepareNativeEscrowRequest, PrepareNativeEscrowResult)>,
    Option<(PrepareWitnessedEscrowRequest, PrepareWitnessedEscrowResult)>,
    Option<(PrepareRevealingClaimRequest, PrepareRevealingClaimResult)>,
    Option<(PrepareWitnessedClaimRequest, PrepareWitnessedClaimResult)>,
    Option<(CompleteWitnessedClaimRequest, CompleteWitnessedClaimResult)>,
);

impl DurableStore {
    fn open(
        path: PathBuf,
        run_id: &RunId,
        runtime: &lez_bridge_protocol::RuntimeDescriptor,
    ) -> Result<Self, BridgeServerError> {
        let persisted = if path.exists() {
            let link = fs::symlink_metadata(&path).map_err(BridgeServerError::DurableState)?;
            if link.file_type().is_symlink() {
                return Err(BridgeServerError::InvalidDurableState);
            }
            let metadata = fs::metadata(&path).map_err(BridgeServerError::DurableState)?;
            if !metadata.is_file() || metadata.len() == 0 || metadata.len() > MAX_STORE_BYTES {
                return Err(BridgeServerError::InvalidDurableState);
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                if metadata.permissions().mode() & 0o077 != 0 {
                    return Err(BridgeServerError::InvalidDurableState);
                }
            }
            serde_json::from_slice(&fs::read(&path).map_err(BridgeServerError::DurableState)?)
                .map_err(|_| BridgeServerError::InvalidDurableState)?
        } else {
            PersistedStore {
                schema_version: STORE_SCHEMA_VERSION,
                run_id: run_id.clone(),
                runtime: runtime.clone(),
                entries: BTreeMap::new(),
            }
        };
        if persisted.schema_version != STORE_SCHEMA_VERSION
            || &persisted.run_id != run_id
            || &persisted.runtime != runtime
            || persisted.entries.len() > MAX_STORE_ENTRIES
            || !persisted.entries.iter().all(|(request_id, entry)| {
                lez_bridge_protocol::RequestId::new(request_id).is_ok()
                    && valid_method(&entry.method)
                    && entry.request_sha256.len() == 64
                    && entry
                        .request_sha256
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
                    && entry.replay_request.as_ref().is_none_or(|_| {
                        matches!(
                            entry.method.as_str(),
                            METHOD_PREPARE_NATIVE_ESCROW
                                | METHOD_PREPARE_WITNESSED_ESCROW
                                | METHOD_PREPARE_REVEALING_CLAIM
                                | METHOD_PREPARE_WITNESSED_CLAIM
                                | METHOD_COMPLETE_WITNESSED_CLAIM
                        )
                    })
            })
        {
            return Err(BridgeServerError::InvalidDurableState);
        }
        let store = Self { path, persisted };
        if !store.path.exists() {
            store.persist()?;
        }
        Ok(store)
    }

    fn persist(&self) -> Result<(), BridgeServerError> {
        let parent = self
            .path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        if !parent.is_dir() {
            return Err(BridgeServerError::InvalidDurableState);
        }
        let mut temporary =
            tempfile::NamedTempFile::new_in(parent).map_err(BridgeServerError::DurableState)?;
        serde_json::to_writer(&mut temporary, &self.persisted)
            .map_err(io::Error::other)
            .map_err(BridgeServerError::DurableState)?;
        temporary.flush().map_err(BridgeServerError::DurableState)?;
        temporary
            .as_file()
            .sync_all()
            .map_err(BridgeServerError::DurableState)?;
        if temporary
            .as_file()
            .metadata()
            .map_err(BridgeServerError::DurableState)?
            .len()
            > MAX_STORE_BYTES
        {
            return Err(BridgeServerError::InvalidDurableState);
        }
        temporary
            .persist(&self.path)
            .map_err(|error| BridgeServerError::DurableState(error.error))?;
        File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(BridgeServerError::DurableState)
    }

    fn replay(
        &self,
        request_id: &str,
        method: &str,
        request_sha256: &str,
    ) -> Result<Option<PersistedOutcome>, OperationFailure> {
        let Some(entry) = self.persisted.entries.get(request_id) else {
            return Ok(None);
        };
        if entry.method != method || entry.request_sha256 != request_sha256 {
            return Err(OperationFailure::invalid_request(
                "request id was already used for a different request",
            ));
        }
        if matches!(entry.outcome, PersistedOutcome::Repeatable) {
            Ok(None)
        } else {
            Ok(Some(entry.outcome.clone()))
        }
    }

    fn reserve_submission(
        &mut self,
        context: &MessageContext,
        request_sha256: &str,
    ) -> Result<(), OperationFailure> {
        self.ensure_capacity()?;
        self.persisted.entries.insert(
            context.request_id.as_str().to_owned(),
            PersistedEntry {
                method: METHOD_SUBMIT_TRANSACTION.to_owned(),
                request_sha256: request_sha256.to_owned(),
                replay_request: None,
                outcome: PersistedOutcome::SubmissionInFlight(protocol_reply(
                    context,
                    OperationFailure {
                        code: ErrorCode::UnknownSubmissionOutcome,
                        message: "official node submission outcome is unknown",
                    },
                )),
            },
        );
        self.persist().map_err(|_| OperationFailure::internal())
    }

    fn finish(
        &mut self,
        method: &str,
        context: &MessageContext,
        request_sha256: String,
        replay_request: Option<Value>,
        outcome: PersistedOutcome,
    ) -> Result<(), OperationFailure> {
        if !self
            .persisted
            .entries
            .contains_key(context.request_id.as_str())
        {
            self.ensure_capacity()?;
        }
        self.persisted.entries.insert(
            context.request_id.as_str().to_owned(),
            PersistedEntry {
                method: method.to_owned(),
                request_sha256,
                replay_request,
                outcome,
            },
        );
        self.persist().map_err(|_| OperationFailure::internal())
    }

    fn ensure_capacity(&self) -> Result<(), OperationFailure> {
        if self.persisted.entries.len() >= MAX_STORE_ENTRIES {
            Err(OperationFailure::internal())
        } else {
            Ok(())
        }
    }

    fn restore_requests(&self) -> Result<RestoredRequests, BridgeServerError> {
        let mut prepare = None;
        let mut witnessed_escrow = None;
        let mut claim = None;
        let mut witnessed = None;
        let mut completed_witnessed = None;
        for entry in self.persisted.entries.values() {
            let PersistedOutcome::Success(value) = &entry.outcome else {
                continue;
            };
            let Some(request) = entry.replay_request.clone() else {
                continue;
            };
            if hex::encode(Sha256::digest(
                serde_json::to_vec(&request).map_err(|_| BridgeServerError::InvalidDurableState)?,
            )) != entry.request_sha256
            {
                return Err(BridgeServerError::InvalidDurableState);
            }
            match entry.method.as_str() {
                METHOD_PREPARE_NATIVE_ESCROW if prepare.is_none() => {
                    prepare = Some((
                        serde_json::from_value(request)
                            .map_err(|_| BridgeServerError::InvalidDurableState)?,
                        serde_json::from_value(value.clone())
                            .map_err(|_| BridgeServerError::InvalidDurableState)?,
                    ));
                }
                METHOD_PREPARE_WITNESSED_ESCROW if witnessed_escrow.is_none() => {
                    witnessed_escrow = Some((
                        serde_json::from_value(request)
                            .map_err(|_| BridgeServerError::InvalidDurableState)?,
                        serde_json::from_value(value.clone())
                            .map_err(|_| BridgeServerError::InvalidDurableState)?,
                    ));
                }
                METHOD_PREPARE_REVEALING_CLAIM if claim.is_none() => {
                    claim = Some((
                        serde_json::from_value(request)
                            .map_err(|_| BridgeServerError::InvalidDurableState)?,
                        serde_json::from_value(value.clone())
                            .map_err(|_| BridgeServerError::InvalidDurableState)?,
                    ));
                }
                METHOD_PREPARE_WITNESSED_CLAIM if witnessed.is_none() => {
                    witnessed = Some((
                        serde_json::from_value(request)
                            .map_err(|_| BridgeServerError::InvalidDurableState)?,
                        serde_json::from_value(value.clone())
                            .map_err(|_| BridgeServerError::InvalidDurableState)?,
                    ));
                }
                METHOD_COMPLETE_WITNESSED_CLAIM if completed_witnessed.is_none() => {
                    completed_witnessed = Some((
                        serde_json::from_value(request)
                            .map_err(|_| BridgeServerError::InvalidDurableState)?,
                        serde_json::from_value(value.clone())
                            .map_err(|_| BridgeServerError::InvalidDurableState)?,
                    ));
                }
                METHOD_PREPARE_NATIVE_ESCROW
                | METHOD_PREPARE_WITNESSED_ESCROW
                | METHOD_PREPARE_REVEALING_CLAIM
                | METHOD_PREPARE_WITNESSED_CLAIM
                | METHOD_COMPLETE_WITNESSED_CLAIM => {
                    return Err(BridgeServerError::InvalidDurableState);
                }
                _ => {}
            }
        }
        Ok((
            prepare,
            witnessed_escrow,
            claim,
            witnessed,
            completed_witnessed,
        ))
    }
}

#[derive(Clone, Copy, Debug)]
struct OperationFailure {
    code: ErrorCode,
    message: &'static str,
}

#[derive(Serialize)]
struct RequestErrorEvent<'a> {
    event: &'static str,
    method: &'a str,
    sidecar_role: Participant,
    error_code: ErrorCode,
}

impl OperationFailure {
    const fn invalid_request(message: &'static str) -> Self {
        Self {
            code: ErrorCode::InvalidRequest,
            message,
        }
    }

    const fn internal() -> Self {
        Self {
            code: ErrorCode::Internal,
            message: "durable bridge request outcome is unavailable",
        }
    }
}

impl From<BridgeRuntimeError> for OperationFailure {
    fn from(value: BridgeRuntimeError) -> Self {
        match value {
            BridgeRuntimeError::Planner | BridgeRuntimeError::InvalidObservation => Self {
                code: ErrorCode::InvalidTransaction,
                message: "official v0.2 bridge validation failed",
            },
            BridgeRuntimeError::Unavailable | BridgeRuntimeError::RefundUnavailable => Self {
                code: ErrorCode::Unavailable,
                message: "required official v0.2 operation is unavailable",
            },
            BridgeRuntimeError::MovingTip => Self {
                code: ErrorCode::MovingTip,
                message: "official v0.2 tip moved during observation",
            },
            BridgeRuntimeError::AmbiguousDiscovery => Self {
                code: ErrorCode::AmbiguousDiscovery,
                message: "official v0.2 discovery matched more than once",
            },
            BridgeRuntimeError::ConflictingDiscovery => Self {
                code: ErrorCode::ConflictingDiscovery,
                message: "official v0.2 discovery conflicted with the signed transcript",
            },
            BridgeRuntimeError::UnknownSubmissionOutcome => Self {
                code: ErrorCode::UnknownSubmissionOutcome,
                message: "official node submission outcome is unknown",
            },
        }
    }
}

/// Starts one health-gated, authenticated, durable, single-flight sidecar.
///
/// # Errors
///
/// Fails before readiness on runtime health, durable-state, authentication,
/// explicit-loopback bind, method registration, or restoration failure.
pub async fn start_bridge_server(
    mut config: BridgeServerConfig,
    runtime: Arc<BridgeRuntime>,
) -> Result<BridgeServerHandle, BridgeServerError> {
    runtime
        .verify_health()
        .await
        .map_err(|_| BridgeServerError::Runtime)?;
    if !config.listen_address.ip().is_loopback() {
        return Err(BridgeServerError::Bind);
    }
    let store = DurableStore::open(
        config.idempotency_path,
        &config.run_id,
        runtime.descriptor(),
    )?;
    restore_runtime_requests(&runtime, store.restore_requests()?).await?;

    let role = match runtime.descriptor().sidecar_role {
        Participant::Maker => "maker",
        Participant::Taker => "taker",
    };
    let mut authorization = String::with_capacity("Bearer ".len() + config.capability.0.len());
    authorization.push_str("Bearer ");
    authorization.push_str(&config.capability.0);
    let middleware = ServiceBuilder::new()
        .layer(
            ValidateRequestHeaderLayer::has_header_value("authorization", &authorization)
                .map_err(|_| BridgeServerError::Bind)?,
        )
        .layer(
            ValidateRequestHeaderLayer::has_header_value(RUN_ID_HEADER, config.run_id.as_str())
                .map_err(|_| BridgeServerError::Bind)?,
        )
        .layer(
            ValidateRequestHeaderLayer::has_header_value(SIDECAR_ROLE_HEADER, role)
                .map_err(|_| BridgeServerError::Bind)?,
        );
    authorization.zeroize();
    config.capability.0.zeroize();

    let server_config = jsonrpsee::server::ServerConfig::builder()
        .max_request_body_size(MAX_RPC_BODY_BYTES)
        .max_response_body_size(MAX_RPC_BODY_BYTES)
        .max_connections(1)
        .build();
    let server = ServerBuilder::with_config(server_config)
        .set_http_middleware(middleware)
        .build(config.listen_address)
        .await
        .map_err(|_| BridgeServerError::Bind)?;
    let address = server.local_addr().map_err(|_| BridgeServerError::Bind)?;
    if !address.ip().is_loopback() || address.port() == 0 {
        return Err(BridgeServerError::Bind);
    }
    let state = ServerState {
        run_id: config.run_id,
        runtime,
        store: Arc::new(Mutex::new(store)),
    };
    let mut module = RpcModule::new(state);
    register_methods(&mut module).map_err(|_| BridgeServerError::Bind)?;
    Ok(BridgeServerHandle {
        endpoint: format!("http://{address}"),
        handle: server.start(module),
    })
}

async fn restore_runtime_requests(
    runtime: &Arc<BridgeRuntime>,
    restored: RestoredRequests,
) -> Result<(), BridgeServerError> {
    let (
        restored_prepare,
        restored_witnessed_escrow,
        restored_claim,
        restored_witnessed,
        restored_completion,
    ) = restored;
    if let Some((request, expected)) = restored_prepare {
        let observed = runtime
            .prepare_native_escrow(request)
            .await
            .map_err(|_| BridgeServerError::InvalidDurableState)?;
        if observed != expected {
            return Err(BridgeServerError::InvalidDurableState);
        }
    }
    if let Some((request, expected)) = restored_witnessed_escrow {
        let observed = runtime
            .prepare_witnessed_escrow(&request)
            .await
            .map_err(|_| BridgeServerError::InvalidDurableState)?;
        if observed != expected {
            return Err(BridgeServerError::InvalidDurableState);
        }
    }
    if let Some((request, expected)) = restored_claim {
        let observed = runtime
            .prepare_revealing_claim(&request)
            .await
            .map_err(|_| BridgeServerError::InvalidDurableState)?;
        if observed != expected {
            return Err(BridgeServerError::InvalidDurableState);
        }
    }
    if let Some((request, expected)) = restored_witnessed {
        let observed = runtime
            .prepare_witnessed_claim(&request)
            .await
            .map_err(|_| BridgeServerError::InvalidDurableState)?;
        if observed != expected {
            return Err(BridgeServerError::InvalidDurableState);
        }
    }
    if let Some((request, expected)) = restored_completion {
        let observed = runtime
            .complete_witnessed_claim(&request)
            .await
            .map_err(|_| BridgeServerError::InvalidDurableState)?;
        if observed != expected {
            return Err(BridgeServerError::InvalidDurableState);
        }
    }
    Ok(())
}

#[allow(
    clippy::too_many_lines,
    reason = "all protocol methods remain visibly registered at one authenticated boundary"
)]
fn register_methods(
    module: &mut RpcModule<ServerState>,
) -> Result<(), jsonrpsee::core::RegisterMethodError> {
    module.register_async_method(METHOD_DESCRIBE_RUNTIME, |params, state, _| async move {
        let request: DescribeRuntimeRequest = params.one()?;
        let context = request.context.clone();
        let runtime = state.runtime.descriptor().clone();
        state
            .execute(
                METHOD_DESCRIBE_RUNTIME,
                &request.context,
                &request,
                || async { to_value(DescribeRuntimeResult::new(context, runtime)) },
            )
            .await
    })?;
    module.register_async_method(
        METHOD_PREPARE_NATIVE_ESCROW,
        |params, state, _| async move {
            let request: PrepareNativeEscrowRequest = params.one()?;
            state.validate_runtime(&request.context, &request.runtime)?;
            let operation = request.clone();
            let runtime = Arc::clone(&state.runtime);
            state
                .execute(
                    METHOD_PREPARE_NATIVE_ESCROW,
                    &request.context,
                    &request,
                    || async move {
                        runtime
                            .prepare_native_escrow(operation)
                            .await
                            .map_err(Into::into)
                            .and_then(to_value)
                    },
                )
                .await
        },
    )?;
    module.register_async_method(
        METHOD_PREPARE_WITNESSED_ESCROW,
        |params, state, _| async move {
            let request = Arc::new(params.one::<PrepareWitnessedEscrowRequest>()?);
            state.validate_runtime(&request.context, &request.runtime)?;
            let operation = Arc::clone(&request);
            let runtime = Arc::clone(&state.runtime);
            state
                .execute(
                    METHOD_PREPARE_WITNESSED_ESCROW,
                    &request.context,
                    request.as_ref(),
                    || async move {
                        runtime
                            .prepare_witnessed_escrow(operation.as_ref())
                            .await
                            .map_err(Into::into)
                            .and_then(to_value)
                    },
                )
                .await
        },
    )?;
    module.register_async_method(METHOD_OBSERVE_ESCROW, |params, state, _| async move {
        let request: ObserveEscrowRequest = params.one()?;
        state.validate_runtime(&request.context, &request.runtime)?;
        let operation = request.clone();
        let runtime = Arc::clone(&state.runtime);
        state
            .execute(
                METHOD_OBSERVE_ESCROW,
                &request.context,
                &request,
                || async move {
                    runtime
                        .observe_escrow(&operation)
                        .await
                        .map_err(Into::into)
                        .and_then(to_value)
                },
            )
            .await
    })?;
    module.register_async_method(
        METHOD_OBSERVE_WITNESSED_ESCROW,
        |params, state, _| async move {
            let request: ObserveWitnessedEscrowRequest = params.one()?;
            state.validate_runtime(&request.context, &request.runtime)?;
            let operation = request.clone();
            let runtime = Arc::clone(&state.runtime);
            state
                .execute(
                    METHOD_OBSERVE_WITNESSED_ESCROW,
                    &request.context,
                    &request,
                    || async move {
                        runtime
                            .observe_witnessed_escrow(&operation)
                            .await
                            .map_err(Into::into)
                            .and_then(to_value)
                    },
                )
                .await
        },
    )?;
    module.register_async_method(
        METHOD_PREPARE_REVEALING_CLAIM,
        |params, state, _| async move {
            let request = Arc::new(params.one::<PrepareRevealingClaimRequest>()?);
            state.validate_runtime(&request.context, &request.runtime)?;
            let operation = Arc::clone(&request);
            let runtime = Arc::clone(&state.runtime);
            state
                .execute(
                    METHOD_PREPARE_REVEALING_CLAIM,
                    &request.context,
                    request.as_ref(),
                    || async move {
                        runtime
                            .prepare_revealing_claim(operation.as_ref())
                            .await
                            .map_err(Into::into)
                            .and_then(to_value)
                    },
                )
                .await
        },
    )?;
    module.register_async_method(
        METHOD_PREPARE_WITNESSED_CLAIM,
        |params, state, _| async move {
            let request = Arc::new(params.one::<PrepareWitnessedClaimRequest>()?);
            state.validate_runtime(&request.context, &request.runtime)?;
            let operation = Arc::clone(&request);
            let runtime = Arc::clone(&state.runtime);
            state
                .execute(
                    METHOD_PREPARE_WITNESSED_CLAIM,
                    &request.context,
                    request.as_ref(),
                    || async move {
                        runtime
                            .prepare_witnessed_claim(operation.as_ref())
                            .await
                            .map_err(Into::into)
                            .and_then(to_value)
                    },
                )
                .await
        },
    )?;
    module.register_async_method(
        METHOD_COMPLETE_WITNESSED_CLAIM,
        |params, state, _| async move {
            let request = Arc::new(params.one::<CompleteWitnessedClaimRequest>()?);
            state.validate_runtime(&request.context, &request.runtime)?;
            let operation = Arc::clone(&request);
            let runtime = Arc::clone(&state.runtime);
            state
                .execute(
                    METHOD_COMPLETE_WITNESSED_CLAIM,
                    &request.context,
                    request.as_ref(),
                    || async move {
                        runtime
                            .complete_witnessed_claim(operation.as_ref())
                            .await
                            .map_err(Into::into)
                            .and_then(to_value)
                    },
                )
                .await
        },
    )?;
    module.register_async_method(
        METHOD_OBSERVE_FINALIZED_WITNESSED_CLAIM,
        |params, state, _| async move {
            let request: ObserveFinalizedWitnessedClaimRequest = params.one()?;
            state.validate_runtime(&request.context, &request.runtime)?;
            let operation = request.clone();
            let runtime = Arc::clone(&state.runtime);
            state
                .execute(
                    METHOD_OBSERVE_FINALIZED_WITNESSED_CLAIM,
                    &request.context,
                    &request,
                    || async move {
                        runtime
                            .observe_finalized_witnessed_claim(&operation)
                            .await
                            .map_err(Into::into)
                            .and_then(to_value)
                    },
                )
                .await
        },
    )?;
    module.register_async_method(
        METHOD_OBSERVE_FINALIZED_WITNESSED_FUNDING,
        |params, state, _| async move {
            let request: ObserveFinalizedWitnessedFundingRequest = params.one()?;
            state.validate_runtime(&request.context, &request.runtime)?;
            let operation = request.clone();
            let runtime = Arc::clone(&state.runtime);
            state
                .execute(
                    METHOD_OBSERVE_FINALIZED_WITNESSED_FUNDING,
                    &request.context,
                    &request,
                    || async move {
                        runtime
                            .observe_finalized_witnessed_funding(&operation)
                            .await
                            .map_err(Into::into)
                            .and_then(to_value)
                    },
                )
                .await
        },
    )?;
    module.register_async_method(
        METHOD_OBSERVE_REVEALING_CLAIM,
        |params, state, _| async move {
            let request: ObserveRevealingClaimRequest = params.one()?;
            state.validate_runtime(&request.context, &request.runtime)?;
            let operation = request.clone();
            let runtime = Arc::clone(&state.runtime);
            state
                .execute(
                    METHOD_OBSERVE_REVEALING_CLAIM,
                    &request.context,
                    &request,
                    || async move {
                        runtime
                            .observe_revealing_claim(&operation)
                            .await
                            .map_err(Into::into)
                            .and_then(to_value)
                    },
                )
                .await
        },
    )?;
    module.register_async_method(METHOD_SUBMIT_TRANSACTION, |params, state, _| async move {
        let request: SubmitTransactionRequest = params.one()?;
        state.validate_runtime(&request.context, &request.runtime)?;
        let operation = request.clone();
        let runtime = Arc::clone(&state.runtime);
        state
            .execute(
                METHOD_SUBMIT_TRANSACTION,
                &request.context,
                &request,
                || async move {
                    runtime
                        .submit_transaction(&operation)
                        .await
                        .map_err(Into::into)
                        .and_then(to_value)
                },
            )
            .await
    })?;
    register_refund_stubs(module)
}

fn register_refund_stubs(
    module: &mut RpcModule<ServerState>,
) -> Result<(), jsonrpsee::core::RegisterMethodError> {
    module.register_async_method(
        METHOD_PREPARE_NATIVE_REFUND,
        |params, state, _| async move {
            let request: PrepareNativeRefundRequest = params.one()?;
            state.validate_runtime(&request.context, &request.runtime)?;
            state
                .execute(
                    METHOD_PREPARE_NATIVE_REFUND,
                    &request.context,
                    &request,
                    || async {
                        Err(OperationFailure::from(
                            BridgeRuntimeError::RefundUnavailable,
                        ))
                    },
                )
                .await
        },
    )?;
    module.register_async_method(
        METHOD_OBSERVE_NATIVE_REFUND,
        |params, state, _| async move {
            let request: ObserveNativeRefundRequest = params.one()?;
            state.validate_runtime(&request.context, &request.runtime)?;
            state
                .execute(
                    METHOD_OBSERVE_NATIVE_REFUND,
                    &request.context,
                    &request,
                    || async {
                        Err(OperationFailure::from(
                            BridgeRuntimeError::RefundUnavailable,
                        ))
                    },
                )
                .await
        },
    )?;
    Ok(())
}

impl ServerState {
    fn validate_context(&self, context: &MessageContext) -> Result<(), ErrorObjectOwned> {
        if context.run_id != self.run_id {
            return Err(protocol_error(
                context,
                OperationFailure::invalid_request("request targets the wrong composed run"),
            ));
        }
        if context.sidecar_role != self.runtime.descriptor().sidecar_role {
            return Err(protocol_error(
                context,
                OperationFailure {
                    code: ErrorCode::WrongSidecarRole,
                    message: "request targets the wrong sidecar role",
                },
            ));
        }
        Ok(())
    }

    fn validate_runtime(
        &self,
        context: &MessageContext,
        runtime: &lez_bridge_protocol::RuntimeDescriptor,
    ) -> Result<(), ErrorObjectOwned> {
        self.validate_context(context)?;
        if runtime != self.runtime.descriptor() {
            return Err(protocol_error(
                context,
                OperationFailure::invalid_request("request targets the wrong runtime identity"),
            ));
        }
        Ok(())
    }

    async fn execute<Request, Operation, Fut>(
        &self,
        method: &'static str,
        context: &MessageContext,
        request: &Request,
        operation: Operation,
    ) -> Result<Value, ErrorObjectOwned>
    where
        Request: Serialize,
        Operation: FnOnce() -> Fut,
        Fut: Future<Output = Result<Value, OperationFailure>>,
    {
        self.validate_context(context)?;
        let (request_sha256, replay_request) =
            encode_request(method, request).map_err(|failure| protocol_error(context, failure))?;
        {
            let mut store = self.store.lock().await;
            if let Some(outcome) = store
                .replay(context.request_id.as_str(), method, &request_sha256)
                .map_err(|failure| protocol_error(context, failure))?
            {
                return outcome.into_rpc_result();
            }
            if method == METHOD_SUBMIT_TRANSACTION {
                store
                    .reserve_submission(context, &request_sha256)
                    .map_err(|failure| protocol_error(context, failure))?;
            }
        }
        let outcome = match operation().await {
            Ok(value) => PersistedOutcome::Success(value),
            Err(failure) => {
                report_request_error(method, self.runtime.descriptor().sidecar_role, failure.code);
                PersistedOutcome::Error(protocol_reply(context, failure))
            }
        };
        let prepare = matches!(
            method,
            METHOD_PREPARE_NATIVE_ESCROW
                | METHOD_PREPARE_REVEALING_CLAIM
                | METHOD_PREPARE_WITNESSED_CLAIM
                | METHOD_COMPLETE_WITNESSED_CLAIM
        );
        let repeatable = method != METHOD_SUBMIT_TRANSACTION
            && (!prepare || matches!(&outcome, PersistedOutcome::Error(_)));
        self.store
            .lock()
            .await
            .finish(
                method,
                context,
                request_sha256,
                replay_request,
                if repeatable {
                    PersistedOutcome::Repeatable
                } else {
                    outcome.clone()
                },
            )
            .map_err(|failure| protocol_error(context, failure))?;
        outcome.into_rpc_result()
    }
}

impl PersistedOutcome {
    fn into_rpc_result(self) -> Result<Value, ErrorObjectOwned> {
        match self {
            Self::Success(value) => Ok(value),
            Self::Error(reply) | Self::SubmissionInFlight(reply) => Err(error_object(reply)),
            Self::Repeatable => unreachable!("repeatable request outcomes are never replayed"),
        }
    }
}

fn to_value<T: Serialize>(value: T) -> Result<Value, OperationFailure> {
    serde_json::to_value(value).map_err(|_| OperationFailure::internal())
}

fn report_request_error(method: &'static str, sidecar_role: Participant, error_code: ErrorCode) {
    let event = RequestErrorEvent {
        event: "request_error",
        method,
        sidecar_role,
        error_code,
    };
    if let Ok(line) = serde_json::to_string(&event)
        && line.len() <= MAX_REQUEST_ERROR_EVENT_BYTES
    {
        let _ = writeln!(io::stderr().lock(), "{line}");
    }
}

fn encode_request<Request: Serialize>(
    method: &str,
    request: &Request,
) -> Result<(String, Option<Value>), OperationFailure> {
    let request_value = serde_json::to_value(request)
        .map_err(|_| OperationFailure::invalid_request("request cannot be encoded canonically"))?;
    let request_sha256 = hex::encode(Sha256::digest(serde_json::to_vec(&request_value).map_err(
        |_| OperationFailure::invalid_request("request cannot be encoded canonically"),
    )?));
    let replay_request = matches!(
        method,
        METHOD_PREPARE_NATIVE_ESCROW
            | METHOD_PREPARE_REVEALING_CLAIM
            | METHOD_PREPARE_WITNESSED_CLAIM
            | METHOD_COMPLETE_WITNESSED_CLAIM
    )
    .then_some(request_value);
    Ok((request_sha256, replay_request))
}

fn protocol_reply(context: &MessageContext, failure: OperationFailure) -> ProtocolErrorReply {
    ProtocolErrorReply::new(
        context.clone(),
        failure.code,
        ErrorMessage::new(failure.message).expect("static bridge error is protocol bounded"),
    )
}

fn protocol_error(context: &MessageContext, failure: OperationFailure) -> ErrorObjectOwned {
    error_object(protocol_reply(context, failure))
}

fn error_object(reply: ProtocolErrorReply) -> ErrorObjectOwned {
    ErrorObjectOwned::owned(PROTOCOL_ERROR_NUMBER, PROTOCOL_ERROR_SUMMARY, Some(reply))
}

fn valid_method(method: &str) -> bool {
    matches!(
        method,
        METHOD_DESCRIBE_RUNTIME
            | METHOD_PREPARE_NATIVE_ESCROW
            | METHOD_PREPARE_WITNESSED_ESCROW
            | METHOD_OBSERVE_ESCROW
            | METHOD_OBSERVE_WITNESSED_ESCROW
            | METHOD_PREPARE_REVEALING_CLAIM
            | METHOD_PREPARE_WITNESSED_CLAIM
            | METHOD_COMPLETE_WITNESSED_CLAIM
            | METHOD_OBSERVE_FINALIZED_WITNESSED_CLAIM
            | METHOD_OBSERVE_FINALIZED_WITNESSED_FUNDING
            | METHOD_OBSERVE_REVEALING_CLAIM
            | METHOD_PREPARE_NATIVE_REFUND
            | METHOD_OBSERVE_NATIVE_REFUND
            | METHOD_SUBMIT_TRANSACTION
    )
}
