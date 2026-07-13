//! Typed, fail-closed Zebra JSON-RPC adapters for LEZ/ZEC swaps.

#![forbid(unsafe_code)]

mod claim;
mod first_lock;
mod rpc;

pub use claim::{ZebraClaimError, ZebraClaimSigner, ZebraRpcClaimPort};
pub use first_lock::{ZebraFirstLockError, ZebraRpcSwapPort};
pub use rpc::{
    HttpZebraRpc, HttpZebraRpcConfig, HttpZebraRpcError, ZebraCanonicalBlock, ZebraChainIdentity,
    ZebraChainInfo, ZebraIdentityError, ZebraRpc, ZebraRpcChain, ZebraSubmissionFailure,
    ZebraTransactionState, ZebraUnspentOutput,
};
