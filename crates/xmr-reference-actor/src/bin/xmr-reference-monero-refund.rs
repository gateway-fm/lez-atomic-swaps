//! Sealed no-argument Maker Monero refund-sweep worker.

#![forbid(unsafe_code)]

use std::{
    fs::{self, File, OpenOptions},
    io::{Read as _, Write as _},
    os::unix::fs::{MetadataExt as _, OpenOptionsExt as _, PermissionsExt as _},
    path::{Path, PathBuf},
};

use anyhow::{Context as _, Result, bail, ensure};
use lez_adaptor_role_runner::{
    Role, ValidatedSession, extract_verified_adaptor_secret, read_final_signature_packet_bytes,
};
use lez_bridge_adapter::XmrLezBridgeBindingV3;
use lez_bridge_protocol::{MessageContext, Participant, RequestId, RunId, RuntimeDescriptor};
use lez_swap_store::{AdaptorSessionPhase, SqliteAdaptorSessionJournal, XmrWorkflowStep};
use lez_xmr_monero_adapter::{LoopbackRpcEndpoint, MoneroRegtestWalletEffects};
use lez_xmr_swap_sdk::{
    CrossCurveScalar, MAX_XMR_ACTIVATION_WIRE_BYTES, MAX_XMR_AGREEMENT_WIRE_BYTES,
    MoneroPrivateViewKey, ReconstructedMoneroSpendKey, XmrActivatedAgreementV1, XmrAgreementV1,
};
use rustix::fs::{SealFlags, fcntl_get_seals};
use serde::Serialize;
use sha2::{Digest as _, Sha256};
use xmr_reference_actor::{
    ActorRole, XMR_EFFECT_DAEMON_PASSWORD_FD, XMR_EFFECT_DAEMON_USERNAME_FD,
    XMR_EFFECT_FINALIZED_REFUND_SIGNATURE_FD, XMR_EFFECT_PRIVATE_VIEW_KEY_FD,
    XMR_EFFECT_PRIVATE_XMR_SHARE_FD, XMR_EFFECT_ROLE_PASSWORD_FD, XMR_EFFECT_ROLE_USERNAME_FD,
    XMR_EFFECT_RUNTIME_FD, XMR_EFFECT_SHARED_PASSWORD_FD, XMR_EFFECT_SHARED_USERNAME_FD,
    XMR_EFFECT_SHARED_WALLET_FILE_PASSWORD_FD, XMR_EFFECT_STAGE_A_FD, XMR_EFFECT_STAGE_B_FD,
    XmrEffectChildModeV1, XmrEffectChildPlanV1, load_xmr_effect_child_plan_fd,
};
use zeroize::{Zeroize as _, Zeroizing};

const ABI: &str = "lez_xmr_monero_refund_sweep_v3";
const MAX_RUNTIME_BYTES: usize = 16 * 1024;
const MAX_SECRET_BYTES: usize = 256;
const MAX_FINAL_SIGNATURE_PACKET_BYTES: usize = 4 * 1024;
const RESTORE_HEIGHT: u64 = 0;

struct ValidatedInputs {
    plan: XmrEffectChildPlanV1,
    agreement: XmrAgreementV1,
    view_key: MoneroPrivateViewKey,
    reconstructed: ReconstructedMoneroSpendKey,
    reconstructed_public_spend_key: [u8; 32],
    daemon: LoopbackRpcEndpoint,
    shared_wallet: LoopbackRpcEndpoint,
    role_wallet: LoopbackRpcEndpoint,
    wallet_password: Zeroizing<String>,
    evidence_path: PathBuf,
}

#[derive(Debug, Serialize)]
struct RefundSubmissionEvidence {
    schema: &'static str,
    role: &'static str,
    run_id: String,
    swap_id: String,
    agreement_commitment: String,
    activation_commitment: String,
    monero_genesis_hash: String,
    shared_address: String,
    reconstructed_public_spend_key: String,
    destination_address: String,
    funded_amount_piconero: u64,
    received_amount_piconero: u64,
    fee_piconero: u64,
    transaction_id: String,
    restore_height: u64,
    sending_tool_plan_sha256: String,
    #[serde(flatten)]
    submission_policy: SubmissionPolicy,
    #[serde(flatten)]
    resources: ResourceUse,
}

#[derive(Debug, Serialize)]
struct SubmissionPolicy {
    finality_observer_required: bool,
    automatic_submission_retry: bool,
}

#[derive(Debug, Serialize)]
struct ResourceUse {
    public_rpc_used: bool,
    faucet_used: bool,
}

#[tokio::main]
async fn main() {
    if let Err(error) = execute().await {
        eprintln!("M7 Maker Monero refund worker failed: {error:#}");
        std::process::exit(1);
    }
}

