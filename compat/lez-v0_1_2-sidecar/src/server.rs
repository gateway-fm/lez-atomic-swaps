use std::{
    collections::BTreeMap,
    fmt,
    fs::{self, File},
    future::Future,
    io::{self, Write as _},
    path::{Path, PathBuf},
    sync::Arc,
};

use jsonrpsee::{RpcModule, server::ServerBuilder, types::ErrorObjectOwned};
use lez_bridge_protocol::{
    DescribeRuntimeRequest, DescribeRuntimeResult, ErrorCode, ErrorMessage, MAX_RPC_BODY_BYTES,
    METHOD_DESCRIBE_RUNTIME, METHOD_OBSERVE_ESCROW, METHOD_OBSERVE_REVEALING_CLAIM,
    METHOD_PREPARE_NATIVE_ESCROW, METHOD_PREPARE_REVEALING_CLAIM, METHOD_SUBMIT_TRANSACTION,
    MessageContext, ObserveEscrowRequest, ObserveRevealingClaimRequest, Participant,
    PrepareNativeEscrowRequest, PrepareNativeEscrowResult, PrepareRevealingClaimRequest,
    PrepareRevealingClaimResult, ProtocolErrorReply, RUN_ID_HEADER, RunId, RuntimeDescriptor,
    SIDECAR_ROLE_HEADER, SubmitTransactionRequest,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest as _, Sha256};
use tokio::sync::Mutex;
use tower::ServiceBuilder;
use tower_http::validate_request::ValidateRequestHeaderLayer;
use zeroize::Zeroize;

use crate::{ExactTransactionSubmitter, NativeEscrowPlanner, SidecarError};

const MIN_CAPABILITY_BYTES: usize = 32;
const MAX_CAPABILITY_BYTES: usize = 128;
const STORE_SCHEMA_VERSION: u16 = 1;
const MAX_STORE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_STORE_ENTRIES: usize = 4_096;
const PROTOCOL_ERROR_NUMBER: i32 = -32_010;
const PROTOCOL_ERROR_SUMMARY: &str = "LEZ bridge request failed";

/// A bounded bearer capability dedicated to one local sidecar.
pub struct BridgeServerCapability(String);

