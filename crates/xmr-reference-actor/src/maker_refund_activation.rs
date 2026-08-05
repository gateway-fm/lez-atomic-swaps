//! Evidence-driven Maker refund workflow activation.
//!
//! This boundary intentionally exposes no branch selector. Exact, role-local
//! finalized Tag-16 evidence is the sole authority for choosing Refund.

use std::{io::Write as _, path::Path};

use anyhow::{Context as _, Result, anyhow, ensure};
use lez_adaptor_role_runner::{ValidatedSession, read_final_signature_packet};
use lez_bridge_adapter::XmrLezBridgeBindingV3;
use lez_bridge_protocol::{Participant as BridgeParticipant, XmrNativeEffectV3};
use lez_swap_store::{
    SqliteXmrWorkflowJournal, XmrWorkflowBranch, XmrWorkflowReconciliationSource,
    XmrWorkflowReconciliationV2, XmrWorkflowStep,
};
use lez_xmr_swap_sdk::{MoneroAddressNetworkV1, XmrActivatedAgreementV1, XmrAgreementV1};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest as _, Sha256};

use crate::{
    ActorRole, M4_MONERO_CONFIRMATIONS, M4_MONERO_DAEMON_VERSION, M4_MONERO_EVIDENCE_MAX_BYTES,
    M4_MONERO_NETWORK_SCOPE, M4_MONERO_RECEIPT_SCHEMA, M4_MONERO_WALLET_VERSION,
    MoneroReceiptEvidenceV2, SecureDestination, XMR_ACTOR_PROVISION_MANIFEST_MAX_BYTES,
    XMR_EFFECT_AUTHORITY_MAX_BYTES, canonical_json_bytes, decode_exact,
    discovered_finalized_xmr_facts,
    effect_input_custody::{
        XMR_EFFECT_FINALIZED_REFUND_SIGNATURE_FILE, parse_private_view_key_bytes,
    },
    load_validated_xmr_effect_execution_v3_bytes, read_canonical_private_json,
    read_finalized_xmr_effect, read_private_input, write_bounded_public_new,
};

const FUNDING_SCHEMA: &str = "lez_v02_m4_actual_local_monero_funding_v2";
const ACTIVATION_SCHEMA: &str = "lez_v02_m7_maker_refund_activation_v1";
const TAKER_CLAIM_ACTIVATION_SCHEMA: &str = "lez_v02_m7_taker_claim_activation_v1";
const TAG13_SCHEMA: &str = "lez_v02_m4_xmr_stage_a_tag13_poc_v2";

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct FundingEvidenceV2 {
    schema: String,
    attempt_state: String,
    agreement_commitment: String,
    shared_address: String,
    amount_piconero: u64,
    transaction_id: Option<String>,
    confirmation_tip_height: Option<u64>,
    required_confirmations: u64,
    restore_height: u64,
    wallet_role: String,
    network_scope: String,
    public_rpc_used: bool,
    faucet_used: bool,
    automatic_submission_retry: bool,
}

#[derive(Debug, Deserialize, Serialize)]
struct Tag13ClockV1 {
    block_hash: String,
    height: u64,
    timestamp_ms: u64,
}

#[derive(Debug, Deserialize, Serialize)]
struct Tag13EffectV1 {
    effect: String,
    transaction_id: String,
    submission_outcome: String,
    finalized_clock: Tag13ClockV1,
    containing_block_id: u64,
    containing_block_hash: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "independent negative assurance facts must remain explicit at the import boundary"
)]
struct Tag13EvidenceV2 {
    schema: String,
    role: String,
    run_id: String,
    stage_a_agreement_wire_sha256: String,
    stage_b_activation_wire_sha256: String,
    terms: Value,
    initialization: Tag13EffectV1,
    funding: Tag13EffectV1,
    maker_xmr_funding_cutoff_ms: u64,
    public_rpc_used: bool,
    automatic_submission_retry: bool,
    send_attempt_ceiling_per_effect_per_process: u64,
    finality_polling_is_submission_retry: bool,
    crash_atomic_submission: bool,
    monero_lock_observed: bool,
    swap_completed: bool,
    atomic_swap_proven: bool,
    atomicity_claim: String,
}

