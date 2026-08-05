//! Sealed no-argument Maker Tag17 punishment worker.

#![forbid(unsafe_code)]

use std::{
    fs::{self, File, OpenOptions},
    io::{Read as _, Write as _},
    os::unix::fs::{MetadataExt as _, OpenOptionsExt as _, PermissionsExt as _},
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Context as _, Result, bail, ensure};
use lez_bridge_adapter::XmrLezBridgeBindingV3;
use lez_bridge_client::{BridgeClient, BridgeClientConfig, SidecarCapability};
use lez_bridge_protocol::{
    MessageContext, Participant, PrepareNativeXmrPunishV3Request, RequestId, RunId,
    RuntimeDescriptor, SubmissionOutcome, SubmitTransactionRequest,
};
use lez_swap_store::XmrWorkflowStep;
use lez_xmr_swap_sdk::{
    MAX_XMR_ACTIVATION_WIRE_BYTES, MAX_XMR_AGREEMENT_WIRE_BYTES, MoneroPrivateViewKey,
    XmrActivatedAgreementV1, XmrAgreementV1,
};
use rustix::fs::{SealFlags, fcntl_get_seals};
use serde::Serialize;
use xmr_reference_actor::{
    ActorRole, XMR_EFFECT_CAPABILITY_FD, XMR_EFFECT_PRIVATE_VIEW_KEY_FD,
    XMR_EFFECT_PRIVATE_XMR_SHARE_FD, XMR_EFFECT_RUNTIME_FD, XMR_EFFECT_STAGE_A_FD,
    XMR_EFFECT_STAGE_B_FD, XmrEffectChildModeV1, load_xmr_effect_child_plan_fd,
};
use zeroize::{Zeroize as _, Zeroizing};

const MAX_RUNTIME_BYTES: usize = 16 * 1024;
const MAX_SECRET_BYTES: usize = 256;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const ABI: &str = "lez_xmr_tag17_punish_v1";

struct ValidatedInputs {
    sidecar_endpoint: String,
    capability: SidecarCapability,
    runtime: RuntimeDescriptor,
    run_id: RunId,
    prepare_request_id: RequestId,
    binding: XmrLezBridgeBindingV3,
    mode: XmrEffectChildModeV1,
    evidence_path: PathBuf,
}

#[derive(Debug, Serialize)]
struct Tag17Evidence {
    schema: &'static str,
    role: Participant,
    run_id: RunId,
    prepare_request_id: RequestId,
    submission_request_id: RequestId,
    transaction_id: String,
    submission_outcome: SubmissionOutcome,
    prepared_message_hash: String,
    automatic_submission_retry: bool,
    public_rpc_used: bool,
}

#[tokio::main]
async fn main() {
    if let Err(error) = execute().await {
        eprintln!("M7 Maker Tag17 worker failed: {error:#}");
        std::process::exit(1);
    }
}

async fn execute() -> Result<()> {
    ensure!(
        std::env::args_os().len() == 1,
        "Tag17 effect worker accepts no arguments"
    );
    let inputs = validate_inputs()?;
    if inputs.mode == XmrEffectChildModeV1::Preflight {
        ensure!(
            publish(inputs, true).await?.is_none(),
            "Tag17 preflight unexpectedly produced evidence"
        );
        return Ok(());
    }
    let evidence_path = inputs.evidence_path.clone();
    let mut evidence_file = reserve_evidence(&evidence_path)?;
    let evidence = publish(inputs, false)
        .await?
        .context("Tag17 invocation produced no evidence")?;
    let mut bytes = serde_json::to_vec(&evidence).context("encode Tag17 evidence")?;
    bytes.push(b'\n');
    evidence_file
        .write_all(&bytes)
        .context("write Tag17 evidence")?;
    evidence_file.sync_all().context("sync Tag17 evidence")
}