async fn execute() -> Result<()> {
    ensure!(
        std::env::args_os().len() == 1,
        "Monero refund worker accepts no arguments"
    );
    let inputs = validate_inputs()?;
    let destination_effects = MoneroRegtestWalletEffects::new(&inputs.daemon, &inputs.role_wallet)
        .context("Maker destination-wallet boundary is invalid")?;
    let destination = destination_effects
        .primary_standard_address()
        .await
        .context("Maker destination address is unavailable")?;
    let destination_address = destination.to_string();
    let mut evidence_file = reserve_evidence(&inputs.evidence_path)?;
    let wallet_filename = wallet_filename(inputs.plan.run_id());
    let sweeper = MoneroRegtestWalletEffects::new(&inputs.daemon, &inputs.shared_wallet)
        .context("shared-wallet effect boundary is invalid")?;
    let sweep = sweeper
        .restore_shared_and_sweep_once(
            inputs.agreement.shared_address(),
            inputs.reconstructed,
            inputs.view_key,
            wallet_filename,
            inputs.wallet_password.to_string(),
            RESTORE_HEIGHT,
            inputs.agreement.body().monero().amount_piconero(),
            destination,
        )
        .await
        .context("official reconstructed-wallet refund sweep failed")?;
    let evidence = RefundSubmissionEvidence {
        schema: "lez_v02_m7_monero_refund_submission_v1",
        role: "maker",
        run_id: inputs.plan.run_id().to_owned(),
        swap_id: hex::encode(inputs.plan.swap_id()),
        agreement_commitment: hex::encode(inputs.plan.agreement_commitment()),
        activation_commitment: hex::encode(inputs.plan.activation_commitment()),
        monero_genesis_hash: hex::encode(inputs.agreement.body().monero().genesis_hash()),
        shared_address: inputs.agreement.shared_address().address_string(),
        reconstructed_public_spend_key: hex::encode(inputs.reconstructed_public_spend_key),
        destination_address,
        funded_amount_piconero: sweep.funded_amount_piconero(),
        received_amount_piconero: sweep.received_amount_piconero(),
        fee_piconero: sweep.fee_piconero(),
        transaction_id: hex::encode(sweep.transaction_id().0),
        restore_height: RESTORE_HEIGHT,
        sending_tool_plan_sha256: hex::encode(inputs.plan.sending_tool_plan_sha256()),
        submission_policy: SubmissionPolicy {
            finality_observer_required: true,
            automatic_submission_retry: false,
        },
        resources: ResourceUse {
            public_rpc_used: false,
            faucet_used: false,
        },
    };
    write_evidence(&mut evidence_file, &evidence)
}

#[allow(clippy::too_many_lines)]
fn validate_inputs() -> Result<ValidatedInputs> {
    let plan = load_xmr_effect_child_plan_fd().context("load Monero refund child plan")?;
    ensure!(
        plan.role() == ActorRole::Maker
            && plan.mode() == XmrEffectChildModeV1::Invoke
            && plan.step() == XmrWorkflowStep::SweepMoneroRefund
            && plan.executable_abi() == ABI,
        "XMR effect child plan differs from the compiled Monero refund route"
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
        "Monero refund requires the Maker runtime"
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
        "Monero refund effect-child application identity changed"
    );
    validate_runtime_binding(&plan, &agreement, &activation, &runtime)?;

    let session = ValidatedSession::from_untweaked_context(
        agreement
            .refund_session_descriptor()
            .context()
            .context("refund session descriptor is invalid")?,
    )
    .context("refund session is invalid")?;
    let identity = session.identity(Role::Maker);
    let journal = SqliteAdaptorSessionJournal::open_existing(plan.adaptor_journal())
        .context("open locked Maker adaptor journal")?;
    let snapshot = journal
        .load(identity.session_id())
        .context("load durable Maker refund session")?
        .context("durable Maker refund session is absent")?;
    ensure!(
        snapshot.identity() == &identity
            && snapshot.phase() == AdaptorSessionPhase::PresignatureVerified
            && snapshot
                .presignature()
                .is_some_and(|value| *value.bytes() == activation.body().refund_presignature()),
        "durable Maker refund session differs from Stage B or is incomplete"
    );
    let final_signature_packet = read_sealed_fd(
        XMR_EFFECT_FINALIZED_REFUND_SIGNATURE_FD,
        MAX_FINAL_SIGNATURE_PACKET_BYTES,
        "finalized refund signature",
    )?;
    let final_signature = read_final_signature_packet_bytes(&final_signature_packet, &session)
        .context("finalized refund signature packet is invalid")?;
    let extracted = extract_verified_adaptor_secret(
        plan.adaptor_journal(),
        &session,
        Role::Maker,
        final_signature,
    )
    .context("extract refund adaptor scalar from durable Maker transcript")?;
    let maker_share = CrossCurveScalar::from_monero_little_endian(
        read_sealed_fd(
            XMR_EFFECT_PRIVATE_XMR_SHARE_FD,
            32,
            "Maker private XMR share",
        )?
        .try_into()
        .map_err(|_| anyhow::anyhow!("Maker private XMR share has the wrong length"))?,
    )
    .context("Maker private XMR share is invalid")?;
    let reconstructed = ReconstructedMoneroSpendKey::reconstruct(
        agreement.shared_address(),
        agreement.taker_proof(),
        maker_share,
        extracted.into_big_endian_bytes(),
    )
    .context("reconstruct Maker refund spend key")?;
    let reconstructed_public_spend_key = reconstructed.public_key();

    let daemon = endpoint_from_fds(
        plan.monero_daemon_url().as_str(),
        XMR_EFFECT_DAEMON_USERNAME_FD,
        XMR_EFFECT_DAEMON_PASSWORD_FD,
        "daemon",
    )?;
    let shared_wallet = endpoint_from_fds(
        plan.monero_shared_wallet_url().as_str(),
        XMR_EFFECT_SHARED_USERNAME_FD,
        XMR_EFFECT_SHARED_PASSWORD_FD,
        "shared wallet",
    )?;
    let role_wallet = endpoint_from_fds(
        plan.monero_role_wallet_url().as_str(),
        XMR_EFFECT_ROLE_USERNAME_FD,
        XMR_EFFECT_ROLE_PASSWORD_FD,
        "Maker role wallet",
    )?;
    let wallet_password = read_secret_text_fd(
        XMR_EFFECT_SHARED_WALLET_FILE_PASSWORD_FD,
        "shared-wallet file password",
    )?;
    let evidence_path = plan.evidence_root().join("monero-refund-submission.json");
    Ok(ValidatedInputs {
        plan,
        agreement,
        view_key,
        reconstructed,
        reconstructed_public_spend_key,
        daemon,
        shared_wallet,
        role_wallet,
        wallet_password,
        evidence_path,
    })
}

