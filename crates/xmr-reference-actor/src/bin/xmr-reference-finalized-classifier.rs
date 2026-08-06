//! Sealed read-only observer for Taker Tag14 finality.

#![forbid(unsafe_code)]

use std::{
    fs::{self, File, OpenOptions},
    io::{Read as _, Write as _},
    os::unix::fs::{MetadataExt as _, OpenOptionsExt as _, PermissionsExt as _},
    path::Path,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context as _, Result, ensure};
use lez_bridge_adapter::{
    CapabilityFileBridgeClientFactory, FreshLezBridgeTransportFactory as _, XmrLezBridgeBindingV3,
};
use lez_bridge_protocol::{
    ClassifyFinalizedNativeXmrEffectV3Request, ClassifyFinalizedNativeXmrEffectV3Result,
    DiscoveryWindow, FinalizedNativeXmrScanOutcomeV3, FinalizedNativeXmrTransactionTargetV3,
    MessageContext, Participant, RequestId, RunId, RuntimeDescriptor, XmrNativeEffectV3,
};
use lez_swap_store::XmrWorkflowStep;
use lez_xmr_swap_sdk::{
    MAX_XMR_ACTIVATION_WIRE_BYTES, MAX_XMR_AGREEMENT_WIRE_BYTES, MoneroPrivateViewKey,
    XmrActivatedAgreementV1, XmrAgreementV1,
};
use rustix::fs::{CWD, OFlags, RenameFlags, SealFlags, fcntl_get_seals, open, renameat_with};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use xmr_reference_actor::{
    ActorRole, XMR_EFFECT_CAPABILITY_FD, XMR_EFFECT_PRIVATE_VIEW_KEY_FD, XMR_EFFECT_RUNTIME_FD,
    XMR_EFFECT_STAGE_A_FD, XMR_EFFECT_STAGE_B_FD, XmrEffectChildModeV1,
    load_xmr_effect_child_plan_fd,
};
use zeroize::Zeroizing;

const ABI: &str = "lez_xmr_finalized_classifier_v1";
const ACTIVATION_FILE: &str = "taker-claim-activation.json";
const FINAL_EVIDENCE_FILE: &str = "tag14-finalized.json";
const MAX_SECRET_BYTES: usize = 256;
const MAX_RUNTIME_BYTES: usize = 16 * 1024;
const MAX_EVIDENCE_BYTES: u64 = 2 * 1024 * 1024;
const MAX_SCAN_BLOCKS: u32 = 16;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ActivationEvidenceV1 {
    schema: String,
    role: String,
    run_id: String,
    monero_run_id: String,
    swap_id: String,
    selected_branch: String,
    tag13_evidence_sha256: String,
    initialization_effect_evidence_sha256: String,
    initialization_tool_plan_identity_sha256: String,
    funding_effect_evidence_sha256: String,
    funding_tool_plan_identity_sha256: String,
    monero_funding_evidence_sha256: String,
    monero_funding_receipt_sha256: String,
    tag14_scan_start_height: u64,
    prepared_step: String,
    private_material_disclosed: bool,
}

#[tokio::main]
async fn main() {
    if let Err(error) = execute().await {
        eprintln!("M7 finalized Tag14 observation failed: {error:#}");
        std::process::exit(1);
    }
}

