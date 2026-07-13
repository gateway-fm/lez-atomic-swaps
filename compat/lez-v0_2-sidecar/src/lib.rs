//! Separately locked official LEZ v0.2 sidecar boundary.

#![forbid(unsafe_code)]

mod native_prepare;
mod runtime;
mod server;
mod vault_claim_prepare;

pub use native_prepare::{
    NativeEscrowPlanner, NativePrepareError, NonceSource, ZecEscrowInstruction,
    compute_custody_pda, compute_metadata_pda, decode_prepared_for_signer,
    prepared_from_transaction,
};

pub use runtime::{
    HealthProbe, OfficialNodeRpc, RuntimeBoundary, RuntimeBoundaryError, RuntimeHealth,
    decode_official_public_transaction,
};
pub use server::{
    DescribeServerCapability, DescribeServerCapabilityError, DescribeServerConfig,
    DescribeServerError, DescribeServerHandle, start_describe_server,
};
pub use vault_claim_prepare::{
    PrepareVaultClaimRequest, PrepareVaultClaimResult, VaultClaimAllocation, VaultClaimNonceSource,
    VaultClaimPlanner, VaultClaimPrepareError,
};
