//! Node-owned BTC↔LEZ swap lifecycle (ADR 0213, stage 1).
//!
//! Both Nodes share this crate. It holds what used to live in the demo
//! runner's shell script: per-reservation role bootstrap, the Bitcoin funding
//! plan built from a Core wallet, the LEZ escrow/claim preparation through a
//! role sidecar, the two-leg `MuSig2` adaptor ceremony over Chat, and the
//! synthesis of the schema-6 actor configuration each role activates.
//!
//! Chain selection is configuration only: [`config::BtcRoleConfigV1`] names
//! the Bitcoin network, endpoints and policy; nothing here assumes regtest.

#[cfg(not(unix))]
compile_error!("lez-btc-role-lifecycle requires Unix file-permission semantics");

pub mod actor;
pub mod ceremony;
pub mod config;
pub mod funding;
pub mod layout;
pub mod lez;
pub mod sidecar;
pub mod wire;

pub use ceremony::{
    CeremonyLegPackets, LegSessions, MakerCeremony, TakerCeremony, TakerCeremonyOutcome,
};
pub use config::{BitcoinNetworkName, BtcRoleConfigV1, BtcRoleRuntime, RecoveryPolicyV1};
pub use funding::{BitcoinWallet, FundingPlan};
pub use layout::SwapLayout;
pub use lez::{LezRole, LezSidecar, PreparedEscrow};
pub use sidecar::{SwapSidecar, swap_run_id};