impl BridgeServerCapability {
    /// Creates a capability in the shared safe HTTP-header grammar.
    ///
    /// # Errors
    ///
    /// Rejects values outside 32..=128 ASCII bytes or `[A-Za-z0-9._-]`.
    pub fn new(value: impl Into<String>) -> Result<Self, BridgeServerCapabilityError> {
        let value = value.into();
        if (MIN_CAPABILITY_BYTES..=MAX_CAPABILITY_BYTES).contains(&value.len())
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        {
            Ok(Self(value))
        } else {
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

/// A server capability failed its bounded bearer-token grammar.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[error("sidecar capability must be 32..=128 ASCII characters from [A-Za-z0-9._-]")]
pub struct BridgeServerCapabilityError;

/// Immutable run, runtime, authentication, and durable-store configuration.
pub struct BridgeServerConfig {
    run_id: RunId,
    runtime: RuntimeDescriptor,
    capability: BridgeServerCapability,
    idempotency_path: PathBuf,
}

impl BridgeServerConfig {
    /// Creates a configuration for one ephemeral loopback server.
    #[must_use]
    pub fn new(
        run_id: RunId,
        runtime: RuntimeDescriptor,
        capability: BridgeServerCapability,
        idempotency_path: PathBuf,
    ) -> Self {
        Self {
            run_id,
            runtime,
            capability,
            idempotency_path,
        }
    }
}

impl fmt::Debug for BridgeServerConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("BridgeServerConfig")
            .field("run_id", &self.run_id)
            .field("runtime", &self.runtime)
            .field("capability", &self.capability)
            .field("idempotency_path", &self.idempotency_path)
            .finish()
    }
}

/// Server construction, persistence, or shutdown failure.
#[derive(Debug, thiserror::Error)]
pub enum BridgeServerError {
    /// Durable state could not be safely opened, validated, or replaced.
    #[error("durable bridge idempotency state is unavailable")]
    DurableState(#[source] io::Error),
    /// Existing state is malformed or belongs to another run/runtime.
    #[error("durable bridge idempotency state does not match this sidecar")]
    InvalidDurableState,
    /// The ephemeral loopback listener could not be created.
    #[error("ephemeral loopback bridge listener is unavailable")]
    Bind,
    /// A running server could not be stopped cleanly.
    #[error("bridge server could not be stopped")]
    Stop,
}

/// Running ephemeral loopback bridge server.
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
    /// Returns the literal loopback HTTP endpoint with its ephemeral port.
    #[must_use]
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// Stops the server and waits until the listener and tasks are closed.
    ///
    /// # Errors
    ///
    /// Returns an error if the server was already stopped through another handle.
    pub async fn stop(self) -> Result<(), BridgeServerError> {
        self.handle.stop().map_err(|_| BridgeServerError::Stop)?;
        self.handle.stopped().await;
        Ok(())
    }
}

#[derive(Clone)]
struct ServerState {
    run_id: RunId,
    runtime: RuntimeDescriptor,
    planner: Arc<NativeEscrowPlanner>,
    submitter: Arc<dyn ExactTransactionSubmitter>,
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
    runtime: RuntimeDescriptor,
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
}

#[derive(Debug)]
struct DurableStore {
    path: PathBuf,
    persisted: PersistedStore,
    dirty: bool,
}

impl DurableStore {
    fn open(
        path: PathBuf,
        run_id: &RunId,
        runtime: &RuntimeDescriptor,
    ) -> Result<Self, BridgeServerError> {
        let persisted = if path.exists() {
            let link_metadata =
                fs::symlink_metadata(&path).map_err(BridgeServerError::DurableState)?;
            if link_metadata.file_type().is_symlink() {
                return Err(BridgeServerError::InvalidDurableState);
            }
            let metadata = fs::metadata(&path).map_err(BridgeServerError::DurableState)?;
            if !metadata.is_file() || metadata.len() > MAX_STORE_BYTES {
                return Err(BridgeServerError::InvalidDurableState);
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;

                if metadata.permissions().mode() & 0o077 != 0 {
                    return Err(BridgeServerError::InvalidDurableState);
                }
            }
            let bytes = fs::read(&path).map_err(BridgeServerError::DurableState)?;
            serde_json::from_slice::<PersistedStore>(&bytes)
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
                            METHOD_PREPARE_NATIVE_ESCROW | METHOD_PREPARE_REVEALING_CLAIM
                        )
                    })
            })
        {
            return Err(BridgeServerError::InvalidDurableState);
        }
        let store = Self {
            path,
            persisted,
            dirty: false,
        };
        if !store.path.exists() {
            store.persist()?;
        }
        Ok(store)
    }

    fn restored_prepare(
        &self,
    ) -> Result<Option<(PrepareNativeEscrowRequest, PrepareNativeEscrowResult)>, BridgeServerError>
    {
        let mut restored = None;
        for entry in self.persisted.entries.values() {
            if entry.method != METHOD_PREPARE_NATIVE_ESCROW {
                continue;
            }
            let PersistedOutcome::Success(response) = &entry.outcome else {
                continue;
            };
            if restored.is_some() {
                return Err(BridgeServerError::InvalidDurableState);
            }
            let request_value = entry
                .replay_request
                .clone()
                .ok_or(BridgeServerError::InvalidDurableState)?;
            let request_bytes = serde_json::to_vec(&request_value)
                .map_err(|_| BridgeServerError::InvalidDurableState)?;
            if hex::encode(Sha256::digest(&request_bytes)) != entry.request_sha256 {
                return Err(BridgeServerError::InvalidDurableState);
            }
            let request = serde_json::from_value::<PrepareNativeEscrowRequest>(request_value)
                .map_err(|_| BridgeServerError::InvalidDurableState)?;
            let result = serde_json::from_value::<PrepareNativeEscrowResult>(response.clone())
                .map_err(|_| BridgeServerError::InvalidDurableState)?;
            restored = Some((request, result));
        }
        Ok(restored)
    }

    fn restored_claim(
        &self,
    ) -> Result<
        Option<(PrepareRevealingClaimRequest, PrepareRevealingClaimResult)>,
        BridgeServerError,
    > {
        let mut restored = None;
        for entry in self.persisted.entries.values() {
            if entry.method != METHOD_PREPARE_REVEALING_CLAIM {
                continue;
            }
            let PersistedOutcome::Success(response) = &entry.outcome else {
                continue;
            };
            if restored.is_some() {
                return Err(BridgeServerError::InvalidDurableState);
            }
            let request_value = entry
                .replay_request
                .clone()
                .ok_or(BridgeServerError::InvalidDurableState)?;
            let request_bytes = serde_json::to_vec(&request_value)
                .map_err(|_| BridgeServerError::InvalidDurableState)?;
            if hex::encode(Sha256::digest(&request_bytes)) != entry.request_sha256 {
                return Err(BridgeServerError::InvalidDurableState);
            }
            let request = serde_json::from_value::<PrepareRevealingClaimRequest>(request_value)
                .map_err(|_| BridgeServerError::InvalidDurableState)?;
            let result = serde_json::from_value::<PrepareRevealingClaimResult>(response.clone())
                .map_err(|_| BridgeServerError::InvalidDurableState)?;
            restored = Some((request, result));
        }
        Ok(restored)
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
        if temporary
            .as_file()
            .metadata()
            .map_err(BridgeServerError::DurableState)?
            .len()
            > MAX_STORE_BYTES
        {
            return Err(BridgeServerError::InvalidDurableState);
        }
        temporary.flush().map_err(BridgeServerError::DurableState)?;
        temporary
            .as_file()
            .sync_all()
            .map_err(BridgeServerError::DurableState)?;
        temporary
            .persist(&self.path)
            .map_err(|error| BridgeServerError::DurableState(error.error))?;
        File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(BridgeServerError::DurableState)
    }
}