#[derive(Serialize)]
struct ImportedTag13PlanV1 {
    schema_version: u16,
    step: &'static str,
    role: &'static str,
    run_id: String,
    swap_id: String,
    agreement_commitment: String,
    activation_commitment: String,
    effect_authority_sha256: String,
    tag13_evidence_sha256: String,
    transaction_id: String,
    containing_block_id: u64,
    containing_block_hash: String,
}

#[derive(Serialize)]
struct TakerClaimActivationSummaryV1 {
    schema: &'static str,
    role: &'static str,
    run_id: String,
    monero_run_id: String,
    swap_id: String,
    selected_branch: &'static str,
    tag13_evidence_sha256: String,
    initialization_effect_evidence_sha256: String,
    initialization_tool_plan_identity_sha256: String,
    funding_effect_evidence_sha256: String,
    funding_tool_plan_identity_sha256: String,
    monero_funding_evidence_sha256: String,
    monero_funding_receipt_sha256: String,
    tag14_scan_start_height: u64,
    prepared_step: &'static str,
    private_material_disclosed: bool,
}

#[derive(Serialize)]
struct ImportedFundingPlanV1 {
    schema_version: u16,
    step: &'static str,
    role: &'static str,
    run_id: String,
    swap_id: String,
    agreement_commitment: String,
    activation_commitment: String,
    effect_authority_sha256: String,
    funding_tool_program_sha256: String,
    funding_tool_abi: String,
    funding_evidence_sha256: String,
    funding_receipt_sha256: String,
}

#[derive(Serialize)]
struct ActivationSummaryV1 {
    schema: &'static str,
    role: &'static str,
    run_id: String,
    monero_run_id: String,
    swap_id: String,
    selected_branch: &'static str,
    funding_effect_evidence_sha256: String,
    funding_tool_plan_identity_sha256: String,
    finalized_refund_sha256: String,
    finalized_refund_signature_sha256: String,
    prepared_step: &'static str,
    private_material_disclosed: bool,
}

