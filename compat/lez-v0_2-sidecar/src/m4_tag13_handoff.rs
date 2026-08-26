//! Typed, effect-free handoff from finalized M4 tag-13 state to role sidecars.

use crate::{
    CHECKED_M4_ESCROW_PROGRAM_ID_HEX, DurableReservationError, StateDirectoryLease,
    StateDirectoryLeaseError,
    durable_reservation::{DurableReservationStore, ReservationKind},
    native_prepare::validate_prepared_native_xmr_escrow_v3_public,
    program_id_from_hex,
};
use lez_bridge_protocol::{
    ChainClock, DiscoveryWindow, Hex32, MessageContext, Participant,
    PrepareNativeXmrEscrowV3Request, PrepareNativeXmrEscrowV3Result, RequestId, RunId,
    RuntimeCompatibility, RuntimeDescriptor, SubmissionOutcome, TransactionId, XmrNativeEffectV3,
    XmrNativeEscrowTermsV3,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use std::{ffi::OsStr, path::Path};

const EVIDENCE_FILE: &str = "m4-xmr-stage-a-tag13-evidence.v2.json";
const RESERVATION_FILE: &str = "xmr-native-escrow-reservation.v3.json";
const EVIDENCE_SCHEMA: &str = "lez_v02_m4_xmr_stage_a_tag13_poc_v2";
const RECEIPT_SCHEMA: &str = "lez_v02_m4_tag13_handoff_v1";
const MAX_EVIDENCE_BYTES: u64 = 256 * 1024;
const MAX_RESERVATION_BYTES: u64 = 8 * 1024 * 1024;
const MAX_ARTIFACT_BYTES: u64 = 256 * 1024;
const SNAPSHOT_BRACKET: &str =
    "fixed_finalized_anchor_genesis_and_tip_reread_by_id_and_hash_latest_tip_monotonic";

/// Fixed owner-private Taker runtime artifact.
pub const M4_TAG13_TAKER_RUNTIME_FILE: &str = "taker-runtime.json";
/// Fixed owner-private Maker runtime artifact.
pub const M4_TAG13_MAKER_RUNTIME_FILE: &str = "maker-runtime.json";
/// Fixed owner-private activated-terms artifact.
pub const M4_TAG13_TERMS_FILE: &str = "terms.json";
/// Fixed owner-private handoff receipt.
pub const M4_TAG13_RECEIPT_FILE: &str = "tag13-handoff-receipt.json";

/// Caller-owned identities that the tag-13 handoff must reproduce.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct M4Tag13Expectation {
    run_id: RunId,
    stage_a: Hex32,
    stage_b: Hex32,
    authenticated_transfer_program_id: Hex32,
}

impl M4Tag13Expectation {
    /// Constructs immutable identities supplied by the run orchestrator.
    #[must_use]
    pub const fn new(
        run_id: RunId,
        stage_a: Hex32,
        stage_b: Hex32,
        authenticated_transfer_program_id: Hex32,
    ) -> Self {
        Self {
            run_id,
            stage_a,
            stage_b,
            authenticated_transfer_program_id,
        }
    }
}

/// Local integrity manifest whose claims require revalidating the exact source-state inode.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct M4Tag13HandoffReceipt {
    schema: String,
    run_id: RunId,
    stage_a_agreement_wire_sha256: Hex32,
    stage_b_activation_wire_sha256: Hex32,
    authenticated_transfer_program_id: Hex32,
    state_device: u64,
    state_inode: u64,
    evidence_file: String,
    evidence_sha256: Hex32,
    reservation_file: String,
    reservation_sha256: Hex32,
    taker_runtime_file: String,
    taker_runtime_sha256: Hex32,
    maker_runtime_file: String,
    maker_runtime_sha256: Hex32,
    terms_file: String,
    terms_sha256: Hex32,
    initialization_transaction_id: TransactionId,
    funding_transaction_id: TransactionId,
    rpc_used: bool,
    submission_performed: bool,
}

/// Verified immutable inputs consumed before a bridge reads its private key or opens RPCs.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct M4Tag13BridgeInputs {
    runtime: RuntimeDescriptor,
    terms: XmrNativeEscrowTermsV3,
}

impl M4Tag13BridgeInputs {
    /// Returns the exact Taker runtime bound by finalized tag-13 state.
    pub const fn runtime(&self) -> &RuntimeDescriptor {
        &self.runtime
    }

    /// Returns the exact activated terms bound by finalized tag-13 state.
    pub const fn terms(&self) -> XmrNativeEscrowTermsV3 {
        self.terms
    }
}

/// Fail-closed, path-redacted handoff failures.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum M4Tag13HandoffError {
    /// State, receipt, output, or artifact storage is unsafe or malformed.
    #[error("tag-13 handoff state or artifact storage is unsafe")]
    UnsafeState,
    /// A live process owns the state directory.
    #[error("tag-13 state directory is already in use")]
    StateInUse,
    /// The exact reservation is absent.
    #[error("tag-13 native-XMR reservation is absent")]
    MissingReservation,
    /// Receipt was supplied without fixed tag-13 state, or omitted when state exists.
    #[error("tag-13 receipt presence does not match fixed tag-13 state")]
    ReceiptPresenceMismatch,
    /// Evidence, reservation, receipt, artifact, or caller identities disagree.
    #[error("tag-13 evidence, reservation, receipt, or artifact binding failed")]
    BindingMismatch,
    /// Reserved transactions do not satisfy the full keyless planner predicate.
    #[error("tag-13 reserved transaction validation failed")]
    InvalidReservation,
    /// The output directory is nonempty or a fixed artifact already exists.
    #[error("tag-13 handoff output is not empty")]
    OutputCollision,
}