fn validate_inputs() -> Result<ValidatedInputs> {
    ensure_private_share_absent()?;
    let plan = load_xmr_effect_child_plan_fd().context("load Tag17 effect-child plan")?;
    ensure!(
        plan.role() == ActorRole::Maker
            && matches!(
                plan.mode(),
                XmrEffectChildModeV1::Preflight | XmrEffectChildModeV1::Invoke
            )
            && plan.step() == XmrWorkflowStep::PunishLezTag17
            && plan.executable_abi() == ABI,
        "XMR effect child plan differs from the compiled Tag17 route"
    );
    validate_evidence_root(plan.evidence_root())?;
    let runtime: RuntimeDescriptor = serde_json::from_slice(&read_sealed_fd(
        XMR_EFFECT_RUNTIME_FD,
        MAX_RUNTIME_BYTES,
        "Maker runtime",
    )?)
    .context("Maker runtime JSON is invalid")?;
    ensure!(
        runtime.sidecar_role == Participant::Maker,
        "Tag17 requires the Maker runtime"
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
    let view_key = parse_view_key(&view_key_bytes)?;
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
        "Tag17 effect-child application identity changed"
    );
    let binding = XmrLezBridgeBindingV3::new(&agreement, &activation)
        .context("Stage-B binding is invalid")?;
    let run_id = RunId::new(plan.run_id().to_owned()).context("run ID is invalid")?;
    let prepare_request_id = RequestId::new(format!("{}-tag17-prepare-001", plan.run_id()))
        .context("Tag17 prepare request ID is invalid")?;
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
    let capability = parse_capability(&capability_bytes)?;
    Ok(ValidatedInputs {
        sidecar_endpoint: plan.lez_sidecar_url().as_str().to_owned(),
        capability,
        runtime,
        run_id,
        prepare_request_id,
        binding,
        mode: plan.mode(),
        evidence_path: plan
            .evidence_root()
            .join("tag17-punishment-submission.json"),
    })
}

async fn publish(inputs: ValidatedInputs, preflight_only: bool) -> Result<Option<Tag17Evidence>> {
    let client = BridgeClient::connect(BridgeClientConfig::new(
        inputs.sidecar_endpoint,
        inputs.capability,
        inputs.run_id.clone(),
        inputs.runtime.clone(),
        REQUEST_TIMEOUT,
    ))
    .context("authenticated sealed Maker sidecar client is unavailable")?;
    let prepared = client
        .prepare_native_xmr_punish_v3(PrepareNativeXmrPunishV3Request::new(
            MessageContext::new(
                inputs.run_id.clone(),
                inputs.prepare_request_id.clone(),
                Participant::Maker,
            ),
            inputs.runtime.clone(),
            inputs.binding.terms(),
        ))
        .await
        .context("Tag17 preparation failed")?;
    if preflight_only {
        return Ok(None);
    }
    let submission_request_id = prepared.punish.transaction_id.submission_request_id();
    let submitted = client
        .submit_transaction(SubmitTransactionRequest::new(
            MessageContext::new(
                inputs.run_id.clone(),
                submission_request_id.clone(),
                Participant::Maker,
            ),
            inputs.runtime,
            prepared.punish.clone(),
        ))
        .await
        .context("Tag17 exact submission failed")?;
    ensure!(
        submitted.transaction_id == prepared.punish.transaction_id,
        "Tag17 returned a different transaction identity"
    );
    Ok(Some(Tag17Evidence {
        schema: "lez_v02_m7_actual_local_tag17_worker_v1",
        role: Participant::Maker,
        run_id: inputs.run_id,
        prepare_request_id: inputs.prepare_request_id,
        submission_request_id,
        transaction_id: hex::encode(submitted.transaction_id.as_bytes()),
        submission_outcome: submitted.outcome,
        prepared_message_hash: hex::encode(
            prepared.terms.to_input().punish_message_hash.as_bytes(),
        ),
        automatic_submission_retry: false,
        public_rpc_used: false,
    }))
}

fn parse_view_key(bytes: &[u8]) -> Result<MoneroPrivateViewKey> {
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
    let mut scalar = Zeroizing::new([0_u8; 32]);
    hex::decode_to_slice(text.as_bytes(), scalar.as_mut()).context("decode Monero view key")?;
    text.zeroize();
    MoneroPrivateViewKey::from_monero_little_endian(*scalar)
        .context("Monero view key is not a canonical scalar")
}

fn parse_capability(bytes: &[u8]) -> Result<SidecarCapability> {
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

fn ensure_private_share_absent() -> Result<()> {
    match File::open(format!("/proc/self/fd/{XMR_EFFECT_PRIVATE_XMR_SHARE_FD}")) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).context("inspect forbidden Tag17 private-share FD"),
        Ok(_) => bail!("Tag17 worker received forbidden Monero private-share FD"),
    }
}

fn validate_evidence_root(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path).context("inspect Tag17 evidence root")?;
    ensure!(
        metadata.file_type().is_dir()
            && metadata.uid() == rustix::process::getuid().as_raw()
            && metadata.permissions().mode() & 0o7777 == 0o700,
        "Tag17 evidence root is unsafe"
    );
    Ok(())
}

fn reserve_evidence(path: &Path) -> Result<File> {
    if fs::symlink_metadata(path).is_ok() {
        bail!("Tag17 evidence destination already exists");
    }
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .context("reserve Tag17 evidence")
}