/// Validates the already-finalized common effects and prepares only the
/// receipt-v2 Taker claim authorization. No operator branch selector exists.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
pub(crate) fn activate_taker_claim_workflow(
    effect_manifest: &Path,
    effect_authority: &Path,
    run_id: &str,
    monero_run_id: &str,
    tag13_evidence: &Path,
    monero_funding_evidence: &Path,
    monero_funding_receipt: &Path,
) -> Result<()> {
    let manifest_bytes = read_private_input(
        effect_manifest,
        XMR_ACTOR_PROVISION_MANIFEST_MAX_BYTES,
        "schema-3 Taker effect manifest",
    )?;
    let authority_bytes = read_private_input(
        effect_authority,
        XMR_EFFECT_AUTHORITY_MAX_BYTES,
        "Taker effect authority",
    )?;
    let execution = load_validated_xmr_effect_execution_v3_bytes(
        &manifest_bytes,
        &authority_bytes,
        ActorRole::Taker,
        run_id,
    )
    .context("validate Taker effect application before claim activation")?;
    let authority = execution.effect_authority();
    ensure!(
        authority.schema_version() == 2
            && authority.role() == ActorRole::Taker
            && authority.run_id() == run_id
            && authority.tag14_release().is_some(),
        "claim activation requires the exact schema-2 Taker release authority"
    );

    let agreement = XmrAgreementV1::from_wire(&execution.application.stage_a_wire)
        .context("effect application Stage A is invalid")?;
    ensure!(
        agreement.body().monero().network() == MoneroAddressNetworkV1::Regtest,
        "M7 local claim activation requires Monero Regtest"
    );
    let view_key = parse_private_view_key_bytes(&execution.application.private_view_key)?;
    let activation = XmrActivatedAgreementV1::from_wire(
        &agreement,
        &execution.application.stage_b_wire,
        &view_key,
    )
    .context("effect application Stage B is invalid")?;
    ensure!(
        agreement.body().swap_id() == authority.swap_id()
            && agreement.agreement_commitment() == authority.agreement_commitment()
            && activation.activation_commitment() == authority.activation_commitment(),
        "claim activation application identity changed"
    );

    let (tag13, tag13_bytes) = read_canonical_private_json::<Tag13EvidenceV2>(
        tag13_evidence,
        "finalized Tag-13 evidence",
    )?;
    validate_tag13(&execution, &agreement, &activation, &tag13)?;

    let (funding, funding_bytes) = read_canonical_private_json::<FundingEvidenceV2>(
        monero_funding_evidence,
        "Monero funding evidence v2",
    )?;
    let (receipt, receipt_bytes) = read_canonical_private_json::<MoneroReceiptEvidenceV2>(
        monero_funding_receipt,
        "Monero funding receipt v2",
    )?;
    validate_funding_pair(&agreement, monero_run_id, &funding, &receipt)?;

    let tag13_sha256: [u8; 32] = Sha256::digest(&tag13_bytes).into();
    let funding_evidence_sha256: [u8; 32] = Sha256::digest(&funding_bytes).into();
    let funding_receipt_sha256: [u8; 32] = Sha256::digest(&receipt_bytes).into();
    let identity = execution.workflow_identity();
    let mut workflow = SqliteXmrWorkflowJournal::open_existing(authority.workflow_journal())
        .context("open Taker effect workflow for evidence-driven activation")?;
    let mut reconciliations = Vec::with_capacity(2);
    for (step, effect) in [
        (XmrWorkflowStep::InitializeLezTag13, &tag13.initialization),
        (XmrWorkflowStep::FundLezTag13, &tag13.funding),
    ] {
        let effect_evidence_sha256 = domain_digest(
            b"lez-xmr-m7-imported-tag13-evidence-v1",
            &[step.name().as_bytes(), &tag13_bytes],
        );
        let imported_plan = ImportedTag13PlanV1 {
            schema_version: 1,
            step: step.name(),
            role: "taker",
            run_id: run_id.to_owned(),
            swap_id: hex::encode(authority.swap_id()),
            agreement_commitment: hex::encode(authority.agreement_commitment()),
            activation_commitment: hex::encode(authority.activation_commitment()),
            effect_authority_sha256: hex::encode(execution.effect_authority_sha256()),
            tag13_evidence_sha256: hex::encode(tag13_sha256),
            transaction_id: effect.transaction_id.clone(),
            containing_block_id: effect.containing_block_id,
            containing_block_hash: effect.containing_block_hash.clone(),
        };
        let imported_plan_bytes =
            canonical_json_bytes(&imported_plan, "encode imported Tag-13 plan")?;
        let plan_sha256: [u8; 32] = Sha256::digest(&imported_plan_bytes).into();
        let reconciliation = XmrWorkflowReconciliationV2::new(
            effect_evidence_sha256,
            plan_sha256,
            XmrWorkflowReconciliationSource::LezFinalizedEvent,
        )
        .context("construct imported Tag-13 reconciliation")?;
        workflow
            .prepare_step(identity, step)
            .context("prepare imported Taker Tag-13 step")?;
        let _ = workflow
            .authorize_once(identity, step)
            .context("consume imported Taker Tag-13 authority")?;
        workflow
            .reconcile_succeeded(identity, step, &reconciliation)
            .context("reconcile exact imported Tag-13 evidence")?;
        reconciliations.push((effect_evidence_sha256, plan_sha256, reconciliation));
    }
    workflow
        .select_branch(identity, XmrWorkflowBranch::Claim)
        .context("select claim branch from finalized cross-chain prerequisites")?;
    workflow
        .prepare_step(identity, XmrWorkflowStep::AuthorizeLezTag14)
        .context("prepare Taker Tag-14 release")?;
    ensure!(
        workflow.selected_branch(identity)? == Some(XmrWorkflowBranch::Claim)
            && workflow.load_reconciliation(identity, XmrWorkflowStep::InitializeLezTag13)?
                == Some(reconciliations[0].2.clone())
            && workflow.load_reconciliation(identity, XmrWorkflowStep::FundLezTag13)?
                == Some(reconciliations[1].2.clone())
            && workflow.step_revision(identity, XmrWorkflowStep::AuthorizeLezTag14)? == 0,
        "durable Taker claim activation did not replay exactly"
    );

    let summary = TakerClaimActivationSummaryV1 {
        schema: TAKER_CLAIM_ACTIVATION_SCHEMA,
        role: "taker",
        run_id: run_id.to_owned(),
        monero_run_id: monero_run_id.to_owned(),
        swap_id: hex::encode(authority.swap_id()),
        selected_branch: "claim",
        tag13_evidence_sha256: hex::encode(tag13_sha256),
        initialization_effect_evidence_sha256: hex::encode(reconciliations[0].0),
        initialization_tool_plan_identity_sha256: hex::encode(reconciliations[0].1),
        funding_effect_evidence_sha256: hex::encode(reconciliations[1].0),
        funding_tool_plan_identity_sha256: hex::encode(reconciliations[1].1),
        monero_funding_evidence_sha256: hex::encode(funding_evidence_sha256),
        monero_funding_receipt_sha256: hex::encode(funding_receipt_sha256),
        tag14_scan_start_height: tag13
            .funding
            .containing_block_id
            .checked_add(1)
            .context("Tag-13 funding height overflow")?,
        prepared_step: XmrWorkflowStep::AuthorizeLezTag14.name(),
        private_material_disclosed: false,
    };
    std::io::stdout()
        .write_all(&canonical_json_bytes(
            &summary,
            "encode Taker claim activation summary",
        )?)
        .context("write Taker claim activation summary")?;
    Ok(())
}