#[allow(clippy::struct_field_names)]
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Effect {
    effect: XmrNativeEffectV3,
    transaction_id: TransactionId,
    submission_outcome: SubmissionOutcome,
    finalized_clock: ChainClock,
    scanned_window: DiscoveryWindow,
    containing_block_id: u64,
    containing_block_hash: Hex32,
    transaction_index: u32,
    classifier_calls: u32,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
enum Presence {
    Present,
    AbsentDefaultNonce,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AccountNonce {
    account_id: Hex32,
    nonce: u128,
    presence: Presence,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Snapshot {
    finalized_clock: ChainClock,
    genesis_block_hash: Hex32,
    maker_owner: AccountNonce,
    taker_owner: AccountNonce,
    claim_authority: AccountNonce,
    refund_authority: AccountNonce,
    bracket: String,
}

#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Evidence {
    schema: String,
    role: Participant,
    run_id: RunId,
    prepare_request_id: RequestId,
    runtime: RuntimeDescriptor,
    terms: XmrNativeEscrowTermsV3,
    stage_a_agreement_wire_sha256: Hex32,
    stage_b_activation_wire_sha256: Hex32,
    finalized_nonce_snapshot: Snapshot,
    maker_xmr_funding_cutoff_ms: u64,
    cutoff_authority: String,
    agreement_source: String,
    activation_source: String,
    claim_message_hash_source: String,
    refund_message_hash_source: String,
    punish_message_hash_source: String,
    initialization: Effect,
    funding: Effect,
    execution_scope: String,
    funding_barrier: String,
    public_rpc_used: bool,
    automatic_submission_retry: bool,
    send_attempt_ceiling_per_effect_per_process: u8,
    finality_polling_is_submission_retry: bool,
    crash_atomic_submission: bool,
    public_stage_a_input_snapshot_durably_journaled: bool,
    recovery_limitation: String,
    monero_lock_observed: bool,
    swap_completed: bool,
    atomic_swap_proven: bool,
    atomicity_claim: String,
}

struct VerifiedState {
    taker_runtime: RuntimeDescriptor,
    maker_runtime: RuntimeDescriptor,
    terms: XmrNativeEscrowTermsV3,
    evidence_sha256: Hex32,
    reservation_sha256: Hex32,
    state_device: u64,
    state_inode: u64,
    initialization_transaction_id: TransactionId,
    funding_transaction_id: TransactionId,
}

/// Reports whether the leased directory contains any fixed tag-13 state.
///
/// # Errors
///
/// Rejects unsafe directory entries or a lease that no longer names its original inode.
pub fn m4_tag13_state_present(lease: &StateDirectoryLease) -> Result<bool, M4Tag13HandoffError> {
    let store = store_for_lease(lease)?;
    let evidence = store
        .contains_fixed_file(EVIDENCE_FILE)
        .map_err(map_state)?;
    let reservation = store
        .contains_fixed_file(RESERVATION_FILE)
        .map_err(map_state)?;
    Ok(evidence || reservation)
}

/// Verifies exact finalized state and creates all four owner-private artifacts.
///
/// No private key, RPC client, planner, or effect submitter is constructed. The
/// state lease remains held through every file and output-directory fsync.
///
/// # Errors
///
/// Rejects unsafe or concurrent state, any semantic or exact-byte drift,
/// aliased/nonempty output, and partial or colliding fixed output files.
pub fn export_m4_tag13_handoff(
    state: impl AsRef<Path>,
    output: impl AsRef<Path>,
    expected: &M4Tag13Expectation,
) -> Result<M4Tag13HandoffReceipt, M4Tag13HandoffError> {
    let lease = StateDirectoryLease::acquire(state).map_err(map_lease)?;
    let verified = verify_state_under_lease(&lease, expected)?;
    let output = DurableReservationStore::open(output.as_ref()).map_err(map_output)?;
    if directories_overlap(lease.state_path(), output.path())
        || output.identity() == (verified.state_device, verified.state_inode)
    {
        return Err(M4Tag13HandoffError::UnsafeState);
    }

    let taker_runtime = canonical_json(&verified.taker_runtime)?;
    let maker_runtime = canonical_json(&verified.maker_runtime)?;
    let terms = canonical_json(&verified.terms)?;
    let receipt = M4Tag13HandoffReceipt {
        schema: RECEIPT_SCHEMA.to_owned(),
        run_id: expected.run_id.clone(),
        stage_a_agreement_wire_sha256: expected.stage_a,
        stage_b_activation_wire_sha256: expected.stage_b,
        authenticated_transfer_program_id: expected.authenticated_transfer_program_id,
        state_device: verified.state_device,
        state_inode: verified.state_inode,
        evidence_file: EVIDENCE_FILE.to_owned(),
        evidence_sha256: verified.evidence_sha256,
        reservation_file: RESERVATION_FILE.to_owned(),
        reservation_sha256: verified.reservation_sha256,
        taker_runtime_file: M4_TAG13_TAKER_RUNTIME_FILE.to_owned(),
        taker_runtime_sha256: sha256(&taker_runtime),
        maker_runtime_file: M4_TAG13_MAKER_RUNTIME_FILE.to_owned(),
        maker_runtime_sha256: sha256(&maker_runtime),
        terms_file: M4_TAG13_TERMS_FILE.to_owned(),
        terms_sha256: sha256(&terms),
        initialization_transaction_id: verified.initialization_transaction_id,
        funding_transaction_id: verified.funding_transaction_id,
        rpc_used: false,
        submission_performed: false,
    };
    let receipt_bytes = canonical_json(&receipt)?;
    output
        .create_fixed_file_set(&[
            (M4_TAG13_TAKER_RUNTIME_FILE, taker_runtime.as_slice()),
            (M4_TAG13_MAKER_RUNTIME_FILE, maker_runtime.as_slice()),
            (M4_TAG13_TERMS_FILE, terms.as_slice()),
            (M4_TAG13_RECEIPT_FILE, receipt_bytes.as_slice()),
        ])
        .map_err(map_output)?;
    drop(lease);
    Ok(receipt)
}

/// Revalidates a receipt and every fixed input while the bridge holds the state lease.
///
/// The runtime path and receipt path must name their fixed owner-private files in
/// one secure artifact directory. No secret, RPC, planner, request store, or
/// server is touched.
///
/// # Errors
///
/// Rejects absent tag-13 state, path aliases, receipt/caller drift, any state or
/// artifact mutation, and malformed public values.
pub fn verify_m4_tag13_bridge_handoff(
    lease: &StateDirectoryLease,
    receipt_path: impl AsRef<Path>,
    runtime_path: impl AsRef<Path>,
    expected_run_id: &RunId,
    expected_authenticated_transfer_program_id: Hex32,
) -> Result<M4Tag13BridgeInputs, M4Tag13HandoffError> {
    if !m4_tag13_state_present(lease)? {
        return Err(M4Tag13HandoffError::ReceiptPresenceMismatch);
    }
    let receipt_parent = fixed_parent(receipt_path.as_ref(), M4_TAG13_RECEIPT_FILE)?;
    let runtime_parent = fixed_parent(runtime_path.as_ref(), M4_TAG13_TAKER_RUNTIME_FILE)?;
    let artifacts = DurableReservationStore::open(receipt_parent).map_err(map_state)?;
    let runtime_artifacts = DurableReservationStore::open(runtime_parent).map_err(map_state)?;
    if artifacts.identity() != runtime_artifacts.identity()
        || artifacts.identity() == lease.state_identity()
    {
        return Err(M4Tag13HandoffError::UnsafeState);
    }

    let receipt_bytes = artifacts
        .read_fixed_file(M4_TAG13_RECEIPT_FILE, MAX_ARTIFACT_BYTES)
        .map_err(map_state)?;
    let receipt: M4Tag13HandoffReceipt =
        serde_json::from_slice(&receipt_bytes).map_err(|_| M4Tag13HandoffError::UnsafeState)?;
    validate_receipt_shape(&receipt)?;
    if &receipt.run_id != expected_run_id
        || receipt.authenticated_transfer_program_id != expected_authenticated_transfer_program_id
    {
        return Err(M4Tag13HandoffError::BindingMismatch);
    }
    let expected = M4Tag13Expectation::new(
        receipt.run_id.clone(),
        receipt.stage_a_agreement_wire_sha256,
        receipt.stage_b_activation_wire_sha256,
        receipt.authenticated_transfer_program_id,
    );
    let verified = verify_state_under_lease(lease, &expected)?;
    if receipt.state_device != verified.state_device
        || receipt.state_inode != verified.state_inode
        || receipt.evidence_sha256 != verified.evidence_sha256
        || receipt.reservation_sha256 != verified.reservation_sha256
        || receipt.initialization_transaction_id != verified.initialization_transaction_id
        || receipt.funding_transaction_id != verified.funding_transaction_id
    {
        return Err(M4Tag13HandoffError::BindingMismatch);
    }

    let taker_bytes = artifacts
        .read_fixed_file(M4_TAG13_TAKER_RUNTIME_FILE, MAX_ARTIFACT_BYTES)
        .map_err(map_state)?;
    let maker_bytes = artifacts
        .read_fixed_file(M4_TAG13_MAKER_RUNTIME_FILE, MAX_ARTIFACT_BYTES)
        .map_err(map_state)?;
    let terms_bytes = artifacts
        .read_fixed_file(M4_TAG13_TERMS_FILE, MAX_ARTIFACT_BYTES)
        .map_err(map_state)?;
    if sha256(&taker_bytes) != receipt.taker_runtime_sha256
        || sha256(&maker_bytes) != receipt.maker_runtime_sha256
        || sha256(&terms_bytes) != receipt.terms_sha256
    {
        return Err(M4Tag13HandoffError::BindingMismatch);
    }
    let taker_runtime: RuntimeDescriptor =
        serde_json::from_slice(&taker_bytes).map_err(|_| M4Tag13HandoffError::UnsafeState)?;
    let maker_runtime: RuntimeDescriptor =
        serde_json::from_slice(&maker_bytes).map_err(|_| M4Tag13HandoffError::UnsafeState)?;
    let terms: XmrNativeEscrowTermsV3 =
        serde_json::from_slice(&terms_bytes).map_err(|_| M4Tag13HandoffError::UnsafeState)?;
    if taker_runtime != verified.taker_runtime
        || maker_runtime != verified.maker_runtime
        || terms != verified.terms
    {
        return Err(M4Tag13HandoffError::BindingMismatch);
    }
    Ok(M4Tag13BridgeInputs {
        runtime: taker_runtime,
        terms,
    })
}

fn directories_overlap(left: &Path, right: &Path) -> bool {
    left.starts_with(right) || right.starts_with(left)
}

fn fixed_parent<'a>(
    path: &'a Path,
    expected_filename: &str,
) -> Result<&'a Path, M4Tag13HandoffError> {
    if path.file_name() != Some(OsStr::new(expected_filename)) {
        return Err(M4Tag13HandoffError::UnsafeState);
    }
    path.parent().ok_or(M4Tag13HandoffError::UnsafeState)
}

