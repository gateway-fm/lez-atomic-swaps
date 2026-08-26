#![forbid(unsafe_code)]

use std::{
    fs::{self, File, OpenOptions},
    io::{Read as _, Write as _},
    os::unix::fs::{MetadataExt as _, OpenOptionsExt as _, PermissionsExt as _},
    path::{Path, PathBuf},
    str::FromStr as _,
    sync::Arc,
    time::Duration,
};

use anyhow::{Context as _, Result, bail, ensure};
use async_trait::async_trait;
use clap::Parser;
use lez_bridge_protocol::{
    ChainClock, ClassifyFinalizedNativeXmrEffectV3Request, DiscoveryWindow,
    FinalizedNativeXmrScanOutcomeV3, FinalizedNativeXmrTransactionTargetV3,
    FinalizedNativeXmrUnavailableReasonV3, Hex32, MessageContext, Participant,
    PrepareNativeXmrEscrowV3Request, PreparedTransaction, RequestId, RunId, RuntimeCompatibility,
    RuntimeDescriptor, SubmissionOutcome, SubmitTransactionRequest, TransactionId,
    XmrNativeEffectV3, XmrNativeEscrowTermsV3, XmrNativeEscrowTermsV3Input,
};
use lez_v0_2_sidecar::{
    BridgeRuntime, CHECKED_M4_ESCROW_PROGRAM_ID_HEX, M4FinalizedAccountIds,
    M4StageAFutureMessageInput, M4StageAFutureMessagePlan, NativeEscrowPlanner, NativePrepareError,
    NonceSource, OfficialIndexerRpc, OfficialNodeRpc, StableM4FinalizedNonceSnapshot,
    compute_custody_pda, compute_metadata_pda, plan_m4_stage_a_future_messages, program_id_to_hex,
    read_stable_m4_finalized_nonce_snapshot, validate_checked_m4_escrow_program_id,
    validate_loopback_http_endpoint,
};
use lez_xmr_swap_sdk::{
    MAX_XMR_ACTIVATION_WIRE_BYTES, MAX_XMR_AGREEMENT_WIRE_BYTES, MoneroPrivateViewKey,
    XmrActivatedAgreementV1, XmrAgreementV1, XmrLezInitializePlanV1,
};
use nssa::{AccountId, PrivateKey, PublicKey};
use serde::Serialize;
use sha2::{Digest as _, Sha256};
use zeroize::Zeroizing;

const EVIDENCE_SCHEMA: &str = "lez_v02_m4_xmr_stage_a_tag13_poc_v2";
const EVIDENCE_FILENAME: &str = "m4-xmr-stage-a-tag13-evidence.v2.json";
const DEFAULT_MAX_FINALITY_SCANS: u32 = 512;
const DEFAULT_FINALITY_SCAN_INTERVAL_MS: u64 = 250;

#[cfg(test)]
static NODE_BOUNDARY_ENTRIES: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

/// Execute the role-fixed Taker's M4 tag-13 Initialize/Fund path.
#[derive(Debug, Parser)]
#[command(version, about)]
struct Arguments {
    /// Existing owner-only 0700 directory dedicated to this Taker and swap.
    #[arg(long)]
    state_directory: PathBuf,

    /// Owner-only regular file containing one lowercase-hex 32-byte Taker key.
    #[arg(long)]
    private_key_file: PathBuf,

    /// Literal-loopback official v0.2 sequencer root URL.
    #[arg(long)]
    sequencer_url: String,

    /// Literal-loopback official v0.2 finalized-indexer root URL.
    #[arg(long)]
    indexer_url: String,

    /// Canonical validated Stage-A agreement wire.
    #[arg(long)]
    agreement_wire_file: PathBuf,

    /// Canonical countersigned Stage-B activation wire.
    #[arg(long)]
    activation_wire_file: PathBuf,

    /// Owner-only file containing the lowercase-hex Monero private view key
    /// that is required to validate the Stage-B shared address.
    #[arg(long)]
    monero_view_key_file: PathBuf,

    /// Bounded local-run identity.
    #[arg(long)]
    run_id: String,

    /// Idempotency identity for the durable tag-13 preparation.
    #[arg(long)]
    prepare_request_id: String,

    /// Total bounded classifier calls per effect. Submission is never retried.
    #[arg(long, default_value_t = DEFAULT_MAX_FINALITY_SCANS)]
    max_finality_scans: u32,

    /// Delay between distinct bounded finalized-classification attempts.
    #[arg(long, default_value_t = DEFAULT_FINALITY_SCAN_INTERVAL_MS)]
    finality_scan_interval_ms: u64,
}

#[derive(Debug)]
struct ExactTakerFinalizedNonce {
    taker: AccountId,
    nonce: u128,
}

