//! Typed, fail-closed Zebra JSON-RPC adapters for LEZ/ZEC swaps.

#![forbid(unsafe_code)]

mod first_lock;
mod rpc;

pub use first_lock::{ZebraFirstLockError, ZebraRpcSwapPort};
pub use rpc::{
    HttpZebraRpc, HttpZebraRpcConfig, HttpZebraRpcError, ZebraChainIdentity, ZebraChainInfo,
    ZebraIdentityError, ZebraRpc, ZebraRpcChain, ZebraTransactionState,
};
