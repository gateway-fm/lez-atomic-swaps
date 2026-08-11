//! Sealed read-only Maker observer for finalized Tag15.

#![forbid(unsafe_code)]

use std::{
    fs::{self, File, OpenOptions},
    io::{Read as _, Write as _},
    os::unix::fs::{MetadataExt as _, OpenOptionsExt as _, PermissionsExt as _},
    path::Path,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context as _, Result, ensure};
use lez_bridge_adapter::XmrLezBridgeBindingV3;
use lez_bridge_client::{BridgeClient, BridgeClientConfig, SidecarCapability};
use lez_bridge_protocol::{
    ClassifyFinalizedNativeXmrEffectV3Request, ClassifyFinalizedNativeXmrEffectV3Result,
    DiscoveryWindow, FinalizedNativeXmrScanOutcomeV3, FinalizedNativeXmrTransactionTargetV3,
    MAX_DISCOVERY_BLOCKS, MessageContext, ObserveFinalizedClockRequest, Participant, RequestId,
    RunId, RuntimeDescriptor, XmrNativeEffectV3, XmrNativeEscrowTermsV3,
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
    XMR_EFFECT_STAGE_A_FD, XMR_EFFECT_STAGE_B_FD, XmrEffectChildModeV1, XmrEffectChildPlanV1,
    load_xmr_effect_child_plan_fd_for,
};
use zeroize::{Zeroize as _, Zeroizing};

const ABI: &str = "lez_xmr_finalized_classifier_v1";
const ACTIVATION_FILE: &str = "maker-claim-activation.json";
const FINAL_EVIDENCE_FILE: &str = "tag15-finalized.json";
const MAX_SECRET_BYTES: usize = 256;
const MAX_CAPABILITY_FILE_BYTES: usize = 130;
const MAX_RUNTIME_BYTES: usize = 16 * 1024;
const MAX_EVIDENCE_BYTES: u64 = 2 * 1024 * 1024;
const MAX_SCAN_BLOCKS: u32 = 16;
const MAX_SCAN_PAGES: u32 = MAX_DISCOVERY_BLOCKS / MAX_SCAN_BLOCKS;
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
    funding_effect_evidence_sha256: String,
    funding_tool_plan_identity_sha256: String,
    finalized_authorization_sha256: String,
    maker_claim_signature_sha256: String,
    tag15_scan_start_height: u64,
    prepared_step: String,
    private_material_disclosed: bool,
}

#[tokio::main]
async fn main() {
    if let Err(error) = execute().await {
        eprintln!("M7 finalized Tag15 observation failed: {error:#}");
        std::process::exit(1);
    }
}

async fn execute() -> Result<()> {
    validate_args()?;
    let plan = load_xmr_effect_child_plan_fd_for(
        ActorRole::Maker,
        XmrEffectChildModeV1::Observe,
        XmrWorkflowStep::ClaimLezTag15,
        ABI,
    )
    .context("load Tag15 observer child plan")?;
    validate_evidence_root(plan.evidence_root())?;
    let runtime: RuntimeDescriptor = serde_json::from_slice(&read_sealed_fd(
        XMR_EFFECT_RUNTIME_FD,
        MAX_RUNTIME_BYTES,
        "Maker runtime",
    )?)
    .context("Maker runtime is invalid")?;
    ensure!(
        runtime.sidecar_role == Participant::Maker,
        "Tag15 observer runtime has the wrong role"
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
        "Tag15 observer application identity changed"
    );
    let binding = XmrLezBridgeBindingV3::new(&agreement, &activation)
        .context("derive Tag15 observer terms")?;
    let activation_evidence = load_activation_evidence(&plan)?;
    let run_id = RunId::new(plan.run_id().to_owned()).context("invalid Tag15 observer run ID")?;
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock predates the Unix epoch")?
        .as_nanos();
    let capability = read_sidecar_capability_fd(XMR_EFFECT_CAPABILITY_FD)?;
    let client = BridgeClient::connect(BridgeClientConfig::new(
        plan.lez_sidecar_url().as_str(),
        capability,
        run_id.clone(),
        runtime.clone(),
        REQUEST_TIMEOUT,
    ))
    .context("authenticated Maker sidecar client is unavailable")?;
    let terms = binding.terms();
    let Some(result) = classify_finalized_tag15_pages(
        &client,
        &run_id,
        &runtime,
        &terms,
        activation_evidence.tag15_scan_start_height,
        nonce,
    )
    .await?
    else {
        println!("{{\"schema_version\":1,\"step\":\"claim_lez_tag15\",\"state\":\"pending\"}}");
        return Ok(());
    };
    let evidence_sha256 = persist_finalized_result(&result, plan.evidence_root())?
        .context("found Tag15 result did not produce finality evidence")?;
    println!(
        "{{\"schema_version\":1,\"step\":\"claim_lez_tag15\",\"state\":\"finalized\",\"effect_evidence_sha256\":\"{evidence_sha256}\"}}"
    );
    Ok(())
}