#[async_trait]
impl NonceSource for ExactTakerFinalizedNonce {
    async fn account_nonce(&self, account_id: AccountId) -> Result<u128, NativePrepareError> {
        if account_id != self.taker {
            return Err(NativePrepareError::NonceUnavailable);
        }
        Ok(self.nonce)
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(deny_unknown_fields)]
struct EffectEvidence {
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

#[allow(
    clippy::struct_excessive_bools,
    reason = "explicit negative evidence fields prevent accidental Stage-A overclaiming"
)]
#[derive(Debug, Serialize)]
#[serde(deny_unknown_fields)]
struct StageAEvidence {
    schema: &'static str,
    role: Participant,
    run_id: RunId,
    prepare_request_id: RequestId,
    runtime: RuntimeDescriptor,
    terms: XmrNativeEscrowTermsV3,
    stage_a_agreement_wire_sha256: Hex32,
    stage_b_activation_wire_sha256: Hex32,
    finalized_nonce_snapshot: StableM4FinalizedNonceSnapshot,
    maker_xmr_funding_cutoff_ms: u64,
    cutoff_authority: &'static str,
    agreement_source: &'static str,
    activation_source: &'static str,
    claim_message_hash_source: &'static str,
    refund_message_hash_source: &'static str,
    punish_message_hash_source: &'static str,
    initialization: EffectEvidence,
    funding: EffectEvidence,
    execution_scope: &'static str,
    funding_barrier: &'static str,
    public_rpc_used: bool,
    automatic_submission_retry: bool,
    send_attempt_ceiling_per_effect_per_process: u8,
    finality_polling_is_submission_retry: bool,
    crash_atomic_submission: bool,
    public_stage_a_input_snapshot_durably_journaled: bool,
    recovery_limitation: &'static str,
    monero_lock_observed: bool,
    swap_completed: bool,
    atomic_swap_proven: bool,
    atomicity_claim: &'static str,
}

#[tokio::main]
async fn main() {
    match execute(Arguments::parse()).await {
        Ok((evidence, evidence_path)) => {
            let Ok(json) = serde_json::to_string(&evidence) else {
                eprintln!("M4 tag-13 evidence encoding failed");
                std::process::exit(1);
            };
            println!("{json}");
            eprintln!("owner-only evidence written to {}", evidence_path.display());
        }
        Err(error) => {
            eprintln!("M4 tag-13 Taker failed: {error:#}");
            std::process::exit(1);
        }
    }
}