#[derive(Clone, Copy, Debug)]
struct OperationFailure {
    code: ErrorCode,
    message: &'static str,
}

impl OperationFailure {
    const fn invalid_request(message: &'static str) -> Self {
        Self {
            code: ErrorCode::InvalidRequest,
            message,
        }
    }
}

impl DurableStore {
    fn replay(
        &mut self,
        request_id: &str,
        method: &str,
        request_sha256: &str,
    ) -> Result<Option<PersistedOutcome>, OperationFailure> {
        let Some(cached) = self.persisted.entries.get(request_id) else {
            return Ok(None);
        };
        if cached.method != method || cached.request_sha256 != request_sha256 {
            return Err(OperationFailure::invalid_request(
                "request id was already used for a different request",
            ));
        }
        let outcome = cached.outcome.clone();
        if self.dirty {
            self.persist().map_err(|_| OperationFailure {
                code: ErrorCode::Internal,
                message: "request outcome could not be made durable",
            })?;
            self.dirty = false;
        }
        Ok(Some(outcome))
    }

    fn reserve(
        &mut self,
        method: &str,
        context: &MessageContext,
        request_sha256: &str,
    ) -> Result<(), OperationFailure> {
        if self.persisted.entries.len() >= MAX_STORE_ENTRIES {
            return Err(OperationFailure {
                code: ErrorCode::Internal,
                message: "durable request capacity is exhausted",
            });
        }
        if method != METHOD_SUBMIT_TRANSACTION {
            return Ok(());
        }
        let outcome = PersistedOutcome::SubmissionInFlight(protocol_reply(
            context,
            OperationFailure {
                code: ErrorCode::UnknownSubmissionOutcome,
                message: "official node submission outcome is unknown",
            },
        ));
        self.persisted.entries.insert(
            context.request_id.as_str().to_owned(),
            PersistedEntry {
                method: method.to_owned(),
                request_sha256: request_sha256.to_owned(),
                replay_request: None,
                outcome,
            },
        );
        self.dirty = true;
        self.persist().map_err(|_| OperationFailure {
            code: ErrorCode::Internal,
            message: "submission guard could not be made durable",
        })?;
        self.dirty = false;
        Ok(())
    }

