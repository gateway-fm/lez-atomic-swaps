//! Role-correct actual-local M4 tag-15 prepare, complete, and submit driver.

#![forbid(unsafe_code)]

use std::{
    fs::{self, File, OpenOptions},
    io::{Read as _, Write as _},
    os::unix::fs::{MetadataExt as _, OpenOptionsExt as _, PermissionsExt as _},
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Context as _, Result, bail, ensure};
use clap::Parser;
use lez_adaptor_role_runner::{
    ValidatedSession, read_final_signature_packet, read_final_signature_packet_bytes,
};
use lez_bridge_adapter::{
    CapabilityFileBridgeClientFactory, FreshLezBridgeTransportFactory as _, XmrLezBridgeBindingV3,
};
use lez_bridge_client::{BridgeClient, BridgeClientConfig, SidecarCapability};
use lez_bridge_protocol::{
    AggregateBip340Signature, CompleteNativeXmrClaimV3Request, MessageContext, Participant,
    PrepareNativeXmrClaimV3Request, PreparedTransaction, RequestId, RunId, RuntimeDescriptor,
    SubmissionOutcome, SubmitTransactionRequest,
};
use lez_swap_store::XmrWorkflowStep;
use lez_xmr_swap_sdk::{
    MAX_XMR_ACTIVATION_WIRE_BYTES, MAX_XMR_AGREEMENT_WIRE_BYTES, MoneroPrivateViewKey,
    XmrActivatedAgreementV1, XmrAgreementV1,
};
use rustix::fs::{SealFlags, fcntl_get_seals};
use serde::Serialize;
use xmr_reference_actor::{
    ActorRole, XMR_EFFECT_CAPABILITY_FD, XMR_EFFECT_FINALIZED_SIGNATURE_FD,
    XMR_EFFECT_PRIVATE_VIEW_KEY_FD, XMR_EFFECT_RUNTIME_FD, XMR_EFFECT_STAGE_A_FD,
    XMR_EFFECT_STAGE_B_FD, XmrEffectChildModeV1, load_xmr_effect_child_plan_fd_for,
};
use zeroize::{Zeroize as _, Zeroizing};

const MAX_RUNTIME_BYTES: usize = 16 * 1024;
const MAX_SECRET_BYTES: usize = 256;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Validate the Maker packet and submit one exact completed tag-15 Claim.
#[derive(Debug, Parser)]
#[command(version, about)]
struct Arguments {
    #[arg(long)]
    sidecar_endpoint: String,
    #[arg(long)]
    capability_file: PathBuf,
    #[arg(long)]
    runtime_file: PathBuf,
    #[arg(long)]
    agreement_wire_file: PathBuf,
    #[arg(long)]
    activation_wire_file: PathBuf,
    #[arg(long)]
    monero_view_key_file: PathBuf,
    #[arg(long)]
    final_signature_file: PathBuf,
    #[arg(long)]
    run_id: String,
    #[arg(long)]
    prepare_request_id: String,
    #[arg(long)]
    complete_request_id: String,
    #[arg(long)]
    output_evidence: PathBuf,
}

#[derive(Debug, Serialize)]
struct Tag15Evidence {
    schema: &'static str,
    role: Participant,
    run_id: RunId,
    swap_id: String,
    prepare_request_id: RequestId,
    complete_request_id: RequestId,
    submission_request_id: RequestId,
    transaction_id: String,
    exact_transaction: PreparedTransaction,
    submission_outcome: SubmissionOutcome,
    prepared_message_hash: String,
    automatic_submission_retry: bool,
    public_rpc_used: bool,
}

struct ValidatedInputs {
    sidecar_endpoint: String,
    capability: CapabilitySource,
    runtime: RuntimeDescriptor,
    run_id: RunId,
    prepare_request_id: RequestId,
    complete_request_id: RequestId,
    swap_id: String,
    binding: XmrLezBridgeBindingV3,
    signature: [u8; 64],
}

enum CapabilitySource {
    PrivateFile(PathBuf),
    SealedDescriptor(SidecarCapability),
}