#[allow(
    clippy::too_many_lines,
    reason = "the executable keeps nonce bracketing, durable prepare, ordered submissions, and evidence assembly in one auditable flow"
)]
async fn execute(arguments: Arguments) -> Result<(StageAEvidence, PathBuf)> {
    validate_state_directory(&arguments.state_directory)?;
    ensure!(
        arguments.max_finality_scans > 0,
        "finality scan bound must be nonzero"
    );
    ensure!(
        arguments.finality_scan_interval_ms > 0,
        "finality scan interval must be nonzero"
    );

    let signer_key = read_private_key(&arguments.private_key_file)?;
    let signer_account_id = AccountId::from(&PublicKey::new_from_private_key(&signer_key));
    let run_id = RunId::new(arguments.run_id.clone()).context("invalid run ID")?;
    let prepare_request_id = RequestId::new(arguments.prepare_request_id.clone())
        .context("invalid prepare request ID")?;

    let agreement_wire = read_bounded_wire_file(
        &arguments.agreement_wire_file,
        "Stage-A agreement wire",
        MAX_XMR_AGREEMENT_WIRE_BYTES,
        false,
    )?;
    let agreement_wire_sha256 = sha256_hex(&agreement_wire);
    let agreement = XmrAgreementV1::from_wire(&agreement_wire)
        .context("Stage-A agreement wire failed canonical validation")?;
    let monero_view_key = read_monero_view_key(&arguments.monero_view_key_file)?;
    let activation_wire = read_bounded_wire_file(
        &arguments.activation_wire_file,
        "Stage-B activation wire",
        MAX_XMR_ACTIVATION_WIRE_BYTES,
        true,
    )?;
    let activation_wire_sha256 = sha256_hex(&activation_wire);
    let activation =
        XmrActivatedAgreementV1::from_wire(&agreement, &activation_wire, &monero_view_key)
            .context("Stage-B activation wire failed canonical validation")?;
    let activated_plan = activation
        .lez_initialize_plan(&agreement)
        .context("Stage-B activation does not bind the Stage-A agreement")?;

    ensure!(
        activated_plan.metadata_version() == 3,
        "Stage-B plan is not native-XMR metadata v3"
    );
    let escrow_program_id = activated_plan.escrow_program_id();
    validate_checked_m4_escrow_program_id(escrow_program_id).with_context(|| {
        format!(
            "Stage-B escrow program does not match checked M4 image {CHECKED_M4_ESCROW_PROGRAM_ID_HEX}"
        )
    })?;
    let transfer_program_id = activated_plan.authenticated_transfer_program_id();
    ensure!(
        transfer_program_id == programs::authenticated_transfer().id(),
        "Stage-B authenticated-transfer program differs from this checked build"
    );
    ensure!(
        transfer_program_id != escrow_program_id,
        "escrow and authenticated-transfer programs must be distinct"
    );

    let taker_account_id = AccountId::new(activated_plan.depositor_account());
    ensure!(
        signer_account_id == taker_account_id,
        "Taker signer is not the Stage-B depositor"
    );
    let maker_account_id = AccountId::new(activated_plan.claimant_account());
    let claim_authority = AccountId::new(activated_plan.claim_authority_account());
    let refund_authority = AccountId::new(activated_plan.refund_authority_account());
    ensure!(
        maker_account_id != taker_account_id,
        "Maker and Taker accounts must be distinct"
    );
    let metadata_account_id = compute_metadata_pda(&escrow_program_id, &activated_plan.swap_id());
    let custody_account_id = compute_custody_pda(&escrow_program_id, &activated_plan.swap_id());
    ensure!(
        metadata_account_id.into_value() == activated_plan.metadata_account()
            && custody_account_id.into_value() == activated_plan.custody_account(),
        "Stage-B plan contains noncanonical native-XMR escrow accounts"
    );
    let chain_id = Hex32::from_bytes(activated_plan.channel_id());
    let expected_genesis = Hex32::from_bytes(activated_plan.genesis_hash());
    let escrow_program_hex = program_id_to_hex(escrow_program_id);

    validate_loopback_http_endpoint(&arguments.sequencer_url)
        .context("sequencer endpoint is not a literal-loopback HTTP root")?;
    validate_loopback_http_endpoint(&arguments.indexer_url)
        .context("indexer endpoint is not a literal-loopback HTTP root")?;

    let evidence_path = arguments.state_directory.join(EVIDENCE_FILENAME);
    let evidence_reservation = reserve_evidence_before_node_boundary(&evidence_path)?;
    let node = Arc::new(
        OfficialNodeRpc::connect_local(&arguments.sequencer_url)
            .context("invalid local sequencer endpoint")?,
    );
    let indexer = Arc::new(
        OfficialIndexerRpc::connect_local(&arguments.indexer_url)
            .context("invalid local finalized-indexer endpoint")?,
    );
    let live = node
        .native_escrow_facts(
            metadata_account_id,
            custody_account_id,
            taker_account_id,
            maker_account_id,
        )
        .await
        .context("stable official sequencer preflight failed")?;
    ensure!(
        chain_id.as_bytes() == &live.channel_id(),
        "Stage-B channel differs from the official local channel"
    );
    ensure!(
        expected_genesis.as_bytes() == &live.genesis_block_hash(),
        "Stage-B genesis differs from the official local chain"
    );
    ensure!(
        live.metadata_account().data.is_empty()
            && live.metadata_account().balance == 0
            && live.custody_account().data.is_empty()
            && live.custody_account().balance == 0,
        "swap ID is already initialized or custody is nonempty"
    );
    ensure!(
        live.depositor_account().balance >= activated_plan.amount(),
        "Taker balance is below the exact tag-13 amount"
    );
    ensure!(
        activated_plan.refund_at_ms() > live.tip_timestamp_ms(),
        "refund boundary is not later than the current consensus clock"
    );

    let finalized_nonces = read_stable_m4_finalized_nonce_snapshot(
        indexer.as_ref(),
        expected_genesis,
        M4FinalizedAccountIds::new(
            maker_account_id,
            taker_account_id,
            claim_authority,
            refund_authority,
        ),
    )
    .await
    .context("stable four-account finalized nonce snapshot failed")?;
    ensure!(
        finalized_nonces.finalized_clock().timestamp_ms < activated_plan.refund_at_ms(),
        "refund boundary is not later than the finalized consensus clock"
    );
    ensure_maker_funding_cutoff_open(
        finalized_nonces.finalized_clock().timestamp_ms,
        activated_plan.maker_xmr_funding_cutoff_ms(),
    )
    .context(
        "signed Maker XMR funding cutoff elapsed before Initialize; no transaction submitted",
    )?;

    verify_live_nonce(
        node.as_ref(),
        maker_account_id,
        finalized_nonces.maker_owner().nonce(),
    )
    .await
    .context("Maker live nonce moved ahead of the finalized Stage-A snapshot")?;
    verify_live_nonce(
        node.as_ref(),
        taker_account_id,
        finalized_nonces.taker_owner().nonce(),
    )
    .await
    .context("Taker live nonce moved ahead of the finalized Stage-A snapshot")?;
    verify_live_nonce(
        node.as_ref(),
        claim_authority,
        finalized_nonces.claim_authority().nonce(),
    )
    .await
    .context("claim-authority live nonce moved ahead of the finalized Stage-A snapshot")?;
    verify_live_nonce(
        node.as_ref(),
        refund_authority,
        finalized_nonces.refund_authority().nonce(),
    )
    .await
    .context("refund-authority live nonce moved ahead of the finalized Stage-A snapshot")?;

    let future_plan = plan_m4_stage_a_future_messages(M4StageAFutureMessageInput::new(
        escrow_program_id,
        activated_plan.swap_id(),
        maker_account_id,
        taker_account_id,
        activated_plan.claim_aggregate_x_only_key(),
        activated_plan.refund_aggregate_x_only_key(),
        finalized_nonces.planned_nonces(),
    ))
    .context("exact generated future-message planning failed")?;
    verify_future_plan_against_activated(&activated_plan, &future_plan, &finalized_nonces)?;
    let terms = build_terms_from_activated(&activated_plan)?;

    let runtime_descriptor = RuntimeDescriptor::new(
        Participant::Taker,
        RuntimeCompatibility::LeeV0_2_0,
        chain_id,
        Hex32::from_bytes(live.channel_id()),
        expected_genesis,
        escrow_program_hex,
        Hex32::from_bytes(taker_account_id.into_value()),
    );
    let nonce_source = Arc::new(ExactTakerFinalizedNonce {
        taker: taker_account_id,
        nonce: finalized_nonces.taker_owner().nonce(),
    });
    let planner = Arc::new(
        NativeEscrowPlanner::new_durable(
            Participant::Taker,
            signer_key,
            escrow_program_id,
            transfer_program_id,
            runtime_descriptor.clone(),
            nonce_source,
            &arguments.state_directory,
        )
        .context("durable tag-13 planner binding failed")?,
    );
    let runtime = BridgeRuntime::new(runtime_descriptor.clone(), planner, node, indexer);
    runtime
        .verify_health()
        .await
        .context("official local runtime health binding failed")?;
    let prepare_context = MessageContext::new(
        run_id.clone(),
        prepare_request_id.clone(),
        Participant::Taker,
    );
    let prepared = runtime
        .prepare_native_xmr_escrow_v3(&PrepareNativeXmrEscrowV3Request::new(
            prepare_context,
            runtime_descriptor.clone(),
            terms,
        ))
        .await
        .context("durable exact tag-13 preparation failed")?;

    let initialization_submission = runtime
        .submit_transaction(&submission_request(
            &run_id,
            &runtime_descriptor,
            prepared.initialization.clone(),
        ))
        .await
        .context("single Initialize submission attempt failed or became ambiguous")?;
    let interval = Duration::from_millis(arguments.finality_scan_interval_ms);
    let initialization = await_finalized_effect(
        &runtime,
        &run_id,
        &runtime_descriptor,
        &terms,
        XmrNativeEffectV3::Initialize,
        prepared.initialization,
        finalized_nonces
            .finalized_clock()
            .height
            .checked_add(1)
            .context("Initialize scan cursor overflow")?,
        arguments.max_finality_scans,
        interval,
        initialization_submission.outcome,
    )
    .await
    .context("exact Initialize did not reach affirmative finalized classification")?;
    ensure_maker_funding_cutoff_open(
        initialization.finalized_clock.timestamp_ms,
        activated_plan.maker_xmr_funding_cutoff_ms(),
    )
    .context(
        "signed Maker XMR funding cutoff elapsed while awaiting Initialize; Fund not submitted",
    )?;

    // Fund is not submitted until exact Initialize bytes returned Found and
    // that finalized Initialize clock remains within the signed funding cutoff.
    let funding_submission = runtime
        .submit_transaction(&submission_request(
            &run_id,
            &runtime_descriptor,
            prepared.funding.clone(),
        ))
        .await
        .context("single Fund submission attempt failed or became ambiguous")?;
    let funding = await_finalized_effect(
        &runtime,
        &run_id,
        &runtime_descriptor,
        &terms,
        XmrNativeEffectV3::Fund,
        prepared.funding,
        initialization
            .containing_block_id
            .checked_add(1)
            .context("Fund scan cursor overflow")?,
        arguments.max_finality_scans,
        interval,
        funding_submission.outcome,
    )
    .await
    .context("exact Fund did not reach affirmative finalized classification")?;
    ensure_maker_funding_cutoff_open(
        funding.finalized_clock.timestamp_ms,
        activated_plan.maker_xmr_funding_cutoff_ms(),
    )
    .context("Fund finalized after the signed Maker XMR funding cutoff; recovery is required")?;

    let evidence = StageAEvidence {
        schema: EVIDENCE_SCHEMA,
        role: Participant::Taker,
        run_id,
        prepare_request_id,
        runtime: runtime_descriptor,
        terms,
        stage_a_agreement_wire_sha256: agreement_wire_sha256,
        stage_b_activation_wire_sha256: activation_wire_sha256,
        finalized_nonce_snapshot: finalized_nonces,
        maker_xmr_funding_cutoff_ms: activated_plan.maker_xmr_funding_cutoff_ms(),
        cutoff_authority: "signed_stage_a_agreement_and_stable_finalized_lez_consensus_timestamps",
        agreement_source: "canonical_validated_stage_a_wire",
        activation_source: "canonical_validated_stage_b_wire_and_owner_private_view_key",
        claim_message_hash_source: "generated_official_tag15_message",
        refund_message_hash_source: "generated_official_tag16_message",
        punish_message_hash_source: "generated_official_tag17_message",
        initialization,
        funding,
        execution_scope: "m4_tag13_lez_initialize_and_fund_only",
        funding_barrier: "exact_initialize_found_and_initialize_and_fund_finalized_at_or_before_signed_maker_xmr_funding_cutoff",
        public_rpc_used: false,
        automatic_submission_retry: false,
        send_attempt_ceiling_per_effect_per_process: 1,
        finality_polling_is_submission_retry: false,
        crash_atomic_submission: false,
        public_stage_a_input_snapshot_durably_journaled: false,
        recovery_limitation: "wire_files_and_raw_finalized_nonce_provenance_require_operator_reentry_after_a_crash",
        monero_lock_observed: false,
        swap_completed: false,
        atomic_swap_proven: false,
        atomicity_claim: "none_tag13_only_proves_ordered_finalized_lez_escrow_funding",
    };
    let mut encoded = serde_json::to_vec_pretty(&evidence).context("encode redacted evidence")?;
    encoded.push(b'\n');
    evidence_reservation.commit(&encoded)?;
    Ok((evidence, evidence_path))
}

