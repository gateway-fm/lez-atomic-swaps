//! Separately locked official LEZ v0.2 sidecar boundary.

#![forbid(unsafe_code)]

#[cfg(target_os = "linux")]
pub mod actor_identity;
mod bridge_runtime;
mod bridge_server;
#[cfg(target_os = "linux")]
mod durable_reservation;
#[cfg(target_os = "linux")]
mod effect_submission;
mod finalized_claim_observation;
mod native_prepare;
mod runtime;
mod server;
mod vault_claim_prepare;

#[cfg(target_os = "linux")]
pub use durable_reservation::DurableReservationError;
#[cfg(target_os = "linux")]
pub use effect_submission::{
    JournaledVaultClaimEffect, PreparedVaultClaimEffect, SequencerSendFailure, SequencerSubmitApi,
    VaultClaimActorBinding, VaultClaimBeforeState, VaultClaimEffectIdentity,
    VaultClaimEffectJournal, VaultClaimEffectJournalError, VaultClaimEffectPrepareError,
    VaultClaimEffectScope, VaultClaimEffectState, VaultClaimSubmissionOutcome,
    VaultClaimSubmissionUncertainty, VaultClaimSubmitError, VaultClaimSubmitter,
    classify_sequencer_send_error,
};

pub use bridge_runtime::{BridgeRuntime, BridgeRuntimeError};
pub use bridge_server::{
    BridgeServerCapability, BridgeServerCapabilityError, BridgeServerConfig, BridgeServerError,
    BridgeServerHandle, start_bridge_server,
};
pub use finalized_claim_observation::{
    FinalizedIndexerApi, FinalizedWitnessedClaimObserver, OfficialIndexerRpc,
};
pub use native_prepare::{
    NativeEscrowPlanner, NativePrepareError, NonceSource, ZecEscrowInstruction,
    compute_custody_pda, compute_metadata_pda, decode_prepared_for_signer,
    prepared_from_transaction, program_id_from_hex, program_id_to_hex,
};

pub use runtime::{
    HealthProbe, OfficialNativeEscrowFacts, OfficialNodeRpc, OfficialVaultClaimFacts,
    RuntimeBoundary, RuntimeBoundaryError, RuntimeHealth, decode_official_public_transaction,
    validate_loopback_http_endpoint,
};
pub use server::{
    DescribeServerCapability, DescribeServerCapabilityError, DescribeServerConfig,
    DescribeServerError, DescribeServerHandle, start_describe_server,
};
pub use vault_claim_prepare::{
    PrepareVaultClaimRequest, PrepareVaultClaimResult, VaultClaimAllocation, VaultClaimNonceSource,
    VaultClaimPlanner, VaultClaimPrepareError,
};