#[tokio::main]
async fn main() {
    let result = if std::env::args_os().len() == 1 {
        Box::pin(execute_effect_child()).await
    } else {
        execute(Arguments::parse()).await
    };
    if let Err(error) = result {
        eprintln!("M4 Maker tag-15 publication failed: {error:#}");
        std::process::exit(1);
    }
}

async fn execute_effect_child() -> Result<()> {
    let (inputs, output) = validate_effect_child_inputs()?;
    let mut evidence_file = reserve_evidence(&output)?;
    let evidence = publish_tag15(inputs).await?;
    write_evidence(&mut evidence_file, &evidence)
}

fn validate_effect_child_inputs() -> Result<(ValidatedInputs, PathBuf)> {
    let plan = load_xmr_effect_child_plan_fd_for(
        ActorRole::Maker,
        XmrEffectChildModeV1::Invoke,
        XmrWorkflowStep::ClaimLezTag15,
        "lez_xmr_tag15_claim_v1",
    )
    .context("load Tag15 effect-child plan")?;
    validate_evidence_root(plan.evidence_root())?;
    let runtime: RuntimeDescriptor = serde_json::from_slice(&read_sealed_fd(
        XMR_EFFECT_RUNTIME_FD,
        MAX_RUNTIME_BYTES,
        "Maker runtime",
    )?)
    .context("Maker runtime JSON is invalid")?;
    ensure!(
        runtime.sidecar_role == Participant::Maker,
        "tag 15 requires the Maker runtime"
    );
    let agreement = XmrAgreementV1::from_wire(&read_sealed_fd(
        XMR_EFFECT_STAGE_A_FD,
        MAX_XMR_AGREEMENT_WIRE_BYTES,
        "Stage-A agreement",
    )?)
    .context("Stage-A agreement is invalid")?;
    let view_key_bytes = Zeroizing::new(read_sealed_fd(
        XMR_EFFECT_PRIVATE_VIEW_KEY_FD,
        MAX_SECRET_BYTES,
        "Monero view key",
    )?);
    let view_key = read_view_key_bytes(&view_key_bytes)?;
    let activation = XmrActivatedAgreementV1::from_wire(
        &agreement,
        &read_sealed_fd(
            XMR_EFFECT_STAGE_B_FD,
            MAX_XMR_ACTIVATION_WIRE_BYTES,
            "Stage-B activation",
        )?,
        &view_key,
    )
    .context("Stage-B activation is invalid")?;
    ensure!(
        agreement.body().swap_id() == plan.swap_id()
            && agreement.agreement_commitment() == plan.agreement_commitment()
            && activation.activation_commitment() == plan.activation_commitment(),
        "Tag15 effect-child application identity changed"
    );
    let binding = XmrLezBridgeBindingV3::new(&agreement, &activation)
        .context("Stage-B LEZ binding is invalid")?;
    let session = ValidatedSession::from_untweaked_context(
        agreement
            .claim_session_descriptor()
            .context()
            .context("claim session descriptor is invalid")?,
    )
    .context("claim session is invalid")?;
    let signature_packet = Zeroizing::new(read_sealed_fd(
        XMR_EFFECT_FINALIZED_SIGNATURE_FD,
        4 * 1024,
        "Maker claim final signature",
    )?);
    let signature = read_final_signature_packet_bytes(&signature_packet, &session)
        .context("canonical Maker final-signature packet is invalid")?;
    let run_id = RunId::new(plan.run_id().to_owned()).context("run ID is invalid")?;
    let prepare_request_id = RequestId::new(format!("{}-tag15-prepare-001", plan.run_id()))
        .context("prepare request ID is invalid")?;
    let complete_request_id = RequestId::new(format!("{}-tag15-complete-001", plan.run_id()))
        .context("complete request ID is invalid")?;
    binding
        .terms()
        .validate_runtime_binding(
            &MessageContext::new(
                run_id.clone(),
                prepare_request_id.clone(),
                Participant::Maker,
            ),
            &runtime,
        )
        .context("Maker runtime is not bound by Stage B")?;
    let capability_bytes = Zeroizing::new(read_sealed_fd(
        XMR_EFFECT_CAPABILITY_FD,
        MAX_SECRET_BYTES,
        "Maker LEZ capability",
    )?);
    let capability = parse_capability_bytes(&capability_bytes)?;
    let output = plan.evidence_root().join("tag15-claim-submission.json");
    Ok((
        ValidatedInputs {
            sidecar_endpoint: plan.lez_sidecar_url().as_str().to_owned(),
            capability: CapabilitySource::SealedDescriptor(capability),
            runtime,
            run_id,
            prepare_request_id,
            complete_request_id,
            swap_id: hex::encode(plan.swap_id()),
            binding,
            signature,
        },
        output,
    ))
}