async fn classify_finalized_tag15_pages(
    client: &BridgeClient,
    run_id: &RunId,
    runtime: &RuntimeDescriptor,
    terms: &XmrNativeEscrowTermsV3,
    initial_scan_height: u64,
    nonce: u128,
) -> Result<Option<ClassifyFinalizedNativeXmrEffectV3Result>> {
    let mut scan_start_height = initial_scan_height;
    for page_index in 0..MAX_SCAN_PAGES {
        let clock_request_id = RequestId::new(format!(
            "m7-tag15-clock-{nonce}-{}-{page_index}",
            std::process::id()
        ))
        .context("invalid Tag15 clock request ID")?;
        let clock_context =
            MessageContext::new(run_id.clone(), clock_request_id, Participant::Maker);
        let finalized_clock = client
            .observe_finalized_clock(ObserveFinalizedClockRequest::new(
                clock_context,
                runtime.clone(),
            ))
            .await
            .context("typed finalized Tag15 clock observation failed")?
            .clock;
        let Some(page_blocks) = scan_page_blocks(scan_start_height, finalized_clock.height)? else {
            break;
        };
        let request_id = RequestId::new(format!(
            "m7-tag15-observe-{nonce}-{}-{page_index}",
            std::process::id()
        ))
        .context("invalid Tag15 observer request ID")?;
        let context = MessageContext::new(run_id.clone(), request_id, Participant::Maker);
        terms
            .validate_runtime_binding(&context, runtime)
            .context("Tag15 terms do not bind the selected Maker runtime")?;
        let window = DiscoveryWindow::new(scan_start_height, page_blocks)
            .context("invalid Tag15 finalized scan window")?;
        let result = client
            .classify_finalized_native_xmr_effect_v3(
                ClassifyFinalizedNativeXmrEffectV3Request::new(
                    context,
                    runtime.clone(),
                    *terms,
                    XmrNativeEffectV3::Claim,
                    FinalizedNativeXmrTransactionTargetV3::DiscoverByTerms {},
                    window,
                ),
            )
            .await
            .context("typed finalized Tag15 classification failed")?;
        match &result.outcome {
            FinalizedNativeXmrScanOutcomeV3::Found { .. } => {
                return Ok(Some(result));
            }
            FinalizedNativeXmrScanOutcomeV3::Absent {
                finalized_clock,
                scanned_window,
            }
            | FinalizedNativeXmrScanOutcomeV3::Uncertain {
                finalized_clock,
                scanned_window,
            } => {
                ensure!(
                    scanned_window.start_height() == scan_start_height
                        && scanned_window.max_blocks() == page_blocks,
                    "Tag15 classifier returned a different scan page"
                );
                let Some(next_height) =
                    next_scan_start_height(scan_start_height, page_blocks, finalized_clock.height)?
                else {
                    break;
                };
                scan_start_height = next_height;
            }
            FinalizedNativeXmrScanOutcomeV3::Unavailable { .. } => break,
        }
    }
    Ok(None)
}

fn scan_page_blocks(start_height: u64, finalized_height: u64) -> Result<Option<u32>> {
    if finalized_height < start_height {
        return Ok(None);
    }
    let available = finalized_height
        .checked_sub(start_height)
        .and_then(|distance| distance.checked_add(1))
        .context("Tag15 finalized tail length overflow")?;
    Ok(Some(u32::try_from(
        available.min(u64::from(MAX_SCAN_BLOCKS)),
    )?))
}

fn next_scan_start_height(
    start_height: u64,
    page_blocks: u32,
    finalized_height: u64,
) -> Result<Option<u64>> {
    ensure!(page_blocks > 0, "Tag15 scan page is empty");
    let page_end = start_height
        .checked_add(u64::from(page_blocks - 1))
        .context("Tag15 scan page height overflow")?;
    if finalized_height < page_end {
        return Ok(None);
    }
    Ok(Some(
        page_end
            .checked_add(1)
            .context("Tag15 next scan page height overflow")?,
    ))
}

fn load_activation_evidence(plan: &XmrEffectChildPlanV1) -> Result<ActivationEvidenceV1> {
    let evidence: ActivationEvidenceV1 = read_canonical_private_json(
        &plan.evidence_root().join(ACTIVATION_FILE),
        "Maker claim activation evidence",
    )?;
    ensure!(
        evidence.schema == "lez_v02_m7_maker_claim_activation_v1"
            && evidence.role == "maker"
            && evidence.run_id == plan.run_id()
            && evidence.swap_id == hex::encode(plan.swap_id())
            && evidence.selected_branch == "claim"
            && evidence.prepared_step == XmrWorkflowStep::ClaimLezTag15.name()
            && !evidence.private_material_disclosed,
        "Tag15 observer activation evidence differs from the exact plan"
    );
    Ok(evidence)
}