    fn finish(
        &mut self,
        method: &str,
        context: &MessageContext,
        request_sha256: String,
        replay_request: Option<Value>,
        outcome: PersistedOutcome,
    ) -> Result<(), OperationFailure> {
        self.persisted.entries.insert(
            context.request_id.as_str().to_owned(),
            PersistedEntry {
                method: method.to_owned(),
                request_sha256,
                replay_request,
                outcome,
            },
        );
        self.dirty = true;
        self.persist().map_err(|_| OperationFailure {
            code: ErrorCode::Internal,
            message: "request outcome could not be made durable",
        })?;
        self.dirty = false;
        Ok(())
    }
}

impl From<SidecarError> for OperationFailure {
    fn from(error: SidecarError) -> Self {
        let code = match error {
            SidecarError::WrongSidecarRole | SidecarError::WrongClaimantRole => {
                ErrorCode::WrongSidecarRole
            }
            SidecarError::NodeRejected
            | SidecarError::TransactionNotPrepared
            | SidecarError::InvalidTransactionBytes
            | SidecarError::WrongTransactionId
            | SidecarError::InvalidSignature => ErrorCode::InvalidTransaction,
            SidecarError::UnknownSubmissionOutcome => ErrorCode::UnknownSubmissionOutcome,
            SidecarError::MovingTip => ErrorCode::MovingTip,
            SidecarError::AmbiguousDiscovery => ErrorCode::AmbiguousDiscovery,
            SidecarError::NonceUnavailable
            | SidecarError::NodeObservationUnavailable
            | SidecarError::InvalidNodeResponse => ErrorCode::Unavailable,
            SidecarError::WrongSigner
            | SidecarError::WrongDepositorRole
            | SidecarError::WrongEscrowProgram
            | SidecarError::WrongAuthenticatedTransferProgram
            | SidecarError::WrongRuntimeCompatibility
            | SidecarError::WrongRuntimeIdentity
            | SidecarError::ActivePrepare
            | SidecarError::ActiveClaimPrepare
            | SidecarError::WrongClaimPreimage
            | SidecarError::InvalidFundingTransaction
            | SidecarError::InvalidNodeEndpoint
            | SidecarError::NonceOverflow
            | SidecarError::InstructionEncoding
            | SidecarError::ProtocolEncoding => ErrorCode::InvalidRequest,
        };
        let message = match code {
            ErrorCode::WrongSidecarRole => "request targets the wrong sidecar role",
            ErrorCode::InvalidTransaction => "official transaction validation failed",
            ErrorCode::Unavailable => "required official node fact is unavailable",
            ErrorCode::UnknownSubmissionOutcome => "official node submission outcome is unknown",
            ErrorCode::InvalidRequest
            | ErrorCode::AmbiguousDiscovery
            | ErrorCode::MovingTip
            | ErrorCode::Internal => "official sidecar rejected the request",
        };
        Self { code, message }
    }
}