fn store_for_lease(
    lease: &StateDirectoryLease,
) -> Result<DurableReservationStore, M4Tag13HandoffError> {
    let store = DurableReservationStore::open(lease.state_path()).map_err(map_state)?;
    if store.identity() != lease.state_identity() {
        return Err(M4Tag13HandoffError::UnsafeState);
    }
    Ok(store)
}

fn verify_state_under_lease(
    lease: &StateDirectoryLease,
    expected: &M4Tag13Expectation,
) -> Result<VerifiedState, M4Tag13HandoffError> {
    let store = store_for_lease(lease)?;
    let evidence_bytes = store
        .read_fixed_file(EVIDENCE_FILE, MAX_EVIDENCE_BYTES)
        .map_err(map_state)?;
    let reservation_bytes = store
        .read_fixed_file(RESERVATION_FILE, MAX_RESERVATION_BYTES)
        .map_err(map_state)?;
    let evidence: Evidence =
        serde_json::from_slice(&evidence_bytes).map_err(|_| M4Tag13HandoffError::UnsafeState)?;
    let (request, result) = store
        .load::<PrepareNativeXmrEscrowV3Request, PrepareNativeXmrEscrowV3Result>(
            ReservationKind::XmrNativeEscrowV3,
        )
        .map_err(map_state)?
        .ok_or(M4Tag13HandoffError::MissingReservation)?;
    validate_evidence(&evidence, expected)?;
    validate_reservation(&evidence, expected, &request, &result)?;
    let maker_runtime = maker_runtime(&evidence)?;
    let (state_device, state_inode) = store.identity();
    Ok(VerifiedState {
        taker_runtime: evidence.runtime,
        maker_runtime,
        terms: evidence.terms,
        evidence_sha256: sha256(&evidence_bytes),
        reservation_sha256: sha256(&reservation_bytes),
        state_device,
        state_inode,
        initialization_transaction_id: result.initialization.transaction_id,
        funding_transaction_id: result.funding.transaction_id,
    })
}