fn ensure_maker_funding_cutoff_open(
    finalized_consensus_timestamp_ms: u64,
    maker_xmr_funding_cutoff_ms: u64,
) -> Result<()> {
    ensure!(
        maker_xmr_funding_cutoff_ms > 0
            && finalized_consensus_timestamp_ms <= maker_xmr_funding_cutoff_ms,
        "stable finalized LEZ consensus time is after the signed Maker XMR funding cutoff"
    );
    Ok(())
}

fn verify_future_plan_against_activated(
    activated: &XmrLezInitializePlanV1,
    future: &M4StageAFutureMessagePlan,
    finalized: &StableM4FinalizedNonceSnapshot,
) -> Result<()> {
    ensure!(
        future.claim_authority().into_value() == activated.claim_authority_account()
            && future.refund_authority().into_value() == activated.refund_authority_account(),
        "fresh future-message planning disagrees with Stage-B authority accounts"
    );
    ensure!(
        future.claim_hash() == activated.claim_message_hash()
            && future.refund_hash() == activated.refund_message_hash()
            && future.punish_hash() == activated.punish_message_hash(),
        "fresh official future-message hashes disagree with Stage-A commitments"
    );
    let expected_fund = finalized
        .taker_owner()
        .nonce()
        .checked_add(1)
        .context("Taker Fund nonce overflow")?;
    let expected_authorize = finalized
        .taker_owner()
        .nonce()
        .checked_add(2)
        .context("Taker Authorize nonce overflow")?;
    let nonces = future.nonces();
    ensure!(
        nonces.maker_owner_finalized() == finalized.maker_owner().nonce()
            && nonces.taker_owner_finalized() == finalized.taker_owner().nonce()
            && nonces.initialize() == finalized.taker_owner().nonce()
            && nonces.fund() == expected_fund
            && nonces.authorize() == expected_authorize
            && nonces.claim() == finalized.claim_authority().nonce()
            && nonces.refund() == finalized.refund_authority().nonce()
            && nonces.punish() == finalized.maker_owner().nonce(),
        "fresh future-message nonce schedule disagrees with the stable finalized snapshot"
    );
    Ok(())
}