async fn execute(arguments: Arguments) -> Result<()> {
    let output_evidence = arguments.output_evidence.clone();
    let inputs = validate_inputs(arguments)?;
    let mut evidence_file = reserve_evidence(&output_evidence)?;
    let evidence = publish_tag15(inputs).await?;
    write_evidence(&mut evidence_file, &evidence)?;
    println!(
        "{}",
        serde_json::to_string(&evidence).context("encode tag-15 report")?
    );
    Ok(())
}

fn validate_inputs(arguments: Arguments) -> Result<ValidatedInputs> {
    let runtime: RuntimeDescriptor = serde_json::from_slice(&read_bounded_file(
        &arguments.runtime_file,
        MAX_RUNTIME_BYTES,
        false,
        "Maker runtime",
    )?)
    .context("Maker runtime JSON is invalid")?;
    ensure!(
        runtime.sidecar_role == Participant::Maker,
        "tag 15 requires the Maker runtime"
    );
    let run_id = RunId::new(arguments.run_id).context("run ID is invalid")?;
    let prepare_request_id =
        RequestId::new(arguments.prepare_request_id).context("prepare request ID is invalid")?;
    let complete_request_id =
        RequestId::new(arguments.complete_request_id).context("complete request ID is invalid")?;
    ensure!(
        prepare_request_id != complete_request_id,
        "prepare and complete request IDs must be distinct"
    );
    let agreement = XmrAgreementV1::from_wire(&read_bounded_file(
        &arguments.agreement_wire_file,
        MAX_XMR_AGREEMENT_WIRE_BYTES,
        false,
        "Stage-A agreement",
    )?)
    .context("Stage-A agreement is invalid")?;
    let view_key = read_view_key(&arguments.monero_view_key_file)?;
    let activation = XmrActivatedAgreementV1::from_wire(
        &agreement,
        &read_bounded_file(
            &arguments.activation_wire_file,
            MAX_XMR_ACTIVATION_WIRE_BYTES,
            false,
            "Stage-B activation",
        )?,
        &view_key,
    )
    .context("Stage-B activation is invalid")?;
    let swap_id = hex::encode(agreement.body().swap_id());
    let binding = XmrLezBridgeBindingV3::new(&agreement, &activation)
        .context("Stage-B LEZ binding is invalid")?;
    let session = ValidatedSession::from_untweaked_context(
        agreement
            .claim_session_descriptor()
            .context()
            .context("claim session descriptor is invalid")?,
    )
    .context("claim session is invalid")?;
    let signature = read_final_signature_packet(&arguments.final_signature_file, &session)
        .context("canonical Maker final-signature packet is invalid")?;
    let prepare_context = MessageContext::new(
        run_id.clone(),
        prepare_request_id.clone(),
        Participant::Maker,
    );
    binding
        .terms()
        .validate_runtime_binding(&prepare_context, &runtime)
        .context("Maker runtime is not bound by Stage B")?;

    Ok(ValidatedInputs {
        sidecar_endpoint: arguments.sidecar_endpoint,
        capability: CapabilitySource::PrivateFile(arguments.capability_file),
        runtime,
        run_id,
        prepare_request_id,
        complete_request_id,
        swap_id,
        binding,
        signature,
    })
}