fn validate_tag13(
    execution: &crate::ValidatedXmrEffectExecutionV3,
    agreement: &XmrAgreementV1,
    activation: &XmrActivatedAgreementV1,
    evidence: &Tag13EvidenceV2,
) -> Result<()> {
    let binding = XmrLezBridgeBindingV3::new(agreement, activation)
        .context("derive finalized Tag-13 terms from Stage B")?;
    let expected_terms =
        serde_json::to_value(binding.terms()).context("encode expected Tag-13 terms")?;
    ensure!(
        evidence.schema == TAG13_SCHEMA
            && evidence.role == "taker"
            && evidence.run_id == execution.effect_authority().run_id()
            && evidence.stage_a_agreement_wire_sha256
                == hex::encode(Sha256::digest(&execution.application.stage_a_wire))
            && evidence.stage_b_activation_wire_sha256
                == hex::encode(Sha256::digest(&execution.application.stage_b_wire))
            && evidence.terms == expected_terms
            && !evidence.public_rpc_used
            && !evidence.automatic_submission_retry
            && evidence.send_attempt_ceiling_per_effect_per_process == 1
            && !evidence.finality_polling_is_submission_retry
            && !evidence.crash_atomic_submission
            && !evidence.monero_lock_observed
            && !evidence.swap_completed
            && !evidence.atomic_swap_proven
            && evidence.atomicity_claim
                == "none_tag13_only_proves_ordered_finalized_lez_escrow_funding",
        "Tag-13 evidence differs from the exact local Taker application"
    );
    validate_tag13_effect(&evidence.initialization, "initialize")?;
    validate_tag13_effect(&evidence.funding, "fund")?;
    ensure!(
        evidence.initialization.finalized_clock.height < evidence.funding.finalized_clock.height
            && evidence.initialization.finalized_clock.timestamp_ms
                <= evidence.maker_xmr_funding_cutoff_ms
            && evidence.funding.finalized_clock.timestamp_ms
                <= evidence.maker_xmr_funding_cutoff_ms,
        "Tag-13 finalized ordering or funding cutoff is invalid"
    );
    Ok(())
}

fn validate_tag13_effect(effect: &Tag13EffectV1, expected: &str) -> Result<()> {
    ensure!(
        effect.effect == expected
            && effect.submission_outcome == "accepted"
            && effect.finalized_clock.height == effect.containing_block_id
            && effect.finalized_clock.block_hash == effect.containing_block_hash,
        "Tag-13 {expected} effect is not finalized in its containing block"
    );
    let _ =
        decode_exact::<32>(&effect.transaction_id).context("Tag-13 transaction ID is invalid")?;
    let block = decode_exact::<32>(&effect.containing_block_hash)
        .context("Tag-13 containing block hash is invalid")?;
    ensure!(
        effect.transaction_id != "0".repeat(64) && block != [0; 32],
        "Tag-13 {expected} effect contains a zero chain identity"
    );
    Ok(())
}

