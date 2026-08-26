use std::{fmt, sync::Arc};

use jsonrpsee::{RpcModule, server::ServerBuilder, types::ErrorObjectOwned};
use lez_bridge_protocol::{
    DescribeRuntimeRequest, DescribeRuntimeResult, ErrorCode, ErrorMessage, MAX_RPC_BODY_BYTES,
    METHOD_DESCRIBE_RUNTIME, MessageContext, Participant, ProtocolErrorReply, RUN_ID_HEADER, RunId,
    SIDECAR_ROLE_HEADER,
};
use tower::ServiceBuilder;
use tower_http::validate_request::ValidateRequestHeaderLayer;
use zeroize::Zeroize as _;

use crate::{RuntimeBoundary, RuntimeBoundaryError};

const MIN_CAPABILITY_BYTES: usize = 32;
const MAX_CAPABILITY_BYTES: usize = 128;
const PROTOCOL_ERROR_NUMBER: i32 = -32_010;
const PROTOCOL_ERROR_SUMMARY: &str = "LEZ bridge request failed";

/// Bounded bearer capability dedicated to one local v0.2 sidecar.
pub struct DescribeServerCapability(String);

impl DescribeServerCapability {
    /// Creates a capability in the bridge client's safe HTTP-header grammar.
    ///
    /// # Errors
    ///
    /// Rejects values outside 32..=128 ASCII bytes or `[A-Za-z0-9._-]`.
    pub fn new(value: impl Into<String>) -> Result<Self, DescribeServerCapabilityError> {
        let mut value = value.into();
        if (MIN_CAPABILITY_BYTES..=MAX_CAPABILITY_BYTES).contains(&value.len())
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        {
            Ok(Self(value))
        } else {
            value.zeroize();
            Err(DescribeServerCapabilityError)
        }
    }
}

impl Drop for DescribeServerCapability {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

impl fmt::Debug for DescribeServerCapability {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("DescribeServerCapability([REDACTED])")
    }
}

/// A describe-server capability failed its bounded bearer-token grammar.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[error("sidecar capability must be 32..=128 ASCII characters from [A-Za-z0-9._-]")]
pub struct DescribeServerCapabilityError;

/// Immutable run and authentication configuration for the describe boundary.
pub struct DescribeServerConfig {
    run_id: RunId,
    capability: DescribeServerCapability,
}

impl DescribeServerConfig {
    /// Creates an ephemeral authenticated server configuration.
    #[must_use]
    pub const fn new(run_id: RunId, capability: DescribeServerCapability) -> Self {
        Self { run_id, capability }
    }
}

impl fmt::Debug for DescribeServerConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DescribeServerConfig")
            .field("run_id", &self.run_id)
            .field("capability", &self.capability)
            .finish()
    }
}

/// Runtime-health, listener, registration, or shutdown failure.
#[derive(Debug, thiserror::Error)]
pub enum DescribeServerError {
    /// The exact official runtime health gate failed before binding.
    #[error("official runtime health gate failed")]
    Runtime(#[from] RuntimeBoundaryError),
    /// The ephemeral authenticated loopback listener could not be created.
    #[error("ephemeral loopback describe server is unavailable")]
    Bind,
    /// The running server could not be stopped cleanly.
    #[error("describe server could not be stopped")]
    Stop,
}

/// Running ephemeral loopback server that exposes only `describe_runtime`.
pub struct DescribeServerHandle {
    endpoint: String,
    handle: jsonrpsee::server::ServerHandle,
}

impl fmt::Debug for DescribeServerHandle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DescribeServerHandle")
            .field("endpoint", &self.endpoint)
            .finish_non_exhaustive()
    }
}

impl DescribeServerHandle {
    /// Returns the literal loopback HTTP endpoint with its ephemeral port.
    #[must_use]
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// Stops the listener and waits for its tasks to finish.
    ///
    /// # Errors
    ///
    /// Returns an error if the server was already stopped through another handle.
    pub async fn stop(self) -> Result<(), DescribeServerError> {
        self.handle.stop().map_err(|_| DescribeServerError::Stop)?;
        self.handle.stopped().await;
        Ok(())
    }
}