fn maker_runtime(evidence: &Evidence) -> Result<RuntimeDescriptor, M4Tag13HandoffError> {
    let terms = evidence.terms.to_input();
    let runtime = RuntimeDescriptor::new(
        Participant::Maker,
        evidence.runtime.compatibility,
        evidence.runtime.chain_id,
        evidence.runtime.channel_id,
        evidence.runtime.genesis_block_hash,
        evidence.runtime.escrow_program_id,
        terms.claimant_account_id,
    );
    let context = MessageContext::new(
        evidence.run_id.clone(),
        evidence.prepare_request_id.clone(),
        Participant::Maker,
    );
    evidence
        .terms
        .validate_runtime_binding(&context, &runtime)
        .map_err(|_| M4Tag13HandoffError::BindingMismatch)?;
    Ok(runtime)
}

fn canonical_json<T: Serialize>(value: &T) -> Result<Vec<u8>, M4Tag13HandoffError> {
    let mut bytes =
        serde_json::to_vec_pretty(value).map_err(|_| M4Tag13HandoffError::UnsafeState)?;
    bytes.push(b'\n');
    Ok(bytes)
}

fn sha256(bytes: &[u8]) -> Hex32 {
    Hex32::from_bytes(Sha256::digest(bytes).into())
}

fn map_lease(error: StateDirectoryLeaseError) -> M4Tag13HandoffError {
    match error {
        StateDirectoryLeaseError::AlreadyHeld => M4Tag13HandoffError::StateInUse,
        StateDirectoryLeaseError::UnsafeState | StateDirectoryLeaseError::Filesystem => {
            M4Tag13HandoffError::UnsafeState
        }
    }
}

fn map_state(_: DurableReservationError) -> M4Tag13HandoffError {
    M4Tag13HandoffError::UnsafeState
}

fn map_output(error: DurableReservationError) -> M4Tag13HandoffError {
    match error {
        DurableReservationError::AlreadyReserved | DurableReservationError::PartialReservation => {
            M4Tag13HandoffError::OutputCollision
        }
        _ => M4Tag13HandoffError::UnsafeState,
    }
}

fn validate_receipt_shape(receipt: &M4Tag13HandoffReceipt) -> Result<(), M4Tag13HandoffError> {
    if receipt.schema != RECEIPT_SCHEMA
        || receipt.state_device == 0
        || receipt.state_inode == 0
        || receipt.evidence_file != EVIDENCE_FILE
        || receipt.reservation_file != RESERVATION_FILE
        || receipt.taker_runtime_file != M4_TAG13_TAKER_RUNTIME_FILE
        || receipt.maker_runtime_file != M4_TAG13_MAKER_RUNTIME_FILE
        || receipt.terms_file != M4_TAG13_TERMS_FILE
        || receipt.authenticated_transfer_program_id == Hex32::from_bytes([0; 32])
        || receipt.rpc_used
        || receipt.submission_performed
    {
        return Err(M4Tag13HandoffError::BindingMismatch);
    }
    Ok(())
}

#[allow(
    clippy::too_many_lines,
    reason = "one auditable exact-v2 evidence predicate"
)]
fn validate_evidence(
    evidence: &Evidence,
    expected: &M4Tag13Expectation,
) -> Result<(), M4Tag13HandoffError> {
    let terms = evidence.terms.to_input();
    let snapshot = &evidence.finalized_nonce_snapshot;
    let initialization = &evidence.initialization;
    let funding = &evidence.funding;
    let checked = Hex32::from_hex(CHECKED_M4_ESCROW_PROGRAM_ID_HEX)
        .map_err(|_| M4Tag13HandoffError::BindingMismatch)?;
    if evidence.schema != EVIDENCE_SCHEMA
        || evidence.role != Participant::Taker
        || evidence.run_id != expected.run_id
        || evidence.stage_a_agreement_wire_sha256 != expected.stage_a
        || evidence.stage_b_activation_wire_sha256 != expected.stage_b
        || evidence.runtime.sidecar_role != Participant::Taker
        || evidence.runtime.compatibility != RuntimeCompatibility::LeeV0_2_0
        || evidence.runtime.chain_id != evidence.runtime.channel_id
        || evidence.runtime.chain_id == Hex32::from_bytes([0; 32])
        || evidence.runtime.genesis_block_hash == Hex32::from_bytes([0; 32])
        || evidence.runtime.escrow_program_id != checked
        || evidence.runtime.escrow_program_id != terms.escrow_program_id
        || evidence.runtime.signer_account_id != terms.depositor_account_id
        || terms.authenticated_transfer_program_id != expected.authenticated_transfer_program_id
        || terms.depositor != Participant::Taker
        || terms.claimant != Participant::Maker
        || snapshot.genesis_block_hash != evidence.runtime.genesis_block_hash
        || snapshot.taker_owner.account_id != evidence.runtime.signer_account_id
        || snapshot.taker_owner.presence != Presence::Present
        || snapshot.maker_owner.account_id != terms.claimant_account_id
        || snapshot.maker_owner.presence != Presence::Present
        || snapshot.claim_authority.account_id != terms.claim_authority_account_id
        || snapshot.refund_authority.account_id != terms.refund_authority_account_id
        || !valid_account_nonce(&snapshot.claim_authority)
        || !valid_account_nonce(&snapshot.refund_authority)
        || snapshot.taker_owner.nonce.checked_add(1).is_none()
        || snapshot.maker_owner.nonce == u128::MAX
        || snapshot.claim_authority.nonce == u128::MAX
        || snapshot.refund_authority.nonce == u128::MAX
        || snapshot.bracket != SNAPSHOT_BRACKET
        || snapshot.finalized_clock.height >= initialization.finalized_clock.height
        || initialization.finalized_clock.height >= funding.finalized_clock.height
        || snapshot.finalized_clock.timestamp_ms > initialization.finalized_clock.timestamp_ms
        || initialization.finalized_clock.timestamp_ms > funding.finalized_clock.timestamp_ms
        || funding.finalized_clock.timestamp_ms > evidence.maker_xmr_funding_cutoff_ms
        || evidence.maker_xmr_funding_cutoff_ms >= terms.refund_at_ms
        || terms.refund_at_ms >= terms.punish_at_ms
        || initialization.effect != XmrNativeEffectV3::Initialize
        || funding.effect != XmrNativeEffectV3::Fund
        || initialization.transaction_id == funding.transaction_id
        || !effect_final(initialization)
        || !effect_final(funding)
        || evidence.maker_xmr_funding_cutoff_ms == 0
        || evidence.cutoff_authority
            != "signed_stage_a_agreement_and_stable_finalized_lez_consensus_timestamps"
        || evidence.agreement_source != "canonical_validated_stage_a_wire"
        || evidence.activation_source
            != "canonical_validated_stage_b_wire_and_owner_private_view_key"
        || evidence.claim_message_hash_source != "generated_official_tag15_message"
        || evidence.refund_message_hash_source != "generated_official_tag16_message"
        || evidence.punish_message_hash_source != "generated_official_tag17_message"
        || evidence.execution_scope != "m4_tag13_lez_initialize_and_fund_only"
        || evidence.funding_barrier
            != "exact_initialize_found_and_initialize_and_fund_finalized_at_or_before_signed_maker_xmr_funding_cutoff"
        || evidence.public_rpc_used
        || evidence.automatic_submission_retry
        || evidence.send_attempt_ceiling_per_effect_per_process != 1
        || evidence.finality_polling_is_submission_retry
        || evidence.crash_atomic_submission
        || evidence.public_stage_a_input_snapshot_durably_journaled
        || evidence.recovery_limitation
            != "wire_files_and_raw_finalized_nonce_provenance_require_operator_reentry_after_a_crash"
        || evidence.monero_lock_observed
        || evidence.swap_completed
        || evidence.atomic_swap_proven
        || evidence.atomicity_claim != "none_tag13_only_proves_ordered_finalized_lez_escrow_funding"
    {
        return Err(M4Tag13HandoffError::BindingMismatch);
    }
    Ok(())
}

