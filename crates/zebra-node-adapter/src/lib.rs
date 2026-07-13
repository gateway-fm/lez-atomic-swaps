//! Typed, fail-closed Zebra JSON-RPC adapters for LEZ/ZEC swaps.

#![forbid(unsafe_code)]

mod claim;
mod first_lock;
mod funding_planner;
mod rpc;
mod signer;

pub use claim::{
    ZebraClaimError, ZebraClaimSigner, ZebraRefundError, ZebraRefundSigner, ZebraRpcClaimPort,
    ZebraRpcRefundPort,
};
pub use first_lock::{ZebraFirstLockError, ZebraRpcSwapPort};
pub use funding_planner::{
    ExactOutpointZcashFundingPlanner, ExactOutpointZcashFundingPlannerError, ZebraFundingSigner,
};
pub use rpc::{
    HttpZebraRpc, HttpZebraRpcConfig, HttpZebraRpcError, ZebraCanonicalBlock, ZebraChainIdentity,
    ZebraChainInfo, ZebraIdentityError, ZebraRpc, ZebraRpcChain, ZebraSubmissionFailure,
    ZebraTransactionState, ZebraUnspentOutput,
};
pub use signer::{RoleKeyedZcashSigner, RoleKeyedZcashSignerError};