fn persist_finalized_result(
    result: &ClassifyFinalizedNativeXmrEffectV3Result,
    evidence_root: &Path,
) -> Result<Option<String>> {
    let FinalizedNativeXmrScanOutcomeV3::Found { .. } = &result.outcome else {
        return Ok(None);
    };
    let evidence_bytes = canonical_line(result).context("encode finalized Tag15 evidence")?;
    persist_or_validate(&evidence_root.join(FINAL_EVIDENCE_FILE), &evidence_bytes)?;
    Ok(Some(hex::encode(Sha256::digest(&evidence_bytes))))
}

fn validate_args() -> Result<()> {
    let args = std::env::args_os().collect::<Vec<_>>();
    ensure!(
        args.len() == 3
            && args[1] == "--xmr-workflow-step"
            && args[2] == XmrWorkflowStep::ClaimLezTag15.name(),
        "finalized classifier requires the parent-selected Tag15 step"
    );
    Ok(())
}

fn parse_view_key(bytes: &[u8]) -> Result<MoneroPrivateViewKey> {
    let logical = bytes
        .strip_suffix(b"\r\n")
        .or_else(|| bytes.strip_suffix(b"\n"))
        .unwrap_or(bytes);
    let text =
        Zeroizing::new(String::from_utf8(logical.to_vec()).context("view key is not UTF-8")?);
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

fn read_sidecar_capability_fd(fd: i32) -> Result<SidecarCapability> {
    let mut bytes = Zeroizing::new(read_sealed_fd(
        fd,
        MAX_CAPABILITY_FILE_BYTES,
        "Maker sidecar capability",
    )?);
    if bytes.ends_with(b"\r\n") {
        let content_len = bytes.len() - 2;
        bytes.truncate(content_len);
    } else if bytes.ends_with(b"\n") {
        let content_len = bytes.len() - 1;
        bytes.truncate(content_len);
    }
    let value = String::from_utf8(std::mem::take(&mut *bytes)).map_err(|error| {
        let mut rejected = error.into_bytes();
        rejected.zeroize();
        anyhow::anyhow!("Maker sidecar capability is not UTF-8")
    })?;
    SidecarCapability::new(value).context("Maker sidecar capability is invalid")
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
        Err(error) => return Err(error).context("inspect existing Tag15 evidence"),
    }
    let parent = path.parent().context("Tag15 evidence has no parent")?;
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .context("Tag15 evidence name is invalid")?;
    let staging = parent.join(format!(".{name}.tmp-{}", std::process::id()));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&staging)
        .context("reserve Tag15 staging evidence")?;
    file.write_all(bytes)
        .context("write Tag15 staging evidence")?;
    file.sync_all().context("sync Tag15 staging evidence")?;
    drop(file);
    match renameat_with(CWD, &staging, CWD, path, RenameFlags::NOREPLACE) {
        Ok(()) => File::open(parent)
            .context("open Tag15 evidence directory")?
            .sync_all()
            .context("sync Tag15 evidence directory"),
        Err(rustix::io::Errno::EXIST) => {
            fs::remove_file(&staging).context("remove redundant Tag15 staging evidence")?;
            validate_existing_evidence(path, bytes)
        }
        Err(error) => {
            let _ = fs::remove_file(&staging);
            Err(error).context("publish Tag15 evidence atomically")
        }
    }
}

fn validate_existing_evidence(path: &Path, bytes: &[u8]) -> Result<()> {
    let existing: ClassifyFinalizedNativeXmrEffectV3Result =
        read_canonical_private_json(path, "finalized Tag15 evidence")?;
    ensure!(
        canonical_line(&existing)? == bytes,
        "durable Tag15 finality evidence changed"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{next_scan_start_height, scan_page_blocks};

    #[test]
    fn finalized_tag15_observation_scans_the_available_tail_without_waiting_for_a_full_page() {
        assert_eq!(scan_page_blocks(131, 130).unwrap(), None);
        assert_eq!(scan_page_blocks(131, 135).unwrap(), Some(5));
        assert_eq!(scan_page_blocks(131, 200).unwrap(), Some(16));
    }

    #[test]
    fn finalized_tag15_observation_advances_past_a_complete_page_boundary() {
        assert_eq!(next_scan_start_height(131, 16, 146).unwrap(), Some(147));
        assert_eq!(next_scan_start_height(131, 16, 145).unwrap(), None);
    }
}
