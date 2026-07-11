//! Authenticated local JSON-RPC boundary for the headless maker.

use std::sync::Mutex;

use jsonrpsee::{RpcModule, core::RpcResult, types::ErrorObjectOwned};
use lez_swap_core::{ConfirmationPolicy, Pair, Phase, SwapCoordinator, SwapId, Timelocks};
use lez_swap_store::SqliteSwapStore;
use serde::{Deserialize, Serialize};
use subtle::ConstantTimeEq;

const UNAUTHORIZED: i32 = -32_001;
const NOT_FOUND: i32 = -32_004;
const CONFLICT: i32 = -32_009;
const INTERNAL_ERROR: i32 = -32_603;

/// Minimum capability length. Deployments should use at least 256 random bits.
pub const MINIMUM_CAPABILITY_LENGTH: usize = 24;

/// RPC context owned by one maker daemon.
pub struct MakerRpc {
    store: Mutex<SqliteSwapStore>,
    capability: Box<str>,
}

impl std::fmt::Debug for MakerRpc {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MakerRpc")
            .field("store", &self.store)
            .field("capability", &"[REDACTED]")
            .finish()
    }
}

impl MakerRpc {
    /// Creates a maker RPC context with a non-trivial owner capability.
    ///
    /// # Errors
    ///
    /// Returns an error when the capability is too short to be an acceptable secret.
    pub fn new(store: SqliteSwapStore, capability: impl Into<Box<str>>) -> anyhow::Result<Self> {
        let capability = capability.into();
        anyhow::ensure!(
            capability.len() >= MINIMUM_CAPABILITY_LENGTH,
            "maker RPC capability must contain at least {MINIMUM_CAPABILITY_LENGTH} bytes"
        );
        Ok(Self {
            store: Mutex::new(store),
            capability,
        })
    }

    fn authorize(&self, presented: &str) -> RpcResult<()> {
        if bool::from(self.capability.as_bytes().ct_eq(presented.as_bytes())) {
            Ok(())
        } else {
            Err(rpc_error(UNAUTHORIZED, "unauthorized"))
        }
    }
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
#[derive(Deserialize, Serialize)]
pub struct CreateSwapRequest {
    /// Owner capability.
    pub capability: Box<str>,
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

impl std::fmt::Debug for CreateSwapRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CreateSwapRequest")
            .field("capability", &"[REDACTED]")
            .field("id", &self.id)
            .field("pair", &self.pair)
            .field("confirmations", &self.confirmations)
            .field("lez_refund_at", &self.lez_refund_at)
            .field("foreign_refund_at", &self.foreign_refund_at)
            .finish()
    }
}

/// Parameters for reading one swap.
#[derive(Deserialize, Serialize)]
pub struct StatusRequest {
    /// Owner capability.
    pub capability: Box<str>,
    /// Stable swap identifier.
    pub id: Box<str>,
}

impl std::fmt::Debug for StatusRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("StatusRequest")
            .field("capability", &"[REDACTED]")
            .field("id", &self.id)
            .finish()
    }
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
            context.authorize(&request.capability)?;

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
            context.authorize(&request.capability)?;
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