/// Starts one single-flight authenticated sidecar on `127.0.0.1:0`.
///
/// Request and response bodies are hard bounded. Header authentication, run,
/// and role validation execute in HTTP middleware before JSON-RPC parsing.
/// Each request is attempted once; outcomes are durably cached by request ID.
///
/// # Errors
///
/// Returns an error if durable state is invalid/unavailable or loopback bind fails.
pub async fn start_bridge_server<S>(
    mut config: BridgeServerConfig,
    planner: Arc<NativeEscrowPlanner>,
    submitter: Arc<S>,
) -> Result<BridgeServerHandle, BridgeServerError>
where
    S: ExactTransactionSubmitter + 'static,
{
    let store = DurableStore::open(
        config.idempotency_path.clone(),
        &config.run_id,
        &config.runtime,
    )?;
    if let Some((request, result)) = store.restored_prepare()? {
        planner
            .restore_prepared(request, result)
            .await
            .map_err(|_| BridgeServerError::InvalidDurableState)?;
    }
    if let Some((request, result)) = store.restored_claim()? {
        planner
            .restore_revealing_claim(&request, result)
            .await
            .map_err(|_| BridgeServerError::InvalidDurableState)?;
    }
    let role = match config.runtime.sidecar_role {
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
        .build("127.0.0.1:0")
        .await
        .map_err(|_| BridgeServerError::Bind)?;
    let address = server.local_addr().map_err(|_| BridgeServerError::Bind)?;
    if !address.ip().is_loopback() || address.port() == 0 {
        return Err(BridgeServerError::Bind);
    }

    let state = ServerState {
        run_id: config.run_id,
        runtime: config.runtime,
        planner,
        submitter,
        store: Arc::new(Mutex::new(store)),
    };
    let mut module = RpcModule::new(state);
    register_methods(&mut module).map_err(|_| BridgeServerError::Bind)?;
    let handle = server.start(module);
    Ok(BridgeServerHandle {
        endpoint: format!("http://{address}"),
        handle,
    })
}

fn register_methods(
    module: &mut RpcModule<ServerState>,
) -> Result<(), jsonrpsee::core::RegisterMethodError> {
    register_runtime_and_escrow_methods(module)?;
    register_claim_and_submit_methods(module)
}

fn register_runtime_and_escrow_methods(
    module: &mut RpcModule<ServerState>,
) -> Result<(), jsonrpsee::core::RegisterMethodError> {
    module.register_async_method(METHOD_DESCRIBE_RUNTIME, |params, state, _| async move {
        let request: DescribeRuntimeRequest = params.one()?;
        let runtime = state.runtime.clone();
        let response_context = request.context.clone();
        state
            .execute(
                METHOD_DESCRIBE_RUNTIME,
                &request.context,
                &request,
                || async move { to_value(DescribeRuntimeResult::new(response_context, runtime)) },
            )
            .await
    })?;
    module.register_async_method(
        METHOD_PREPARE_NATIVE_ESCROW,
        |params, state, _| async move {
            let request: PrepareNativeEscrowRequest = params.one()?;
            state.validate_runtime(&request.context, &request.runtime)?;
            let planner = Arc::clone(&state.planner);
            let operation_request = request.clone();
            state
                .execute(
                    METHOD_PREPARE_NATIVE_ESCROW,
                    &request.context,
                    &request,
                    || async move {
                        planner
                            .prepare(operation_request)
                            .await
                            .map_err(OperationFailure::from)
                            .and_then(to_value)
                    },
                )
                .await
        },
    )?;
    module.register_async_method(METHOD_OBSERVE_ESCROW, |params, state, _| async move {
        let request: ObserveEscrowRequest = params.one()?;
        state.validate_runtime(&request.context, &request.runtime)?;
        let planner = Arc::clone(&state.planner);
        let observer = Arc::clone(&state.submitter);
        let operation_request = request.clone();
        state
            .execute(
                METHOD_OBSERVE_ESCROW,
                &request.context,
                &request,
                || async move {
                    observer
                        .observe_native_escrow(&planner, &operation_request)
                        .await
                        .map_err(OperationFailure::from)
                        .and_then(to_value)
                },
            )
            .await
    })?;
    Ok(())
}

fn register_claim_and_submit_methods(
    module: &mut RpcModule<ServerState>,
) -> Result<(), jsonrpsee::core::RegisterMethodError> {
    module.register_async_method(
        METHOD_PREPARE_REVEALING_CLAIM,
        |params, state, _| async move {
            let request = Arc::new(params.one::<PrepareRevealingClaimRequest>()?);
            state.validate_runtime(&request.context, &request.runtime)?;
            let planner = Arc::clone(&state.planner);
            let operation_request = Arc::clone(&request);
            state
                .execute(
                    METHOD_PREPARE_REVEALING_CLAIM,
                    &request.context,
                    request.as_ref(),
                    || async move {
                        planner
                            .prepare_revealing_claim(operation_request.as_ref())
                            .await
                            .map_err(OperationFailure::from)
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
            let planner = Arc::clone(&state.planner);
            let observer = Arc::clone(&state.submitter);
            let operation_request = request.clone();
            state
                .execute(
                    METHOD_OBSERVE_REVEALING_CLAIM,
                    &request.context,
                    &request,
                    || async move {
                        observer
                            .observe_revealing_claim(&planner, &operation_request)
                            .await
                            .map_err(OperationFailure::from)
                            .and_then(to_value)
                    },
                )
                .await
        },
    )?;
    module.register_async_method(METHOD_SUBMIT_TRANSACTION, |params, state, _| async move {
        let request: SubmitTransactionRequest = params.one()?;
        state.validate_runtime(&request.context, &request.runtime)?;
        let planner = Arc::clone(&state.planner);
        let submitter = Arc::clone(&state.submitter);
        let operation_request = request.clone();
        state
            .execute(
                METHOD_SUBMIT_TRANSACTION,
                &request.context,
                &request,
                || async move {
                    submitter
                        .submit_exact(&planner, &operation_request)
                        .await
                        .map_err(OperationFailure::from)
                        .and_then(to_value)
                },
            )
            .await
    })?;
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
        if context.sidecar_role != self.runtime.sidecar_role {
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
        runtime: &RuntimeDescriptor,
    ) -> Result<(), ErrorObjectOwned> {
        self.validate_context(context)?;
        if runtime != &self.runtime {
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
        let mut store = self.store.lock().await;
        if let Some(outcome) = store
            .replay(context.request_id.as_str(), method, &request_sha256)
            .map_err(|failure| protocol_error(context, failure))?
        {
            return outcome.into_rpc_result();
        }
        store
            .reserve(method, context, &request_sha256)
            .map_err(|failure| protocol_error(context, failure))?;

        let outcome = match operation().await {
            Ok(value) => PersistedOutcome::Success(value),
            Err(failure) => PersistedOutcome::Error(protocol_reply(context, failure)),
        };
        store
            .finish(
                method,
                context,
                request_sha256,
                replay_request,
                outcome.clone(),
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
        }
    }
}

fn to_value<T: Serialize>(value: T) -> Result<Value, OperationFailure> {
    serde_json::to_value(value).map_err(|_| OperationFailure {
        code: ErrorCode::Internal,
        message: "sidecar result could not be encoded",
    })
}

fn encode_request<Request: Serialize>(
    method: &str,
    request: &Request,
) -> Result<(String, Option<Value>), OperationFailure> {
    let request_value = serde_json::to_value(request)
        .map_err(|_| OperationFailure::invalid_request("request cannot be encoded canonically"))?;
    let request_bytes = serde_json::to_vec(&request_value)
        .map_err(|_| OperationFailure::invalid_request("request cannot be encoded canonically"))?;
    let request_sha256 = hex::encode(Sha256::digest(&request_bytes));
    let replay_request = matches!(
        method,
        METHOD_PREPARE_NATIVE_ESCROW | METHOD_PREPARE_REVEALING_CLAIM
    )
    .then_some(request_value);
    Ok((request_sha256, replay_request))
}

fn protocol_reply(context: &MessageContext, failure: OperationFailure) -> ProtocolErrorReply {
    let message = ErrorMessage::new(failure.message)
        .expect("all static bridge server error messages are protocol bounded");
    ProtocolErrorReply::new(context.clone(), failure.code, message)
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
            | METHOD_OBSERVE_ESCROW
            | METHOD_PREPARE_REVEALING_CLAIM
            | METHOD_OBSERVE_REVEALING_CLAIM
            | METHOD_SUBMIT_TRANSACTION
    )
}
