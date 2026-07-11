//! Authenticated local JSON-RPC boundary for the headless maker.

use std::sync::Mutex;

use jsonrpsee::{RpcModule, core::RpcResult, types::ErrorObjectOwned};
use lez_swap_core::{ConfirmationPolicy, Pair, Phase, SwapCoordinator, SwapId, Timelocks};
use lez_swap_store::SqliteSwapStore;
use serde::{Deserialize, Serialize};

const NOT_FOUND: i32 = -32_004;
const CONFLICT: i32 = -32_009;
const INTERNAL_ERROR: i32 = -32_603;

/// Minimum capability length. Deployments should use at least 256 random bits.
pub const MINIMUM_CAPABILITY_LENGTH: usize = 24;

/// RPC context owned by one maker daemon.
pub struct MakerRpc {
    store: Mutex<SqliteSwapStore>,
}

impl std::fmt::Debug for MakerRpc {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MakerRpc")
            .field("store", &self.store)
            .finish()
    }
}

impl MakerRpc {
    /// Creates a maker RPC context. Transport authentication is configured by the daemon.
    #[must_use]
    pub fn new(store: SqliteSwapStore) -> Self {
        Self {
            store: Mutex::new(store),
        }
    }
}

/// Rejects trivially weak owner capabilities before transport setup.
///
/// # Errors
///
/// Returns an error when the capability is too short.
pub fn validate_capability(capability: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        capability.len() >= MINIMUM_CAPABILITY_LENGTH,
        "maker RPC capability must contain at least {MINIMUM_CAPABILITY_LENGTH} bytes"
    );
    Ok(())
}

/// Serializable operator-facing snapshot. Secret evidence is deliberately omitted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SwapView {
    /// Stable swap identifier.
    pub id: Box<str>,
    /// Foreign-chain pair.
    pub pair: Pair,
    /// Current durable phase.
    pub phase: Phase,
}

impl From<&SwapCoordinator> for SwapView {
    fn from(swap: &SwapCoordinator) -> Self {
        Self {
            id: swap.id().as_str().into(),
            pair: swap.pair(),
            phase: swap.phase(),
        }
    }
}

/// Parameters for creating one swap with already negotiated immutable terms.
#[derive(Debug, Deserialize, Serialize)]
pub struct CreateSwapRequest {
    /// Stable swap identifier.
    pub id: Box<str>,
    /// Foreign-chain pair.
    pub pair: Pair,
    /// Confirmations required before maker lock.
    pub confirmations: u32,
    /// LEZ refund deadline in the current normalized prototype domain.
    pub lez_refund_at: u64,
    /// Foreign-chain refund deadline in the current normalized prototype domain.
    pub foreign_refund_at: u64,
}

/// Parameters for reading one swap.
#[derive(Debug, Deserialize, Serialize)]
pub struct StatusRequest {
    /// Stable swap identifier.
    pub id: Box<str>,
}

/// Builds the RPC module shared by daemon transports and direct contract tests.
///
/// # Errors
///
/// Returns an error if a method cannot be registered.
pub fn rpc_module(context: MakerRpc) -> anyhow::Result<RpcModule<MakerRpc>> {
    let mut module = RpcModule::new(context);
    module.register_blocking_method::<RpcResult<SwapView>, _>(
        "swap_create",
        |params, context, _| {
            let request: CreateSwapRequest = params.one()?;

            let id = SwapId::new(request.id).map_err(invalid_request)?;
            let confirmations =
                ConfirmationPolicy::new(request.confirmations).map_err(invalid_request)?;
            let timelocks = Timelocks::new(request.lez_refund_at, request.foreign_refund_at)
                .map_err(invalid_request)?;
            let swap = SwapCoordinator::new(id, request.pair, confirmations, timelocks);
            let store = context
                .store
                .lock()
                .map_err(|_| rpc_error(INTERNAL_ERROR, "swap store lock poisoned"))?;
            if store
                .load(swap.id())
                .map_err(internal_store_error)?
                .is_some()
            {
                return Err(rpc_error(CONFLICT, "swap already exists"));
            }
            store.save(&swap).map_err(internal_store_error)?;
            Ok(SwapView::from(&swap))
        },
    )?;
    module.register_blocking_method::<RpcResult<SwapView>, _>(
        "swap_status",
        |params, context, _| {
            let request: StatusRequest = params.one()?;
            let id = SwapId::new(request.id).map_err(invalid_request)?;
            let store = context
                .store
                .lock()
                .map_err(|_| rpc_error(INTERNAL_ERROR, "swap store lock poisoned"))?;
            let swap = store
                .load(&id)
                .map_err(internal_store_error)?
                .ok_or_else(|| rpc_error(NOT_FOUND, "swap not found"))?;
            Ok(SwapView::from(&swap))
        },
    )?;
    Ok(module)
}

fn invalid_request(error: impl std::fmt::Display) -> ErrorObjectOwned {
    rpc_error(-32_602, error.to_string())
}

fn internal_store_error(error: impl std::fmt::Display) -> ErrorObjectOwned {
    rpc_error(INTERNAL_ERROR, format!("swap store failure: {error}"))
}

fn rpc_error(code: i32, message: impl Into<String>) -> ErrorObjectOwned {
    ErrorObjectOwned::owned(code, message.into(), None::<()>)
}
