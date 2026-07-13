//! Separately locked official LEZ v0.2 sidecar boundary.

#![forbid(unsafe_code)]

mod runtime;
mod server;

pub use runtime::{
    HealthProbe, OfficialNodeRpc, RuntimeBoundary, RuntimeBoundaryError, RuntimeHealth,
    decode_official_public_transaction,
};
pub use server::{
    DescribeServerCapability, DescribeServerCapabilityError, DescribeServerConfig,
    DescribeServerError, DescribeServerHandle, start_describe_server,
};
