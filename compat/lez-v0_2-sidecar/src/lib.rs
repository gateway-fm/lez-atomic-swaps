//! Separately locked official LEZ v0.2 sidecar boundary.

#![forbid(unsafe_code)]

#[cfg(target_os = "linux")]
mod durable_reservation;
#[cfg(target_os = "linux")]
mod effect_submission;
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