/// Validates external evidence and advances one Maker workflow to the prepared
/// Monero-refund sweep without accepting an operator-selected branch.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
pub(crate) fn activate_maker_refund_workflow(
    effect_manifest: &Path,
    effect_authority: &Path,
    run_id: &str,
    monero_run_id: &str,
    monero_funding_evidence: &Path,
    monero_funding_receipt: &Path,
    finalized_refund: &Path,
    observed_final_signature: &Path,
) -> Result<()> {
    let manifest_bytes = read_private_input(
        effect_manifest,
        XMR_ACTOR_PROVISION_MANIFEST_MAX_BYTES,
        "schema-3 Maker effect manifest",
    )?;
    let authority_bytes = read_private_input(
        effect_authority,
        XMR_EFFECT_AUTHORITY_MAX_BYTES,
        "Maker effect authority",
    )?;
    let execution = load_validated_xmr_effect_execution_v3_bytes(
        &manifest_bytes,
        &authority_bytes,
        ActorRole::Maker,
        run_id,
    )
    .context("validate Maker effect application before refund activation")?;
    let authority = execution.effect_authority();
    ensure!(
        authority.schema_version() == 3
            && authority.role() == ActorRole::Maker
            && authority.run_id() == run_id,
        "refund activation requires the exact schema-3 Maker authority"
    );

    let agreement = XmrAgreementV1::from_wire(&execution.application.stage_a_wire)
        .context("effect application Stage A is invalid")?;
    ensure!(
        agreement.body().monero().network() == MoneroAddressNetworkV1::Regtest,
        "M7 local refund activation requires Monero Regtest"
    );
    let view_key = parse_private_view_key_bytes(&execution.application.private_view_key)?;
    let activation = XmrActivatedAgreementV1::from_wire(
        &agreement,
        &execution.application.stage_b_wire,
        &view_key,
    )
    .context("effect application Stage B is invalid")?;
    ensure!(
        agreement.body().swap_id() == authority.swap_id()
            && agreement.agreement_commitment() == authority.agreement_commitment()
            && activation.activation_commitment() == authority.activation_commitment(),
        "refund activation application identity changed"
    );

    let (funding, funding_bytes) = read_canonical_private_json::<FundingEvidenceV2>(
        monero_funding_evidence,
        "Monero funding evidence v2",
    )?;
    let (receipt, receipt_bytes) = read_canonical_private_json::<MoneroReceiptEvidenceV2>(
        monero_funding_receipt,
        "Monero funding receipt v2",
    )?;
    validate_funding_pair(&agreement, monero_run_id, &funding, &receipt)?;

    let binding = XmrLezBridgeBindingV3::new(&agreement, &activation)
        .context("derive finalized refund terms from Stage B")?;
    let finalized_result = read_finalized_xmr_effect(finalized_refund)?;
    let facts = discovered_finalized_xmr_facts(
        &finalized_result,
        run_id,
        &binding.terms(),
        XmrNativeEffectV3::Refund,
        BridgeParticipant::Maker,
    )?;
    let aggregate_signature = facts
        .aggregate_signature
        .ok_or_else(|| anyhow!("finalized Tag-16 facts omit the aggregate signature"))?;

    let observed_bytes_before = read_private_input(
        observed_final_signature,
        M4_MONERO_EVIDENCE_MAX_BYTES,
        "observed refund final-signature packet",
    )?;
    let refund_session = ValidatedSession::from_untweaked_context(
        agreement
            .refund_session_descriptor()
            .context()
            .context("refund session descriptor is invalid")?,
    )
    .context("refund runner session is invalid")?;
    let observed_signature = read_final_signature_packet(observed_final_signature, &refund_session)
        .context("validate observed finalized Tag-16 signature packet")?;
    let observed_bytes_after = read_private_input(
        observed_final_signature,
        M4_MONERO_EVIDENCE_MAX_BYTES,
        "observed refund final-signature packet",
    )?;
    ensure!(
        observed_bytes_before == observed_bytes_after
            && aggregate_signature.as_bytes() == &observed_signature,
        "finalized Tag-16 signature differs from the stable observed packet"
    );

    let finalized_bytes = canonical_json_bytes(&finalized_result, "encode finalized refund")?;
    let funding_evidence_sha256: [u8; 32] = Sha256::digest(&funding_bytes).into();
    let funding_receipt_sha256: [u8; 32] = Sha256::digest(&receipt_bytes).into();
    let effect_evidence_sha256 = domain_digest(
        b"lez-xmr-m7-imported-funding-evidence-v1",
        &[&funding_bytes, &receipt_bytes],
    );
    let maker_tools = authority
        .maker_tools()
        .ok_or_else(|| anyhow!("Maker effect tool profile is unavailable"))?;
    let imported_plan = ImportedFundingPlanV1 {
        schema_version: 1,
        step: XmrWorkflowStep::FundMonero.name(),
        role: "maker",
        run_id: run_id.to_owned(),
        swap_id: hex::encode(authority.swap_id()),
        agreement_commitment: hex::encode(authority.agreement_commitment()),
        activation_commitment: hex::encode(authority.activation_commitment()),
        effect_authority_sha256: hex::encode(execution.effect_authority_sha256()),
        funding_tool_program_sha256: hex::encode(maker_tools.monero_fund().program_sha256()),
        funding_tool_abi: maker_tools.monero_fund().abi().to_owned(),
        funding_evidence_sha256: hex::encode(funding_evidence_sha256),
        funding_receipt_sha256: hex::encode(funding_receipt_sha256),
    };
    let imported_plan_bytes = canonical_json_bytes(&imported_plan, "encode imported funding plan")?;
    let tool_plan_identity_sha256: [u8; 32] = Sha256::digest(&imported_plan_bytes).into();
    let reconciliation = XmrWorkflowReconciliationV2::new(
        effect_evidence_sha256,
        tool_plan_identity_sha256,
        XmrWorkflowReconciliationSource::MoneroWalletTransaction,
    )
    .context("construct exact imported funding reconciliation")?;

    let published_signature = authority
        .evidence_root()
        .join(XMR_EFFECT_FINALIZED_REFUND_SIGNATURE_FILE);
    persist_or_validate_private(&published_signature, &observed_bytes_after)?;

    let mut workflow = SqliteXmrWorkflowJournal::open_existing(authority.workflow_journal())
        .context("open Maker effect workflow for evidence-driven activation")?;
    let identity = execution.workflow_identity();
    workflow
        .prepare_step(identity, XmrWorkflowStep::FundMonero)
        .context("prepare imported Maker Monero funding")?;
    let _ = workflow
        .authorize_once(identity, XmrWorkflowStep::FundMonero)
        .context("consume imported Maker funding authority")?;
    workflow
        .reconcile_succeeded(identity, XmrWorkflowStep::FundMonero, &reconciliation)
        .context("reconcile exact imported Maker funding evidence")?;
    workflow
        .select_branch(identity, XmrWorkflowBranch::Refund)
        .context("select refund branch from finalized Tag-16 evidence")?;
    workflow
        .prepare_step(identity, XmrWorkflowStep::SweepMoneroRefund)
        .context("prepare Maker Monero refund sweep")?;
    ensure!(
        workflow.selected_branch(identity)? == Some(XmrWorkflowBranch::Refund)
            && workflow.load_reconciliation(identity, XmrWorkflowStep::FundMonero)?
                == Some(reconciliation)
            && workflow.step_revision(identity, XmrWorkflowStep::SweepMoneroRefund)? == 0,
        "durable Maker refund activation did not replay exactly"
    );

    let summary = ActivationSummaryV1 {
        schema: ACTIVATION_SCHEMA,
        role: "maker",
        run_id: run_id.to_owned(),
        monero_run_id: monero_run_id.to_owned(),
        swap_id: hex::encode(authority.swap_id()),
        selected_branch: "refund",
        funding_effect_evidence_sha256: hex::encode(effect_evidence_sha256),
        funding_tool_plan_identity_sha256: hex::encode(tool_plan_identity_sha256),
        finalized_refund_sha256: hex::encode(Sha256::digest(&finalized_bytes)),
        finalized_refund_signature_sha256: hex::encode(Sha256::digest(&observed_bytes_after)),
        prepared_step: XmrWorkflowStep::SweepMoneroRefund.name(),
        private_material_disclosed: false,
    };
    let summary_bytes = canonical_json_bytes(&summary, "encode Maker refund activation summary")?;
    std::io::stdout()
        .write_all(&summary_bytes)
        .context("write Maker refund activation summary")?;

    Ok(())
}