#[allow(
    clippy::too_many_lines,
    reason = "one visible observer boundary validates every sealed authority before RPC"
)]
async fn execute() -> Result<()> {
    validate_args()?;
    let plan = load_xmr_effect_child_plan_fd().context("load Tag14 observer child plan")?;
    ensure!(
        plan.role() == ActorRole::Taker
            && plan.mode() == XmrEffectChildModeV1::Observe
            && plan.step() == XmrWorkflowStep::AuthorizeLezTag14
            && plan.executable_abi() == ABI,
        "XMR effect child plan differs from the compiled Tag14 observer route"
    );
    validate_evidence_root(plan.evidence_root())?;
    let runtime: RuntimeDescriptor = serde_json::from_slice(&read_sealed_fd(
        XMR_EFFECT_RUNTIME_FD,
        MAX_RUNTIME_BYTES,
        "Taker runtime",
    )?)
    .context("Taker runtime is invalid")?;
    ensure!(
        runtime.sidecar_role == Participant::Taker,
        "Tag14 observer runtime has the wrong role"
    );
    let agreement = XmrAgreementV1::from_wire(&read_sealed_fd(
        XMR_EFFECT_STAGE_A_FD,
        MAX_XMR_AGREEMENT_WIRE_BYTES,
        "Stage-A agreement",
    )?)
    .context("Stage-A agreement is invalid")?;
    let view_key = parse_view_key(&read_sealed_fd(
        XMR_EFFECT_PRIVATE_VIEW_KEY_FD,
        MAX_SECRET_BYTES,
        "Monero view key",
    )?)?;
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
        "Tag14 observer application identity changed"
    );
    let binding = XmrLezBridgeBindingV3::new(&agreement, &activation)
        .context("derive Tag14 observer terms")?;
    let activation_path = plan.evidence_root().join(ACTIVATION_FILE);
    let activation_evidence: ActivationEvidenceV1 =
        read_canonical_private_json(&activation_path, "Taker claim activation evidence")?;
    ensure!(
        activation_evidence.schema == "lez_v02_m7_taker_claim_activation_v1"
            && activation_evidence.role == "taker"
            && activation_evidence.run_id == plan.run_id()
            && activation_evidence.swap_id == hex::encode(plan.swap_id())
            && activation_evidence.selected_branch == "claim"
            && activation_evidence.prepared_step == XmrWorkflowStep::AuthorizeLezTag14.name()
            && !activation_evidence.private_material_disclosed,
        "Tag14 observer activation evidence differs from the exact plan"
    );
    let run_id = RunId::new(plan.run_id().to_owned()).context("invalid Tag14 observer run ID")?;
    let observation_nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock predates the Unix epoch")?
        .as_nanos();
    let request_id = RequestId::new(format!(
        "m7-tag14-observe-{observation_nonce}-{}",
        std::process::id()
    ))
    .context("invalid Tag14 observer request ID")?;
    let context = MessageContext::new(run_id.clone(), request_id, Participant::Taker);
    binding
        .terms()
        .validate_runtime_binding(&context, &runtime)
        .context("Tag14 terms do not bind the selected Taker runtime")?;
    let capability = format!("/proc/self/fd/{XMR_EFFECT_CAPABILITY_FD}");
    let client = CapabilityFileBridgeClientFactory::new(
        plan.lez_sidecar_url().as_str(),
        capability,
        run_id,
        runtime.clone(),
        REQUEST_TIMEOUT,
    )
    .fresh_transport()
    .context("authenticated Taker sidecar client is unavailable")?;
    let result = client
        .classify_finalized_native_xmr_effect_v3(ClassifyFinalizedNativeXmrEffectV3Request::new(
            context,
            runtime,
            binding.terms(),
            XmrNativeEffectV3::AuthorizeClaim,
            FinalizedNativeXmrTransactionTargetV3::DiscoverByTerms {},
            DiscoveryWindow::new(activation_evidence.tag14_scan_start_height, MAX_SCAN_BLOCKS)
                .context("invalid Tag14 finalized scan window")?,
        ))
        .await
        .context("typed finalized Tag14 classification failed")?;
    let FinalizedNativeXmrScanOutcomeV3::Found { .. } = &result.outcome else {
        println!("{{\"schema_version\":1,\"step\":\"authorize_lez_tag14\",\"state\":\"pending\"}}");
        return Ok(());
    };
    let evidence_bytes = canonical_line(&result).context("encode finalized Tag14 evidence")?;
    persist_or_validate(
        &plan.evidence_root().join(FINAL_EVIDENCE_FILE),
        &evidence_bytes,
    )?;
    println!(
        "{{\"schema_version\":1,\"step\":\"authorize_lez_tag14\",\"state\":\"finalized\",\"effect_evidence_sha256\":\"{}\"}}",
        hex::encode(Sha256::digest(&evidence_bytes))
    );
    Ok(())
}

fn validate_args() -> Result<()> {
    let args = std::env::args_os().collect::<Vec<_>>();
    ensure!(
        args.len() == 3
            && args[1] == "--xmr-workflow-step"
            && args[2] == XmrWorkflowStep::AuthorizeLezTag14.name(),
        "finalized classifier requires the parent-selected Tag14 step"
    );
    Ok(())
}