fn valid_account_nonce(account: &AccountNonce) -> bool {
    match account.presence {
        Presence::Present => true,
        Presence::AbsentDefaultNonce => account.nonce == 0,
    }
}

fn effect_final(effect: &Effect) -> bool {
    let end = effect.scanned_window.start_height().checked_add(u64::from(
        effect.scanned_window.max_blocks().saturating_sub(1),
    ));
    effect.classifier_calls > 0
        && effect.containing_block_id == effect.finalized_clock.height
        && effect.containing_block_hash == effect.finalized_clock.block_hash
        && end.is_some_and(|end| {
            effect.containing_block_id >= effect.scanned_window.start_height()
                && effect.containing_block_id <= end
        })
        && matches!(
            effect.submission_outcome,
            SubmissionOutcome::Accepted | SubmissionOutcome::AlreadyKnown
        )
        && effect.transaction_index < u32::MAX
}

fn validate_reservation(
    evidence: &Evidence,
    expected: &M4Tag13Expectation,
    request: &PrepareNativeXmrEscrowV3Request,
    result: &PrepareNativeXmrEscrowV3Result,
) -> Result<(), M4Tag13HandoffError> {
    if request.context.run_id != evidence.run_id
        || request.context.request_id != evidence.prepare_request_id
        || request.context.sidecar_role != Participant::Taker
        || request.runtime != evidence.runtime
        || request.terms != evidence.terms
        || result.context != request.context
        || result.terms != request.terms
        || result.initialization.transaction_id != evidence.initialization.transaction_id
        || result.funding.transaction_id != evidence.funding.transaction_id
    {
        return Err(M4Tag13HandoffError::BindingMismatch);
    }
    validate_prepared_native_xmr_escrow_v3_public(
        &evidence.runtime,
        program_id_from_hex(expected.authenticated_transfer_program_id),
        request,
        result,
        Some(evidence.finalized_nonce_snapshot.taker_owner.nonce),
    )
    .map_err(|_| M4Tag13HandoffError::InvalidReservation)
}
#[cfg(test)]
mod tests {
    #![allow(
        clippy::large_futures,
        reason = "test-only exact planner fixtures retain key and transaction values across await"
    )]
    use super::{
        EVIDENCE_FILE, M4_TAG13_RECEIPT_FILE, M4_TAG13_TAKER_RUNTIME_FILE, M4_TAG13_TERMS_FILE,
        M4Tag13Expectation, M4Tag13HandoffError, export_m4_tag13_handoff,
        verify_m4_tag13_bridge_handoff,
    };
    use crate::{
        CHECKED_M4_ESCROW_PROGRAM_ID, NativeEscrowPlanner, NativePrepareError, NonceSource,
        StateDirectoryLease, compute_custody_pda, compute_metadata_pda, program_id_to_hex,
    };
    use async_trait::async_trait;
    use lez_bridge_protocol::{
        DiscoveryWindow, Hex32, MessageContext, Participant, PrepareNativeXmrEscrowV3Request,
        PrepareNativeXmrEscrowV3Result, RequestId, RunId, RuntimeCompatibility, RuntimeDescriptor,
        XmrNativeEscrowTermsV3, XmrNativeEscrowTermsV3Input,
    };
    use nssa::{AccountId, PrivateKey, PublicKey};
    use serde_json::{Value, json};
    use std::{
        fs,
        os::unix::fs::{PermissionsExt as _, symlink},
        path::Path,
        sync::Arc,
    };
    use tempfile::TempDir;

    const TRANSFER_PROGRAM: [u32; 8] = [0x5060_7080; 8];
    const SWAP_ID: [u8; 32] = [51; 32];

    #[derive(Debug)]
    struct FixedNonce(u128);

    #[async_trait]
    impl NonceSource for FixedNonce {
        async fn account_nonce(&self, _: AccountId) -> Result<u128, NativePrepareError> {
            Ok(self.0)
        }
    }

    struct Fixture {
        state: TempDir,
        output: TempDir,
        expected: M4Tag13Expectation,
        run: RunId,
        runtime: RuntimeDescriptor,
        terms: XmrNativeEscrowTermsV3,
    }

    const fn h(byte: u8) -> Hex32 {
        Hex32::from_bytes([byte; 32])
    }

    fn actor(byte: u8) -> (AccountId, PrivateKey, PublicKey) {
        let key = PrivateKey::try_new([byte; 32]).expect("key");
        let public = PublicKey::new_from_private_key(&key);
        (AccountId::from(&public), key, public)
    }

    fn private_dir() -> TempDir {
        let directory = TempDir::new().expect("temporary directory");
        fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700))
            .expect("owner-only directory");
        directory
    }

    fn make_terms(
        taker: AccountId,
        maker: AccountId,
        claim_authority: AccountId,
        claim_key: &PublicKey,
        refund_authority: AccountId,
        refund_key: &PublicKey,
        amount: u128,
    ) -> XmrNativeEscrowTermsV3 {
        XmrNativeEscrowTermsV3::new(XmrNativeEscrowTermsV3Input {
            swap_id: h(51),
            activation_commitment: h(2),
            escrow_program_id: program_id_to_hex(CHECKED_M4_ESCROW_PROGRAM_ID),
            authenticated_transfer_program_id: program_id_to_hex(TRANSFER_PROGRAM),
            metadata_account_id: Hex32::from_bytes(
                compute_metadata_pda(&CHECKED_M4_ESCROW_PROGRAM_ID, &SWAP_ID).into_value(),
            ),
            custody_account_id: Hex32::from_bytes(
                compute_custody_pda(&CHECKED_M4_ESCROW_PROGRAM_ID, &SWAP_ID).into_value(),
            ),
            depositor: Participant::Taker,
            depositor_account_id: Hex32::from_bytes(taker.into_value()),
            claimant: Participant::Maker,
            claimant_account_id: Hex32::from_bytes(maker.into_value()),
            claim_aggregate_x_only_public_key: Hex32::from_bytes(*claim_key.value()),
            claim_authority_account_id: Hex32::from_bytes(claim_authority.into_value()),
            refund_aggregate_x_only_public_key: Hex32::from_bytes(*refund_key.value()),
            refund_authority_account_id: Hex32::from_bytes(refund_authority.into_value()),
            maker_dleq_transcript_commitment: h(13),
            taker_dleq_transcript_commitment: h(14),
            claim_partial_context_binding: h(15),
            claim_partial_commitment: h(16),
            amount,
            refund_at_ms: 20_000,
            punish_at_ms: 30_000,
            claim_message_hash: h(17),
            refund_message_hash: h(18),
            punish_message_hash: h(19),
        })
        .expect("valid terms")
    }

    fn write_private(path: &Path, bytes: &[u8]) {
        fs::write(path, bytes).expect("write private file");
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).expect("owner-only file");
    }

    fn write_value(path: &Path, value: &Value) {
        write_private(
            path,
            &serde_json::to_vec_pretty(value).expect("serialize JSON"),
        );
    }

    #[allow(
        clippy::too_many_lines,
        reason = "one fixture visibly assembles the complete strict tag-13 evidence schema"
    )]
    async fn build_fixture(initial_nonce: u128, signed_wrong_message: bool) -> Fixture {
        let (taker, taker_key, _) = actor(21);
        let (maker, _, _) = actor(22);
        let (claim_authority, _, claim_key) = actor(23);
        let (refund_authority, _, refund_key) = actor(24);
        let runtime = RuntimeDescriptor::new(
            Participant::Taker,
            RuntimeCompatibility::LeeV0_2_0,
            h(40),
            h(40),
            h(42),
            program_id_to_hex(CHECKED_M4_ESCROW_PROGRAM_ID),
            Hex32::from_bytes(taker.into_value()),
        );
        let terms = make_terms(
            taker,
            maker,
            claim_authority,
            &claim_key,
            refund_authority,
            &refund_key,
            75,
        );
        let run = RunId::new("m4-tag13-handoff-test").expect("run");
        let request_id = RequestId::new("m4-tag13-prepare-test").expect("request");
        let request = PrepareNativeXmrEscrowV3Request::new(
            MessageContext::new(run.clone(), request_id.clone(), Participant::Taker),
            runtime.clone(),
            terms,
        );
        let planner = NativeEscrowPlanner::new(
            Participant::Taker,
            taker_key.clone(),
            CHECKED_M4_ESCROW_PROGRAM_ID,
            TRANSFER_PROGRAM,
            runtime.clone(),
            Arc::new(FixedNonce(initial_nonce)),
        )
        .expect("planner");
        let mut prepared = planner
            .prepare_native_xmr_escrow_v3(&request)
            .await
            .expect("prepare");
        if signed_wrong_message {
            let alternate_terms = make_terms(
                taker,
                maker,
                claim_authority,
                &claim_key,
                refund_authority,
                &refund_key,
                76,
            );
            let alternate_request = PrepareNativeXmrEscrowV3Request::new(
                request.context.clone(),
                runtime.clone(),
                alternate_terms,
            );
            let alternate = NativeEscrowPlanner::new(
                Participant::Taker,
                taker_key,
                CHECKED_M4_ESCROW_PROGRAM_ID,
                TRANSFER_PROGRAM,
                runtime.clone(),
                Arc::new(FixedNonce(initial_nonce)),
            )
            .expect("alternate planner")
            .prepare_native_xmr_escrow_v3(&alternate_request)
            .await
            .expect("alternate prepare");
            prepared = PrepareNativeXmrEscrowV3Result::new(
                request.context.clone(),
                request.terms,
                alternate.initialization,
                alternate.funding,
            );
        }

        let state = private_dir();
        let output = private_dir();
        write_value(
            &state.path().join("xmr-native-escrow-reservation.v3.json"),
            &json!({
                "schema_version": 1,
                "kind": "xmr_native_escrow_v3",
                "request": request,
                "result": prepared,
            }),
        );
        let evidence = json!({
            "schema":"lez_v02_m4_xmr_stage_a_tag13_poc_v2","role":"taker",
            "run_id":run,"prepare_request_id":request_id,"runtime":runtime,"terms":terms,
            "stage_a_agreement_wire_sha256":h(31),"stage_b_activation_wire_sha256":h(32),
            "finalized_nonce_snapshot":{
                "finalized_clock":{"block_hash":h(60),"height":19,"timestamp_ms":7_000},
                "genesis_block_hash":h(42),
                "maker_owner":{"account_id":Hex32::from_bytes(maker.into_value()),"nonce":3,"presence":"present"},
                "taker_owner":{"account_id":Hex32::from_bytes(taker.into_value()),"nonce":7,"presence":"present"},
                "claim_authority":{"account_id":Hex32::from_bytes(claim_authority.into_value()),"nonce":0,"presence":"absent_default_nonce"},
                "refund_authority":{"account_id":Hex32::from_bytes(refund_authority.into_value()),"nonce":0,"presence":"absent_default_nonce"},
                "bracket":"fixed_finalized_anchor_genesis_and_tip_reread_by_id_and_hash_latest_tip_monotonic"
            },
            "maker_xmr_funding_cutoff_ms":10_000,
            "cutoff_authority":"signed_stage_a_agreement_and_stable_finalized_lez_consensus_timestamps",
            "agreement_source":"canonical_validated_stage_a_wire",
            "activation_source":"canonical_validated_stage_b_wire_and_owner_private_view_key",
            "claim_message_hash_source":"generated_official_tag15_message",
            "refund_message_hash_source":"generated_official_tag16_message",
            "punish_message_hash_source":"generated_official_tag17_message",
            "initialization":{"effect":"initialize","transaction_id":prepared.initialization.transaction_id,
                "submission_outcome":"accepted",
                "finalized_clock":{"block_hash":h(61),"height":20,"timestamp_ms":8_000},
                "scanned_window":DiscoveryWindow::new(20,1).expect("window"),
                "containing_block_id":20,"containing_block_hash":h(61),"transaction_index":0,"classifier_calls":1},
            "funding":{"effect":"fund","transaction_id":prepared.funding.transaction_id,
                "submission_outcome":"accepted",
                "finalized_clock":{"block_hash":h(62),"height":21,"timestamp_ms":9_000},
                "scanned_window":DiscoveryWindow::new(21,1).expect("window"),
                "containing_block_id":21,"containing_block_hash":h(62),"transaction_index":0,"classifier_calls":1},
            "execution_scope":"m4_tag13_lez_initialize_and_fund_only",
            "funding_barrier":"exact_initialize_found_and_initialize_and_fund_finalized_at_or_before_signed_maker_xmr_funding_cutoff",
            "public_rpc_used":false,"automatic_submission_retry":false,
            "send_attempt_ceiling_per_effect_per_process":1,"finality_polling_is_submission_retry":false,
            "crash_atomic_submission":false,"public_stage_a_input_snapshot_durably_journaled":false,
            "recovery_limitation":"wire_files_and_raw_finalized_nonce_provenance_require_operator_reentry_after_a_crash",
            "monero_lock_observed":false,"swap_completed":false,"atomic_swap_proven":false,
            "atomicity_claim":"none_tag13_only_proves_ordered_finalized_lez_escrow_funding"
        });
        write_value(&state.path().join(EVIDENCE_FILE), &evidence);
        Fixture {
            state,
            output,
            expected: M4Tag13Expectation::new(
                RunId::new("m4-tag13-handoff-test").expect("run"),
                h(31),
                h(32),
                program_id_to_hex(TRANSFER_PROGRAM),
            ),
            run: RunId::new("m4-tag13-handoff-test").expect("run"),
            runtime,
            terms,
        }
    }

    fn mutate_json(path: &Path, mutate: impl FnOnce(&mut Value)) {
        let mut value: Value =
            serde_json::from_slice(&fs::read(path).expect("read JSON")).expect("parse JSON");
        mutate(&mut value);
        write_value(path, &value);
    }

    #[test]
    fn receipt_with_absent_fixed_tag13_state_fails_before_artifact_reads() {
        let state = private_dir();
        let output = private_dir();
        let lease = StateDirectoryLease::acquire(state.path()).expect("lease");
        assert_eq!(
            verify_m4_tag13_bridge_handoff(
                &lease,
                output.path().join(M4_TAG13_RECEIPT_FILE),
                output.path().join(M4_TAG13_TAKER_RUNTIME_FILE),
                &RunId::new("m4-tag13-handoff-test").expect("run"),
                program_id_to_hex(TRANSFER_PROGRAM),
            )
            .expect_err("receipt cannot introduce absent tag-13 state"),
            M4Tag13HandoffError::ReceiptPresenceMismatch
        );
    }

    #[tokio::test]
    async fn exact_state_exports_both_roles_and_bridge_revalidates_every_artifact() {
        let fixture = build_fixture(7, false).await;
        export_m4_tag13_handoff(
            fixture.state.path(),
            fixture.output.path(),
            &fixture.expected,
        )
        .expect("export");
        let lease = StateDirectoryLease::acquire(fixture.state.path()).expect("lease");
        let verified = verify_m4_tag13_bridge_handoff(
            &lease,
            fixture.output.path().join(M4_TAG13_RECEIPT_FILE),
            fixture.output.path().join(M4_TAG13_TAKER_RUNTIME_FILE),
            &fixture.run,
            program_id_to_hex(TRANSFER_PROGRAM),
        )
        .expect("bridge verification");
        assert_eq!(verified.runtime(), &fixture.runtime);
        assert_eq!(verified.terms(), fixture.terms);
        let maker: RuntimeDescriptor = serde_json::from_slice(
            &fs::read(fixture.output.path().join("maker-runtime.json")).expect("maker runtime"),
        )
        .expect("typed maker runtime");
        assert_eq!(maker.sidecar_role, Participant::Maker);
        assert_eq!(
            maker.signer_account_id,
            fixture.terms.to_input().claimant_account_id
        );
    }

    #[tokio::test]
    async fn signed_wrong_nonce_and_signed_wrong_message_fail_the_shared_planner_predicate() {
        for fixture in [build_fixture(8, false).await, build_fixture(7, true).await] {
            assert_eq!(
                export_m4_tag13_handoff(
                    fixture.state.path(),
                    fixture.output.path(),
                    &fixture.expected,
                )
                .expect_err("invalid signed reservation"),
                M4Tag13HandoffError::InvalidReservation
            );
        }
    }

    #[tokio::test]
    async fn evidence_reservation_role_and_signer_mutations_fail_closed() {
        let fixture = build_fixture(7, false).await;
        mutate_json(&fixture.state.path().join(EVIDENCE_FILE), |value| {
            value["role"] = json!("maker");
        });
        assert!(
            export_m4_tag13_handoff(
                fixture.state.path(),
                fixture.output.path(),
                &fixture.expected,
            )
            .is_err()
        );

        let fixture = build_fixture(7, false).await;
        mutate_json(&fixture.state.path().join(EVIDENCE_FILE), |value| {
            value["runtime"]["signer_account_id"] = json!(h(99));
        });
        assert!(
            export_m4_tag13_handoff(
                fixture.state.path(),
                fixture.output.path(),
                &fixture.expected,
            )
            .is_err()
        );

        let fixture = build_fixture(7, false).await;
        write_private(
            &fixture
                .state
                .path()
                .join("xmr-native-escrow-reservation.v3.json"),
            b"{",
        );
        assert!(
            export_m4_tag13_handoff(
                fixture.state.path(),
                fixture.output.path(),
                &fixture.expected,
            )
            .is_err()
        );
    }

    #[tokio::test]
    async fn concurrent_lease_and_unsafe_or_aliased_output_are_rejected() {
        let fixture = build_fixture(7, false).await;
        let lease = StateDirectoryLease::acquire(fixture.state.path()).expect("first lease");
        assert_eq!(
            export_m4_tag13_handoff(
                fixture.state.path(),
                fixture.output.path(),
                &fixture.expected,
            )
            .expect_err("concurrent export"),
            M4Tag13HandoffError::StateInUse
        );
        drop(lease);
        assert_eq!(
            export_m4_tag13_handoff(
                fixture.state.path(),
                fixture.state.path(),
                &fixture.expected,
            )
            .expect_err("aliased output"),
            M4Tag13HandoffError::UnsafeState
        );
        let descendant = fixture.state.path().join("nested-output");
        fs::create_dir(&descendant).expect("nested output directory");
        fs::set_permissions(&descendant, fs::Permissions::from_mode(0o700))
            .expect("owner-only nested output");
        assert_eq!(
            export_m4_tag13_handoff(fixture.state.path(), &descendant, &fixture.expected,)
                .expect_err("descendant output must be rejected"),
            M4Tag13HandoffError::UnsafeState
        );

        let fixture = build_fixture(7, false).await;
        let ancestor = private_dir();
        let nested_state = ancestor.path().join("nested-state");
        fs::create_dir(&nested_state).expect("nested state directory");
        fs::set_permissions(&nested_state, fs::Permissions::from_mode(0o700))
            .expect("owner-only nested state");
        for filename in [EVIDENCE_FILE, "xmr-native-escrow-reservation.v3.json"] {
            write_private(
                &nested_state.join(filename),
                &fs::read(fixture.state.path().join(filename)).expect("source state file"),
            );
        }
        assert_eq!(
            export_m4_tag13_handoff(&nested_state, ancestor.path(), &fixture.expected,)
                .expect_err("ancestor output must be rejected"),
            M4Tag13HandoffError::UnsafeState
        );

        let fixture = build_fixture(7, false).await;
        let parent = private_dir();
        let alias = parent.path().join("alias");
        symlink(fixture.output.path(), &alias).expect("output symlink");
        assert!(export_m4_tag13_handoff(fixture.state.path(), &alias, &fixture.expected).is_err());
    }

    #[tokio::test]
    async fn output_collision_or_partial_state_creates_no_new_artifacts() {
        for name in ["occupied", ".tag13-handoff-receipt.json.partial.1"] {
            let fixture = build_fixture(7, false).await;
            write_private(&fixture.output.path().join(name), b"occupied");
            assert_eq!(
                export_m4_tag13_handoff(
                    fixture.state.path(),
                    fixture.output.path(),
                    &fixture.expected,
                )
                .expect_err("nonempty output"),
                M4Tag13HandoffError::OutputCollision
            );
            assert!(!fixture.output.path().join(M4_TAG13_RECEIPT_FILE).exists());
            assert!(
                !fixture
                    .output
                    .path()
                    .join(M4_TAG13_TAKER_RUNTIME_FILE)
                    .exists()
            );
        }
    }

    #[tokio::test]
    async fn receipt_artifact_and_post_export_state_mutations_are_detected() {
        let fixture = build_fixture(7, false).await;
        export_m4_tag13_handoff(
            fixture.state.path(),
            fixture.output.path(),
            &fixture.expected,
        )
        .expect("export");
        mutate_json(
            &fixture.output.path().join(M4_TAG13_RECEIPT_FILE),
            |value| value["state_inode"] = json!(1),
        );
        let lease = StateDirectoryLease::acquire(fixture.state.path()).expect("lease");
        assert!(
            verify_m4_tag13_bridge_handoff(
                &lease,
                fixture.output.path().join(M4_TAG13_RECEIPT_FILE),
                fixture.output.path().join(M4_TAG13_TAKER_RUNTIME_FILE),
                &fixture.run,
                program_id_to_hex(TRANSFER_PROGRAM),
            )
            .is_err()
        );
        drop(lease);

        let fixture = build_fixture(7, false).await;
        export_m4_tag13_handoff(
            fixture.state.path(),
            fixture.output.path(),
            &fixture.expected,
        )
        .expect("export");
        mutate_json(&fixture.output.path().join(M4_TAG13_TERMS_FILE), |value| {
            value["amount"] = json!("76");
        });
        let lease = StateDirectoryLease::acquire(fixture.state.path()).expect("lease");
        assert!(
            verify_m4_tag13_bridge_handoff(
                &lease,
                fixture.output.path().join(M4_TAG13_RECEIPT_FILE),
                fixture.output.path().join(M4_TAG13_TAKER_RUNTIME_FILE),
                &fixture.run,
                program_id_to_hex(TRANSFER_PROGRAM),
            )
            .is_err(),
            "otherwise-valid cross-swap terms must not satisfy the receipt"
        );
        drop(lease);

        let fixture = build_fixture(7, false).await;
        export_m4_tag13_handoff(
            fixture.state.path(),
            fixture.output.path(),
            &fixture.expected,
        )
        .expect("export");
        let runtime_path = fixture.output.path().join(M4_TAG13_TAKER_RUNTIME_FILE);
        let mut runtime = fs::read(&runtime_path).expect("runtime");
        runtime.push(b' ');
        write_private(&runtime_path, &runtime);
        let lease = StateDirectoryLease::acquire(fixture.state.path()).expect("lease");
        assert!(
            verify_m4_tag13_bridge_handoff(
                &lease,
                fixture.output.path().join(M4_TAG13_RECEIPT_FILE),
                &runtime_path,
                &fixture.run,
                program_id_to_hex(TRANSFER_PROGRAM),
            )
            .is_err()
        );
        drop(lease);

        let fixture = build_fixture(7, false).await;
        export_m4_tag13_handoff(
            fixture.state.path(),
            fixture.output.path(),
            &fixture.expected,
        )
        .expect("export");
        mutate_json(&fixture.state.path().join(EVIDENCE_FILE), |value| {
            value["maker_xmr_funding_cutoff_ms"] = json!(9_500);
        });
        let lease = StateDirectoryLease::acquire(fixture.state.path()).expect("lease");
        assert!(
            verify_m4_tag13_bridge_handoff(
                &lease,
                fixture.output.path().join(M4_TAG13_RECEIPT_FILE),
                fixture.output.path().join(M4_TAG13_TAKER_RUNTIME_FILE),
                &fixture.run,
                program_id_to_hex(TRANSFER_PROGRAM),
            )
            .is_err()
        );
    }
}