fn build_terms_from_activated(
    activated: &XmrLezInitializePlanV1,
) -> Result<XmrNativeEscrowTermsV3> {
    XmrNativeEscrowTermsV3::new(XmrNativeEscrowTermsV3Input {
        swap_id: Hex32::from_bytes(activated.swap_id()),
        activation_commitment: Hex32::from_bytes(activated.activation_commitment()),
        escrow_program_id: program_id_to_hex(activated.escrow_program_id()),
        authenticated_transfer_program_id: program_id_to_hex(
            activated.authenticated_transfer_program_id(),
        ),
        metadata_account_id: Hex32::from_bytes(activated.metadata_account()),
        custody_account_id: Hex32::from_bytes(activated.custody_account()),
        depositor: Participant::Taker,
        depositor_account_id: Hex32::from_bytes(activated.depositor_account()),
        claimant: Participant::Maker,
        claimant_account_id: Hex32::from_bytes(activated.claimant_account()),
        claim_aggregate_x_only_public_key: Hex32::from_bytes(
            activated.claim_aggregate_x_only_key(),
        ),
        claim_authority_account_id: Hex32::from_bytes(activated.claim_authority_account()),
        refund_aggregate_x_only_public_key: Hex32::from_bytes(
            activated.refund_aggregate_x_only_key(),
        ),
        refund_authority_account_id: Hex32::from_bytes(activated.refund_authority_account()),
        maker_dleq_transcript_commitment: Hex32::from_bytes(
            activated.maker_dleq_transcript_commitment(),
        ),
        taker_dleq_transcript_commitment: Hex32::from_bytes(
            activated.taker_dleq_transcript_commitment(),
        ),
        claim_partial_context_binding: Hex32::from_bytes(activated.claim_partial_context_binding()),
        claim_partial_commitment: Hex32::from_bytes(activated.claim_partial_commitment()),
        amount: activated.amount(),
        refund_at_ms: activated.refund_at_ms(),
        punish_at_ms: activated.punish_at_ms(),
        claim_message_hash: Hex32::from_bytes(activated.claim_message_hash()),
        refund_message_hash: Hex32::from_bytes(activated.refund_message_hash()),
        punish_message_hash: Hex32::from_bytes(activated.punish_message_hash()),
    })
    .context("invalid complete Stage-B-derived XMR-native v3 terms")
}

fn submission_request(
    run_id: &RunId,
    runtime: &RuntimeDescriptor,
    transaction: PreparedTransaction,
) -> SubmitTransactionRequest {
    SubmitTransactionRequest::new(
        MessageContext::new(
            run_id.clone(),
            transaction.transaction_id.submission_request_id(),
            Participant::Taker,
        ),
        runtime.clone(),
        transaction,
    )
}