#[derive(Debug)]
struct ServerState {
    run_id: RunId,
    boundary: Arc<RuntimeBoundary>,
}

impl ServerState {
    fn describe(
        &self,
        request: DescribeRuntimeRequest,
    ) -> Result<DescribeRuntimeResult, ErrorObjectOwned> {
        self.validate_context(&request.context)?;
        Ok(DescribeRuntimeResult::new(
            request.context,
            self.boundary.describe().clone(),
        ))
    }

    fn validate_context(&self, context: &MessageContext) -> Result<(), ErrorObjectOwned> {
        if context.run_id != self.run_id {
            return Err(protocol_error(
                context,
                ErrorCode::InvalidRequest,
                "request targets the wrong composed run",
            ));
        }
        if context.sidecar_role != self.boundary.describe().sidecar_role {
            return Err(protocol_error(
                context,
                ErrorCode::WrongSidecarRole,
                "request targets the wrong sidecar role",
            ));
        }
        Ok(())
    }
}

/// Starts one health-gated, authenticated, describe-only server on `127.0.0.1:0`.
///
/// The official v0.2 sequencer must answer both `checkHealth` and `getChannelId`,
/// and the channel must exactly equal the immutable runtime descriptor, before
/// the listener is created. No transaction method is registered in this slice.
///
/// # Errors
///
/// Returns an error when health/identity validation or loopback binding fails.
pub async fn start_describe_server(
    mut config: DescribeServerConfig,
    boundary: Arc<RuntimeBoundary>,
) -> Result<DescribeServerHandle, DescribeServerError> {
    boundary.verify_health().await?;

    let role = match boundary.describe().sidecar_role {
        Participant::Maker => "maker",
        Participant::Taker => "taker",
    };
    let mut authorization = String::with_capacity("Bearer ".len() + config.capability.0.len());
    authorization.push_str("Bearer ");
    authorization.push_str(&config.capability.0);
    let middleware = ServiceBuilder::new()
        .layer(
            ValidateRequestHeaderLayer::has_header_value("authorization", &authorization)
                .map_err(|_| DescribeServerError::Bind)?,
        )
        .layer(
            ValidateRequestHeaderLayer::has_header_value(RUN_ID_HEADER, config.run_id.as_str())
                .map_err(|_| DescribeServerError::Bind)?,
        )
        .layer(
            ValidateRequestHeaderLayer::has_header_value(SIDECAR_ROLE_HEADER, role)
                .map_err(|_| DescribeServerError::Bind)?,
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
        .map_err(|_| DescribeServerError::Bind)?;
    let address = server.local_addr().map_err(|_| DescribeServerError::Bind)?;
    if !address.ip().is_loopback() || address.port() == 0 {
        return Err(DescribeServerError::Bind);
    }

    let state = ServerState {
        run_id: config.run_id,
        boundary,
    };
    let mut module = RpcModule::new(state);
    module
        .register_async_method(METHOD_DESCRIBE_RUNTIME, |params, state, _| async move {
            let request: DescribeRuntimeRequest = params.one()?;
            state.describe(request)
        })
        .map_err(|_| DescribeServerError::Bind)?;
    let handle = server.start(module);
    Ok(DescribeServerHandle {
        endpoint: format!("http://{address}"),
        handle,
    })
}

fn protocol_error(
    context: &MessageContext,
    code: ErrorCode,
    message: &'static str,
) -> ErrorObjectOwned {
    let message = ErrorMessage::new(message).expect("static protocol error text is bounded");
    let reply = ProtocolErrorReply::new(context.clone(), code, message);
    ErrorObjectOwned::owned(PROTOCOL_ERROR_NUMBER, PROTOCOL_ERROR_SUMMARY, Some(reply))
}