fn validate_funding_pair(
    agreement: &XmrAgreementV1,
    monero_run_id: &str,
    funding: &FundingEvidenceV2,
    receipt: &MoneroReceiptEvidenceV2,
) -> Result<()> {
    let transaction_id = funding
        .transaction_id
        .as_deref()
        .ok_or_else(|| anyhow!("confirmed funding evidence omits the transaction ID"))?;
    let confirmation_tip_height = funding
        .confirmation_tip_height
        .ok_or_else(|| anyhow!("confirmed funding evidence omits its confirmation tip"))?;
    let agreement_commitment = hex::encode(agreement.agreement_commitment());
    let genesis_hash = hex::encode(agreement.body().monero().genesis_hash());
    let shared_address = agreement.shared_address().address_string();
    let amount = agreement.body().monero().amount_piconero();
    ensure!(
        funding.schema == FUNDING_SCHEMA
            && funding.attempt_state == "confirmed"
            && funding.agreement_commitment == agreement_commitment
            && funding.shared_address == shared_address
            && funding.amount_piconero == amount
            && funding.required_confirmations == M4_MONERO_CONFIRMATIONS
            && funding.wallet_role == "stage_a_shared_view_only"
            && funding.network_scope == M4_MONERO_NETWORK_SCOPE
            && !funding.public_rpc_used
            && !funding.faucet_used
            && !funding.automatic_submission_retry,
        "Monero funding evidence differs from the exact local Stage-A effect"
    );
    ensure!(
        receipt.schema == M4_MONERO_RECEIPT_SCHEMA
            && receipt.run_id == monero_run_id
            && receipt.agreement_commitment == agreement_commitment
            && receipt.monero_genesis_hash == genesis_hash
            && receipt.transaction_id == transaction_id
            && receipt.destination_address == shared_address
            && receipt.amount_piconero == amount
            && receipt.confirmations >= funding.required_confirmations
            && receipt.stable_tip_height == confirmation_tip_height
            && receipt.stable_tip_height >= receipt.containing_block_height
            && receipt.peer_count == 0
            && receipt.daemon_version == M4_MONERO_DAEMON_VERSION
            && receipt.target_wallet_version == M4_MONERO_WALLET_VERSION
            && receipt.foreign_wallet_version == M4_MONERO_WALLET_VERSION
            && receipt.network_scope == M4_MONERO_NETWORK_SCOPE
            && !receipt.public_rpc_used
            && !receipt.faucet_used,
        "independent Monero funding receipt differs from Stage A or funding evidence"
    );
    let exact_confirmations = receipt
        .stable_tip_height
        .checked_sub(receipt.containing_block_height)
        .and_then(|distance| distance.checked_add(1))
        .ok_or_else(|| anyhow!("Monero funding confirmation heights overflow"))?;
    ensure!(
        receipt.confirmations == exact_confirmations
            && funding.restore_height <= receipt.containing_block_height,
        "Monero funding receipt has inconsistent chain positions"
    );
    let transaction = decode_exact::<32>(transaction_id)?;
    let containing_block = decode_exact::<32>(&receipt.containing_block_hash)?;
    let stable_tip = decode_exact::<32>(&receipt.stable_tip_hash)?;
    ensure!(
        transaction != [0; 32] && containing_block != [0; 32] && stable_tip != [0; 32],
        "Monero funding receipt contains a zero chain identity"
    );
    if receipt.stable_tip_height == receipt.containing_block_height {
        ensure!(
            containing_block == stable_tip,
            "same-height Monero funding block hashes differ"
        );
    }
    Ok(())
}