#[allow(
    clippy::too_many_arguments,
    reason = "the finalized barrier keeps every immutable binding explicit"
)]
async fn await_finalized_effect(
    runtime: &BridgeRuntime,
    run_id: &RunId,
    descriptor: &RuntimeDescriptor,
    terms: &XmrNativeEscrowTermsV3,
    effect: XmrNativeEffectV3,
    transaction: PreparedTransaction,
    mut cursor: u64,
    max_calls: u32,
    interval: Duration,
    submission_outcome: SubmissionOutcome,
) -> Result<EffectEvidence> {
    for call in 1..=max_calls {
        let request_id = finalized_request_id(effect, cursor, call)?;
        let window = DiscoveryWindow::new(cursor, 1).context("invalid finalized scan window")?;
        let result = runtime
            .classify_finalized_native_xmr_effect_v3(
                &ClassifyFinalizedNativeXmrEffectV3Request::new(
                    MessageContext::new(run_id.clone(), request_id, Participant::Taker),
                    descriptor.clone(),
                    *terms,
                    effect,
                    FinalizedNativeXmrTransactionTargetV3::exact(transaction.clone()),
                    window,
                ),
            )
            .await
            .context("one bounded finalized classification call failed")?;
        match result.outcome {
            FinalizedNativeXmrScanOutcomeV3::Found {
                finalized_clock,
                scanned_window,
                facts,
            } => {
                ensure!(
                    facts.transaction.transaction_id == transaction.transaction_id,
                    "classifier returned a substituted exact transaction"
                );
                return Ok(EffectEvidence {
                    effect,
                    transaction_id: transaction.transaction_id,
                    submission_outcome,
                    finalized_clock,
                    scanned_window,
                    containing_block_id: facts.containing_block.block_id,
                    containing_block_hash: facts.containing_block.block_hash,
                    transaction_index: facts.transaction.position.transaction_index,
                    classifier_calls: call,
                });
            }
            FinalizedNativeXmrScanOutcomeV3::Absent { .. }
            | FinalizedNativeXmrScanOutcomeV3::Uncertain { .. } => {
                cursor = cursor
                    .checked_add(1)
                    .context("finalized scan cursor overflow")?;
            }
            FinalizedNativeXmrScanOutcomeV3::Unavailable {
                reason:
                    FinalizedNativeXmrUnavailableReasonV3::FinalityUnavailable
                    | FinalizedNativeXmrUnavailableReasonV3::MovingTip,
            } => {}
            FinalizedNativeXmrScanOutcomeV3::Unavailable { reason } => {
                bail!("finalized classification cannot safely continue: {reason:?}");
            }
        }
        tokio::time::sleep(interval).await;
    }
    bail!("bounded finalized-classification calls exhausted")
}

fn finalized_request_id(effect: XmrNativeEffectV3, height: u64, call: u32) -> Result<RequestId> {
    let prefix = match effect {
        XmrNativeEffectV3::Initialize => "m4-i",
        XmrNativeEffectV3::Fund => "m4-f",
        _ => bail!("tag-13 classifier accepts only Initialize or Fund"),
    };
    RequestId::new(format!("{prefix}-{height:016x}-{call:08x}"))
        .context("invalid derived finalized request ID")
}

async fn verify_live_nonce(
    node: &OfficialNodeRpc,
    account_id: AccountId,
    finalized_nonce: u128,
) -> Result<()> {
    let live_nonce = NonceSource::account_nonce(node, account_id)
        .await
        .context("live nonce preflight unavailable")?;
    ensure!(
        live_nonce == finalized_nonce,
        "live and finalized nonce differ"
    );
    Ok(())
}

fn sha256_hex(bytes: &[u8]) -> Hex32 {
    Hex32::from_bytes(Sha256::digest(bytes).into())
}

fn read_bounded_wire_file(
    path: &Path,
    name: &str,
    max_bytes: usize,
    owner_only: bool,
) -> Result<Vec<u8>> {
    let before = fs::symlink_metadata(path).with_context(|| format!("{name} is unavailable"))?;
    ensure!(before.is_file(), "{name} must be a regular file");
    ensure!(before.nlink() == 1, "{name} must have one link");
    if owner_only {
        ensure!(
            before.uid() == rustix::process::geteuid().as_raw()
                && before.permissions().mode().trailing_zeros() >= 6,
            "{name} must be owner-only and owned by the current user"
        );
    }
    ensure!(
        before.len() <= max_bytes as u64,
        "{name} exceeds its fixed wire bound"
    );

    let mut file = File::open(path).with_context(|| format!("{name} could not be opened"))?;
    let opened = file
        .metadata()
        .with_context(|| format!("{name} metadata could not be read"))?;
    ensure!(
        before.dev() == opened.dev()
            && before.ino() == opened.ino()
            && opened.is_file()
            && opened.nlink() == 1,
        "{name} changed while it was being opened"
    );
    let mut bytes = Vec::with_capacity(max_bytes);
    std::io::Read::by_ref(&mut file)
        .take((max_bytes as u64).saturating_add(1))
        .read_to_end(&mut bytes)
        .with_context(|| format!("{name} could not be read"))?;
    ensure!(!bytes.is_empty(), "{name} must not be empty");
    ensure!(
        bytes.len() <= max_bytes,
        "{name} exceeds its fixed wire bound"
    );
    Ok(bytes)
}