fn validate_runtime_binding(
    plan: &XmrEffectChildPlanV1,
    agreement: &XmrAgreementV1,
    activation: &XmrActivatedAgreementV1,
    runtime: &RuntimeDescriptor,
) -> Result<()> {
    let binding =
        XmrLezBridgeBindingV3::new(agreement, activation).context("Stage-B binding is invalid")?;
    let digest = Sha256::digest(plan.run_id().as_bytes());
    let request_id = RequestId::new(format!("m7-refund-{}", hex::encode(&digest[..12])))
        .context("refund runtime request ID is invalid")?;
    let run_id = RunId::new(plan.run_id().to_owned()).context("run ID is invalid")?;
    binding
        .terms()
        .validate_runtime_binding(
            &MessageContext::new(run_id, request_id, Participant::Maker),
            runtime,
        )
        .context("Maker runtime is not bound by Stage B")
}

fn endpoint_from_fds(
    url: &str,
    username_fd: i32,
    password_fd: i32,
    label: &'static str,
) -> Result<LoopbackRpcEndpoint> {
    let username = read_secret_text_fd(username_fd, label)?;
    let password = read_secret_text_fd(password_fd, label)?;
    LoopbackRpcEndpoint::new(url, username.as_str(), password.as_str())
        .with_context(|| format!("{label} RPC authority is invalid"))
}

fn read_secret_text_fd(fd: i32, label: &'static str) -> Result<Zeroizing<String>> {
    let bytes = Zeroizing::new(read_sealed_fd(fd, MAX_SECRET_BYTES, label)?);
    let mut text = Zeroizing::new(
        String::from_utf8(bytes.to_vec()).with_context(|| format!("{label} is not UTF-8"))?,
    );
    if text.ends_with("\r\n") {
        let content_len = text.len() - 2;
        text.truncate(content_len);
    } else if text.ends_with('\n') {
        text.pop();
    }
    ensure!(!text.is_empty(), "{label} is empty");
    Ok(text)
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
    let metadata = fs::symlink_metadata(path).context("inspect refund evidence root")?;
    ensure!(
        metadata.file_type().is_dir()
            && metadata.uid() == rustix::process::getuid().as_raw()
            && metadata.permissions().mode() & 0o7777 == 0o700,
        "refund evidence root is unsafe"
    );
    Ok(())
}

fn reserve_evidence(path: &Path) -> Result<File> {
    if fs::symlink_metadata(path).is_ok() {
        bail!("refund evidence destination already exists");
    }
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .context("reserve refund submission evidence")
}

fn write_evidence(file: &mut File, evidence: &RefundSubmissionEvidence) -> Result<()> {
    let mut bytes = serde_json::to_vec(evidence).context("encode refund submission evidence")?;
    bytes.push(b'\n');
    file.write_all(&bytes)
        .context("write refund submission evidence")?;
    file.sync_all().context("sync refund submission evidence")
}

fn wallet_filename(run_id: &str) -> String {
    let digest = Sha256::digest(run_id.as_bytes());
    format!("m7_refund_{}", hex::encode(&digest[..12]))
}