fn parse_view_key(bytes: &[u8]) -> Result<MoneroPrivateViewKey> {
    let mut text =
        Zeroizing::new(String::from_utf8(bytes.to_vec()).context("view key is not UTF-8")?);
    while text.ends_with(['\n', '\r']) {
        text.pop();
    }
    ensure!(
        text.len() == 64
            && text
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "view key is not canonical lowercase hex"
    );
    let mut scalar = [0_u8; 32];
    hex::decode_to_slice(text.as_bytes(), &mut scalar).context("decode Monero view key")?;
    ensure!(scalar != [0; 32], "Monero view key is zero");
    MoneroPrivateViewKey::from_monero_little_endian(scalar)
        .context("Monero view key is not a canonical scalar")
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
    let metadata = fs::symlink_metadata(path).context("inspect Tag14 evidence root")?;
    ensure!(
        metadata.file_type().is_dir()
            && metadata.uid() == rustix::process::getuid().as_raw()
            && metadata.permissions().mode() & 0o7777 == 0o700,
        "Tag14 evidence root is unsafe"
    );
    Ok(())
}

fn read_canonical_private_json<T>(path: &Path, label: &'static str) -> Result<T>
where
    T: for<'de> Deserialize<'de> + Serialize,
{
    let fd = open(
        path,
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        rustix::fs::Mode::empty(),
    )
    .with_context(|| format!("open {label}"))?;
    let mut file = File::from(fd);
    let metadata = file
        .metadata()
        .with_context(|| format!("inspect {label}"))?;
    ensure!(
        metadata.file_type().is_file()
            && metadata.uid() == rustix::process::getuid().as_raw()
            && metadata.permissions().mode() & 0o7777 == 0o600
            && metadata.len() <= MAX_EVIDENCE_BYTES,
        "{label} is unsafe"
    );
    let mut bytes = Vec::new();
    std::io::Read::by_ref(&mut file)
        .take(MAX_EVIDENCE_BYTES + 1)
        .read_to_end(&mut bytes)
        .with_context(|| format!("read {label}"))?;
    ensure!(
        bytes.len() as u64 == metadata.len(),
        "{label} changed or is oversized"
    );
    let value: T =
        serde_json::from_slice(&bytes).with_context(|| format!("{label} is malformed"))?;
    ensure!(canonical_line(&value)? == bytes, "{label} is noncanonical");
    Ok(value)
}

fn canonical_line(value: &impl Serialize) -> Result<Vec<u8>> {
    let mut bytes = serde_json::to_vec(value).context("encode canonical JSON")?;
    bytes.push(b'\n');
    Ok(bytes)
}

fn persist_or_validate(path: &Path, bytes: &[u8]) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(_) => return validate_existing_evidence(path, bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error).context("inspect existing Tag14 evidence"),
    }
    let parent = path
        .parent()
        .context("Tag14 finality evidence has no parent")?;
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .context("Tag14 finality evidence name is invalid")?;
    let staging = parent.join(format!(".{name}.tmp-{}", std::process::id()));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&staging)
        .context("reserve Tag14 finality staging evidence")?;
    file.write_all(bytes)
        .context("write Tag14 finality staging evidence")?;
    file.sync_all()
        .context("sync Tag14 finality staging evidence")?;
    drop(file);
    match renameat_with(CWD, &staging, CWD, path, RenameFlags::NOREPLACE) {
        Ok(()) => File::open(parent)
            .context("open Tag14 evidence directory")?
            .sync_all()
            .context("sync Tag14 evidence directory"),
        Err(rustix::io::Errno::EXIST) => {
            fs::remove_file(&staging).context("remove redundant Tag14 staging evidence")?;
            validate_existing_evidence(path, bytes)
        }
        Err(error) => {
            let _ = fs::remove_file(&staging);
            Err(error).context("publish Tag14 finality evidence atomically")
        }
    }
}

fn validate_existing_evidence(path: &Path, bytes: &[u8]) -> Result<()> {
    let existing: ClassifyFinalizedNativeXmrEffectV3Result =
        read_canonical_private_json(path, "finalized Tag14 evidence")?;
    ensure!(
        canonical_line(&existing)? == bytes,
        "durable Tag14 finality evidence changed"
    );
    Ok(())
}