fn read_monero_view_key(path: &Path) -> Result<MoneroPrivateViewKey> {
    let encoded = Zeroizing::new(read_bounded_wire_file(
        path,
        "Monero private view key file",
        128,
        true,
    )?);
    let encoded = std::str::from_utf8(encoded.as_slice())
        .context("Monero private view key file must be UTF-8")?
        .trim_end_matches(['\r', '\n']);
    ensure!(
        encoded.len() == 64
            && encoded
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "Monero private view key file must contain exactly 64 lowercase hex characters"
    );
    let mut bytes = Zeroizing::new([0_u8; 32]);
    hex::decode_to_slice(encoded, bytes.as_mut())
        .context("Monero private view key file contains invalid hex")?;
    MoneroPrivateViewKey::from_monero_little_endian(*bytes)
        .context("Monero private view key is not a canonical nonzero scalar")
}

fn validate_state_directory(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path).context("state directory is unavailable")?;
    ensure!(
        metadata.is_dir(),
        "state directory must be a real directory"
    );
    ensure!(
        metadata.permissions().mode() & 0o7777 == 0o700,
        "state directory must already have mode 0700"
    );
    ensure!(
        metadata.uid() == rustix::process::geteuid().as_raw(),
        "state directory must be owned by the current user"
    );
    Ok(())
}

fn read_private_key(path: &Path) -> Result<PrivateKey> {
    let encoded = Zeroizing::new(read_bounded_wire_file(path, "private key file", 128, true)?);
    let encoded = std::str::from_utf8(encoded.as_slice())
        .context("private key file must be UTF-8")?
        .trim_end_matches(['\r', '\n']);
    ensure!(
        encoded.len() == 64
            && encoded
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "private key file must contain exactly 64 lowercase hex characters"
    );
    PrivateKey::from_str(encoded).context("private key file contains an invalid key")
}

#[derive(Debug)]
struct OwnerOnlyEvidenceReservation {
    file: File,
}

impl OwnerOnlyEvidenceReservation {
    fn reserve(path: &Path) -> Result<Self> {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true).mode(0o600);
        let file = options
            .open(path)
            .context("owner-only evidence already exists or cannot be reserved")?;
        validate_evidence_file(&file)?;
        Ok(Self { file })
    }

    fn commit(mut self, bytes: &[u8]) -> Result<()> {
        self.file
            .write_all(bytes)
            .context("write owner-only evidence")?;
        self.file.sync_all().context("sync owner-only evidence")?;
        validate_evidence_file(&self.file)
    }
}

fn reserve_evidence_before_node_boundary(path: &Path) -> Result<OwnerOnlyEvidenceReservation> {
    let reservation = OwnerOnlyEvidenceReservation::reserve(path)?;
    #[cfg(test)]
    NODE_BOUNDARY_ENTRIES.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    Ok(reservation)
}

fn validate_evidence_file(file: &File) -> Result<()> {
    let metadata = file.metadata().context("inspect owner-only evidence")?;
    ensure!(
        metadata.is_file()
            && metadata.nlink() == 1
            && metadata.uid() == rustix::process::geteuid().as_raw()
            && metadata.permissions().mode() & 0o777 == 0o600,
        "evidence file is not an owner-only, single-link regular file"
    );
    Ok(())
}
#[cfg(test)]
mod tests {
    use std::{fs, os::unix::fs::PermissionsExt as _};

    use lez_v0_2_sidecar::CHECKED_M4_ESCROW_PROGRAM_ID;
    use nssa::PrivateKey;
    use tempfile::TempDir;

    use super::*;

    fn identity(byte: u8) -> (AccountId, PrivateKey, PublicKey) {
        let private = PrivateKey::try_new([byte; 32]).expect("valid fixture scalar");
        let public = PublicKey::new_from_private_key(&private);
        (AccountId::from(&public), private, public)
    }