async fn publish_tag15(inputs: ValidatedInputs) -> Result<Tag15Evidence> {
    let ValidatedInputs {
        sidecar_endpoint,
        capability,
        runtime,
        run_id,
        prepare_request_id,
        complete_request_id,
        swap_id,
        binding,
        signature,
    } = inputs;
    let prepare_context = MessageContext::new(
        run_id.clone(),
        prepare_request_id.clone(),
        Participant::Maker,
    );
    let client = match capability {
        CapabilitySource::PrivateFile(path) => CapabilityFileBridgeClientFactory::new(
            sidecar_endpoint,
            path,
            run_id.clone(),
            runtime.clone(),
            REQUEST_TIMEOUT,
        )
        .fresh_transport()
        .context("authenticated Maker sidecar client is unavailable")?,
        CapabilitySource::SealedDescriptor(capability) => {
            BridgeClient::connect(BridgeClientConfig::new(
                sidecar_endpoint,
                capability,
                run_id.clone(),
                runtime.clone(),
                REQUEST_TIMEOUT,
            ))
            .context("authenticated sealed Maker sidecar client is unavailable")?
        }
    };
    let prepared = client
        .prepare_native_xmr_claim_v3(PrepareNativeXmrClaimV3Request::new(
            prepare_context,
            runtime.clone(),
            binding.terms(),
        ))
        .await
        .context("tag-15 preparation failed")?;
    let completed = client
        .complete_native_xmr_claim_v3(
            CompleteNativeXmrClaimV3Request::new(
                MessageContext::new(
                    run_id.clone(),
                    complete_request_id.clone(),
                    Participant::Maker,
                ),
                runtime.clone(),
                binding.terms(),
                prepared.claim.clone(),
                AggregateBip340Signature::from_bytes(signature),
            )
            .context("tag-15 completion request is invalid")?,
        )
        .await
        .context("tag-15 completion failed")?;
    let submission_request_id = completed.claim.transaction_id.submission_request_id();
    let submitted = client
        .submit_transaction(SubmitTransactionRequest::new(
            MessageContext::new(
                run_id.clone(),
                submission_request_id.clone(),
                Participant::Maker,
            ),
            runtime,
            completed.claim.clone(),
        ))
        .await
        .context("tag-15 exact submission failed")?;
    ensure!(
        submitted.transaction_id == completed.claim.transaction_id,
        "tag-15 returned a different transaction identity"
    );
    Ok(Tag15Evidence {
        schema: "lez_v02_m4_actual_local_tag15_v1",
        role: Participant::Maker,
        run_id,
        swap_id,
        prepare_request_id,
        complete_request_id,
        submission_request_id,
        transaction_id: hex::encode(submitted.transaction_id.as_bytes()),
        exact_transaction: completed.claim,
        submission_outcome: submitted.outcome,
        prepared_message_hash: hex::encode(prepared.claim.message_hash.as_bytes()),
        automatic_submission_retry: false,
        public_rpc_used: false,
    })
}

fn write_evidence(file: &mut File, evidence: &Tag15Evidence) -> Result<()> {
    let mut bytes = serde_json::to_vec(evidence).context("encode tag-15 evidence")?;
    bytes.push(b'\n');
    file.write_all(&bytes).context("write tag-15 evidence")?;
    file.sync_all().context("sync tag-15 evidence")?;
    Ok(())
}

fn read_view_key(path: &Path) -> Result<MoneroPrivateViewKey> {
    read_view_key_bytes(&read_bounded_file(
        path,
        MAX_SECRET_BYTES,
        true,
        "Monero view key",
    )?)
}