fn domain_digest(domain: &[u8], parts: &[&[u8]]) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(domain);
    for part in parts {
        digest.update(u64::try_from(part.len()).unwrap_or(u64::MAX).to_be_bytes());
        digest.update(part);
    }
    digest.finalize().into()
}

fn persist_or_validate_private(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Ok(existing) = read_private_input(
        path,
        M4_MONERO_EVIDENCE_MAX_BYTES,
        "existing finalized refund signature",
    ) {
        ensure!(
            existing == bytes,
            "existing finalized refund signature conflicts"
        );
    } else {
        let destination = SecureDestination::new(path, "finalized refund signature evidence")?;
        if let Err(publish_error) = write_bounded_public_new(
            &destination,
            bytes,
            M4_MONERO_EVIDENCE_MAX_BYTES,
            "finalized refund signature evidence",
        ) {
            let existing = read_private_input(
                path,
                M4_MONERO_EVIDENCE_MAX_BYTES,
                "raced finalized refund signature",
            )
            .with_context(|| format!("publish finalized refund signature: {publish_error}"))?;
            ensure!(
                existing == bytes,
                "raced finalized refund signature conflicts"
            );
        }
    }
    ensure!(
        read_private_input(
            path,
            M4_MONERO_EVIDENCE_MAX_BYTES,
            "published finalized refund signature"
        )? == bytes,
        "published finalized refund signature changed"
    );
    Ok(())
}