    fn private_directory() -> TempDir {
        let directory = TempDir::new().expect("temporary directory");
        fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700))
            .expect("owner-only directory");
        directory
    }

    #[test]
    fn checked_m4_program_id_matches_source_manifest() {
        let manifest = include_str!(
            "../../../lez-v0.2-provisional/escrow/methods/guest/m4-deployment-manifest.toml"
        );
        let expected_line = format!("image_id = \"{CHECKED_M4_ESCROW_PROGRAM_ID_HEX}\"");
        assert!(manifest.lines().any(|line| line == expected_line));
        assert_eq!(
            hex::encode(program_id_to_hex(CHECKED_M4_ESCROW_PROGRAM_ID).as_bytes()),
            CHECKED_M4_ESCROW_PROGRAM_ID_HEX
        );
    }

    #[test]
    fn mutated_m4_program_id_is_rejected() {
        validate_checked_m4_escrow_program_id(CHECKED_M4_ESCROW_PROGRAM_ID)
            .expect("exact checked M4 program");
        let mut mutated = CHECKED_M4_ESCROW_PROGRAM_ID;
        mutated[0] ^= 1;
        assert!(validate_checked_m4_escrow_program_id(mutated).is_err());
    }

    #[tokio::test]
    async fn fixed_finalized_nonce_rejects_every_non_taker_account() {
        let (taker, _, _) = identity(21);
        let (maker, _, _) = identity(22);
        let source = ExactTakerFinalizedNonce { taker, nonce: 19 };
        assert_eq!(source.account_nonce(taker).await.expect("Taker nonce"), 19);
        assert_eq!(
            source
                .account_nonce(maker)
                .await
                .expect_err("wrong account"),
            NativePrepareError::NonceUnavailable
        );
    }

    #[test]
    fn evidence_creation_is_owner_only_and_no_clobber() {
        let directory = private_directory();
        let path = directory.path().join(EVIDENCE_FILENAME);
        let reservation =
            OwnerOnlyEvidenceReservation::reserve(&path).expect("reserve before side effects");
        let metadata = fs::metadata(&path).expect("metadata");
        assert_eq!(metadata.permissions().mode() & 0o777, 0o600);
        assert_eq!(metadata.len(), 0);
        assert!(OwnerOnlyEvidenceReservation::reserve(&path).is_err());
        reservation
            .commit(b"{\"redacted\":true}\n")
            .expect("complete held reservation");
        assert_eq!(
            fs::read_to_string(path).expect("evidence"),
            "{\"redacted\":true}\n"
        );
    }

    #[test]
    fn existing_evidence_rejects_before_node_boundary_and_zero_sends() {
        let directory = private_directory();
        let path = directory.path().join(EVIDENCE_FILENAME);
        OwnerOnlyEvidenceReservation::reserve(&path)
            .expect("reserve existing evidence")
            .commit(b"existing evidence\n")
            .expect("complete existing evidence");
        NODE_BOUNDARY_ENTRIES.store(0, std::sync::atomic::Ordering::SeqCst);

        let error = reserve_evidence_before_node_boundary(&path)
            .expect_err("no-clobber must fail before node boundary");
        assert!(
            format!("{error:#}").contains("owner-only evidence already exists"),
            "unexpected error: {error:#}"
        );
        assert_eq!(
            NODE_BOUNDARY_ENTRIES.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "no sequencer/indexer construction or send path may be entered"
        );
    }

    #[test]
    fn private_key_accepts_only_optional_trailing_newlines() {
        let directory = private_directory();
        let valid = directory.path().join("valid.key");
        let bad = directory.path().join("bad.key");
        fs::write(&valid, format!("{}\r\n", hex::encode([21_u8; 32]))).expect("write valid key");
        fs::write(&bad, format!(" {}\n", hex::encode([21_u8; 32]))).expect("write invalid key");
        for path in [&valid, &bad] {
            fs::set_permissions(path, fs::Permissions::from_mode(0o600)).expect("owner-only key");
        }
        assert!(read_private_key(&valid).is_ok());
        assert!(read_private_key(&bad).is_err());
    }

    #[test]
    fn monero_view_key_is_owner_only_and_strict() {
        let directory = private_directory();
        let valid = directory.path().join("view.key");
        let exposed = directory.path().join("exposed-view.key");
        let mut scalar = [0_u8; 32];
        scalar[0] = 1;
        fs::write(&valid, format!("{}\n", hex::encode(scalar))).expect("write view key");
        fs::write(&exposed, hex::encode(scalar)).expect("write exposed view key");
        fs::set_permissions(&valid, fs::Permissions::from_mode(0o600)).expect("private view key");
        fs::set_permissions(&exposed, fs::Permissions::from_mode(0o644)).expect("exposed view key");
        assert!(read_monero_view_key(&valid).is_ok());
        assert!(read_monero_view_key(&exposed).is_err());
    }

    #[test]
    fn hard_linked_wire_is_rejected() {
        let directory = private_directory();
        let wire = directory.path().join("agreement.wire");
        let alias = directory.path().join("agreement.alias");
        fs::write(&wire, b"canonical-wire").expect("write wire");
        fs::hard_link(&wire, alias).expect("hard link");
        assert!(read_bounded_wire_file(&wire, "agreement", 64, false).is_err());
    }

    #[test]
    fn loose_protocol_fields_are_not_cli_arguments() {
        let error = Arguments::try_parse_from([
            "lez-v02-xmr-stage-a-poc",
            "--swap-id",
            "0000000000000000000000000000000000000000000000000000000000000001",
        ])
        .expect_err("legacy loose protocol field must be rejected");
        assert_eq!(error.kind(), clap::error::ErrorKind::UnknownArgument);
    }

    #[test]
    fn stale_finalized_clock_is_rejected_before_initialize_policy() {
        assert!(ensure_maker_funding_cutoff_open(1_001, 1_000).is_err());
        assert!(ensure_maker_funding_cutoff_open(1_000, 1_000).is_ok());
    }

    #[test]
    fn cutoff_crossing_after_initialize_is_rejected_before_fund_policy() {
        ensure_maker_funding_cutoff_open(999, 1_000).expect("Initialize remains permitted");
        assert!(ensure_maker_funding_cutoff_open(1_001, 1_000).is_err());
    }

    #[test]
    fn finalized_request_ids_are_effect_and_window_specific() {
        let initialize =
            finalized_request_id(XmrNativeEffectV3::Initialize, 42, 1).expect("Initialize ID");
        let fund = finalized_request_id(XmrNativeEffectV3::Fund, 42, 1).expect("Fund ID");
        let next =
            finalized_request_id(XmrNativeEffectV3::Initialize, 43, 2).expect("next Initialize ID");
        assert_ne!(initialize, fund);
        assert_ne!(initialize, next);
        assert!(finalized_request_id(XmrNativeEffectV3::Claim, 42, 1).is_err());
    }
}