fn read_view_key_bytes(bytes: &[u8]) -> Result<MoneroPrivateViewKey> {
    let mut text =
        Zeroizing::new(String::from_utf8(bytes.to_vec()).context("Monero view key is not UTF-8")?);
    while text.ends_with(['\n', '\r']) {
        text.pop();
    }
    ensure!(
        text.len() == 64
            && text
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "Monero view key is not exact lowercase hex"
    );
    let mut bytes = Zeroizing::new([0_u8; 32]);
    hex::decode_to_slice(text.as_bytes(), &mut *bytes).context("decode Monero view key")?;
    text.zeroize();
    MoneroPrivateViewKey::from_monero_little_endian(*bytes)
        .context("Monero view key is not a canonical scalar")
}

fn parse_capability_bytes(bytes: &[u8]) -> Result<SidecarCapability> {
    let mut text = Zeroizing::new(
        String::from_utf8(bytes.to_vec()).context("Maker LEZ capability is not UTF-8")?,
    );
    if text.ends_with("\r\n") {
        let content_len = text.len() - 2;
        text.truncate(content_len);
    } else if text.ends_with('\n') {
        text.pop();
    }
    SidecarCapability::new(text.as_str().to_owned()).context("Maker LEZ capability is invalid")
}

fn read_sealed_fd(fd: i32, maximum: usize, label: &'static str) -> Result<Vec<u8>> {
    let mut file =
        File::open(format!("/proc/self/fd/{fd}")).with_context(|| format!("open {label} FD"))?;
    let metadata = file
        .metadata()
        .with_context(|| format!("inspect {label} FD"))?;
    let required = SealFlags::SEAL | SealFlags::SHRINK | SealFlags::GROW | SealFlags::WRITE;
    ensure!(
        metadata.file_type().is_file()
            && metadata.permissions().mode() & 0o7777 == 0o400
            && fcntl_get_seals(&file)
                .with_context(|| format!("inspect {label} seals"))?
                .contains(required),
        "{label} FD is unsafe"
    );
    let mut bytes = Vec::new();
    std::io::Read::by_ref(&mut file)
        .take(u64::try_from(maximum).unwrap_or(u64::MAX) + 1)
        .read_to_end(&mut bytes)
        .with_context(|| format!("read {label} FD"))?;
    ensure!(
        bytes.len() <= maximum && metadata.len() == u64::try_from(bytes.len()).unwrap_or(u64::MAX),
        "{label} FD is oversized or changed"
    );
    Ok(bytes)
}

fn validate_evidence_root(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path).context("inspect Tag15 evidence root")?;
    ensure!(
        metadata.file_type().is_dir()
            && metadata.uid() == rustix::process::getuid().as_raw()
            && metadata.permissions().mode() & 0o7777 == 0o700,
        "Tag15 evidence root is unsafe"
    );
    Ok(())
}

fn read_bounded_file(
    path: &Path,
    maximum: usize,
    private: bool,
    label: &'static str,
) -> Result<Vec<u8>> {
    let before = fs::symlink_metadata(path).with_context(|| format!("inspect {label}"))?;
    ensure!(
        before.file_type().is_file()
            && before.nlink() == 1
            && (!private
                || (before.uid() == rustix::process::getuid().as_raw()
                    && before.permissions().mode().trailing_zeros() >= 6)),
        "{label} is unsafe"
    );
    let mut file = File::open(path).with_context(|| format!("open {label}"))?;
    let mut bytes = Vec::new();
    std::io::Read::by_ref(&mut file)
        .take(u64::try_from(maximum).unwrap_or(u64::MAX).saturating_add(1))
        .read_to_end(&mut bytes)
        .with_context(|| format!("read {label}"))?;
    ensure!(bytes.len() <= maximum, "{label} is oversized");
    let after = file.metadata().with_context(|| format!("restat {label}"))?;
    ensure!(
        before.dev() == after.dev()
            && before.ino() == after.ino()
            && before.len() == after.len()
            && usize::try_from(after.len()).ok() == Some(bytes.len()),
        "{label} changed while read"
    );
    Ok(bytes)
}

fn reserve_evidence(path: &Path) -> Result<File> {
    if fs::symlink_metadata(path).is_ok() {
        bail!("tag-15 evidence destination already exists");
    }
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .context("reserve tag-15 evidence")
}
