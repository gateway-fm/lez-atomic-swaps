//! Sealed read-only observer for Maker Monero refund finality.

#![forbid(unsafe_code)]

use std::{
    fs::{self, File, OpenOptions},
    io::{Read as _, Write as _},
    os::unix::fs::{MetadataExt as _, OpenOptionsExt as _, PermissionsExt as _},
    path::Path,
};

use anyhow::{Context as _, Result, bail, ensure};
use lez_swap_store::XmrWorkflowStep;
use lez_xmr_monero_adapter::{
    ExpectedMoneroOutput, LoopbackRpcEndpoint, MoneroAddress, MoneroChainIdentity,
    MoneroEvidenceError, MoneroNetwork, MoneroOutputVerifier, MoneroTransactionId,
    REQUIRED_MONERO_CONFIRMATIONS, VerifiedMoneroOutputObservation,
};
use lez_xmr_swap_sdk::{
    MAX_XMR_ACTIVATION_WIRE_BYTES, MAX_XMR_AGREEMENT_WIRE_BYTES, MoneroAddressNetworkV1,
    MoneroPrivateViewKey, XmrActivatedAgreementV1, XmrAgreementV1,
};
use rustix::fs::{CWD, OFlags, RenameFlags, SealFlags, fcntl_get_seals, open, renameat_with};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use xmr_reference_actor::{
    ActorRole, XMR_EFFECT_DAEMON_PASSWORD_FD, XMR_EFFECT_DAEMON_USERNAME_FD,
    XMR_EFFECT_PRIVATE_VIEW_KEY_FD, XMR_EFFECT_ROLE_PASSWORD_FD, XMR_EFFECT_ROLE_USERNAME_FD,
    XMR_EFFECT_STAGE_A_FD, XMR_EFFECT_STAGE_B_FD, XmrEffectChildModeV1, XmrEffectChildPlanV1,
    load_xmr_effect_child_plan_fd,
};
use zeroize::Zeroizing;

const ABI: &str = "lez_xmr_monero_verify_v2";
const MAX_SECRET_BYTES: usize = 256;
const MAX_EVIDENCE_BYTES: u64 = 16 * 1024;
const SUBMISSION_FILE: &str = "monero-refund-submission.json";
const FINAL_EVIDENCE_FILE: &str = "monero-refund-finalized.json";

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RefundSubmissionEvidence {
    schema: String,
    role: String,
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

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct SubmissionPolicy {
    finality_observer_required: bool,
    automatic_submission_retry: bool,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ResourceUse {
    public_rpc_used: bool,
    faucet_used: bool,
}

#[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct RefundFinalityEvidence {
    schema: String,
    role: String,
    run_id: String,
    swap_id: String,
    agreement_commitment: String,
    activation_commitment: String,
    submission_sha256: String,
    sending_tool_plan_sha256: String,
    monero_genesis_hash: String,
    destination_address: String,
    received_amount_piconero: u64,
    transaction_id: String,
    containing_block_hash: String,
    containing_block_height: u64,
    confirmations: u64,
    stable_tip_hash: String,
    stable_tip_height: u64,
    required_confirmations: u64,
    finality_observer_sent_transaction: bool,
    public_rpc_used: bool,
    faucet_used: bool,
}

struct ValidatedInputs {
    plan: XmrEffectChildPlanV1,
    agreement: XmrAgreementV1,
    daemon: LoopbackRpcEndpoint,
    role_wallet: LoopbackRpcEndpoint,
    submission: RefundSubmissionEvidence,
    submission_bytes: Vec<u8>,
    destination: MoneroAddress,
    transaction_id: MoneroTransactionId,
}

#[tokio::main]
async fn main() {
    if let Err(error) = execute().await {
        eprintln!("M7 Maker Monero finality observer failed: {error:#}");
        std::process::exit(1);
    }
}

async fn execute() -> Result<()> {
    validate_args()?;
    reject_forbidden_invocation_secrets()?;
    let inputs = validate_inputs()?;
    let identity = MoneroChainIdentity::new(
        monero_network(inputs.agreement.body().monero().network()),
        inputs.agreement.body().monero().genesis_hash(),
    )
    .context("Stage-A Monero chain identity is invalid")?;
    let expected = ExpectedMoneroOutput::new(
        inputs.transaction_id,
        inputs.destination,
        inputs.submission.received_amount_piconero,
    )
    .context("refund submission output terms are invalid")?;
    let verifier = MoneroOutputVerifier::new(identity, &inputs.daemon, &inputs.role_wallet)
        .context("Maker refund observation boundary is invalid")?;
    let observation = match verifier.verify(&expected).await {
        Ok(observation) => observation,
        Err(error) if is_pending(&error) => {
            write_pending();
            return Ok(());
        }
        Err(error) => return Err(error).context("Maker refund finality proof failed closed"),
    };
    let evidence_bytes = canonical_line(&final_evidence(&inputs, &observation))
        .context("encode refund finality evidence")?;
    persist_or_validate(
        &inputs.plan.evidence_root().join(FINAL_EVIDENCE_FILE),
        &evidence_bytes,
    )?;
    println!(
        "{{\"schema_version\":1,\"step\":\"sweep_monero_refund\",\"state\":\"finalized\",\"effect_evidence_sha256\":\"{}\"}}",
        hex::encode(Sha256::digest(&evidence_bytes))
    );
    Ok(())
}

fn validate_args() -> Result<()> {
    let args = std::env::args_os().collect::<Vec<_>>();
    ensure!(
        args.len() == 3
            && args[1] == "--xmr-workflow-step"
            && args[2] == XmrWorkflowStep::SweepMoneroRefund.name(),
        "Monero finality observer requires the parent-selected refund step"
    );
    Ok(())
}

fn reject_forbidden_invocation_secrets() -> Result<()> {
    for (fd, label) in [(218, "private XMR share"), (219, "finalized LEZ signature")] {
        match fs::metadata(format!("/proc/self/fd/{fd}")) {
            Ok(_) => bail!("Monero finality observer received forbidden {label} FD"),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error).with_context(|| format!("inspect forbidden {label} FD"));
            }
        }
    }
    Ok(())
}

fn validate_inputs() -> Result<ValidatedInputs> {
    let plan = load_xmr_effect_child_plan_fd().context("load Monero observer child plan")?;
    ensure!(
        plan.role() == ActorRole::Maker
            && plan.mode() == XmrEffectChildModeV1::Observe
            && plan.step() == XmrWorkflowStep::SweepMoneroRefund
            && plan.executable_abi() == ABI,
        "XMR effect child plan differs from the compiled Monero observer route"
    );
    validate_evidence_root(plan.evidence_root())?;
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
        "Monero observer application identity changed"
    );
    let daemon = endpoint_from_fds(
        plan.monero_daemon_url().as_str(),
        XMR_EFFECT_DAEMON_USERNAME_FD,
        XMR_EFFECT_DAEMON_PASSWORD_FD,
        "daemon",
    )?;
    let role_wallet = endpoint_from_fds(
        plan.monero_role_wallet_url().as_str(),
        XMR_EFFECT_ROLE_USERNAME_FD,
        XMR_EFFECT_ROLE_PASSWORD_FD,
        "Maker role wallet",
    )?;
    let (submission, submission_bytes) = read_canonical_private_json(
        &plan.evidence_root().join(SUBMISSION_FILE),
        "refund submission evidence",
    )?;
    validate_submission(&plan, &agreement, &activation, &submission)?;
    let destination = parse_canonical_address(&submission.destination_address)?;
    let transaction_id = MoneroTransactionId(decode_nonzero_hex32(
        &submission.transaction_id,
        "refund transaction ID",
    )?);
    Ok(ValidatedInputs {
        plan,
        agreement,
        daemon,
        role_wallet,
        submission,
        submission_bytes,
        destination,
        transaction_id,
    })
}

fn validate_submission(
    plan: &XmrEffectChildPlanV1,
    agreement: &XmrAgreementV1,
    activation: &XmrActivatedAgreementV1,
    submission: &RefundSubmissionEvidence,
) -> Result<()> {
    ensure!(
        submission.schema == "lez_v02_m7_monero_refund_submission_v1"
            && submission.role == "maker"
            && submission.run_id == plan.run_id()
            && submission.swap_id == hex::encode(plan.swap_id())
            && submission.agreement_commitment == hex::encode(plan.agreement_commitment())
            && submission.activation_commitment == hex::encode(plan.activation_commitment())
            && submission.monero_genesis_hash
                == hex::encode(agreement.body().monero().genesis_hash())
            && submission.shared_address == agreement.shared_address().address_string()
            && submission.funded_amount_piconero == agreement.body().monero().amount_piconero()
            && submission.received_amount_piconero > 0
            && submission
                .received_amount_piconero
                .checked_add(submission.fee_piconero)
                == Some(submission.funded_amount_piconero)
            && submission.sending_tool_plan_sha256 == hex::encode(plan.sending_tool_plan_sha256())
            && submission.submission_policy.finality_observer_required
            && !submission.submission_policy.automatic_submission_retry
            && !submission.resources.public_rpc_used
            && !submission.resources.faucet_used
            && activation.activation_commitment() == plan.activation_commitment(),
        "refund submission evidence differs from the sealed application and sending plan"
    );
    decode_nonzero_hex32(
        &submission.reconstructed_public_spend_key,
        "reconstructed public spend key",
    )?;
    ensure!(
        submission.restore_height == 0,
        "refund restore height changed"
    );
    Ok(())
}

fn final_evidence(
    inputs: &ValidatedInputs,
    observation: &VerifiedMoneroOutputObservation,
) -> RefundFinalityEvidence {
    RefundFinalityEvidence {
        schema: "lez_v02_m7_monero_refund_finality_v1".to_owned(),
        role: "maker".to_owned(),
        run_id: inputs.plan.run_id().to_owned(),
        swap_id: hex::encode(inputs.plan.swap_id()),
        agreement_commitment: hex::encode(inputs.plan.agreement_commitment()),
        activation_commitment: hex::encode(inputs.plan.activation_commitment()),
        submission_sha256: hex::encode(Sha256::digest(&inputs.submission_bytes)),
        sending_tool_plan_sha256: hex::encode(inputs.plan.sending_tool_plan_sha256()),
        monero_genesis_hash: hex::encode(observation.genesis_hash()),
        destination_address: observation.destination().to_string(),
        received_amount_piconero: observation.amount_piconero(),
        transaction_id: hex::encode(observation.transaction_id().0),
        containing_block_hash: hex::encode(observation.containing_block_hash()),
        containing_block_height: observation.containing_block_height(),
        confirmations: observation.confirmations(),
        stable_tip_hash: hex::encode(observation.stable_tip_hash()),
        stable_tip_height: observation.stable_tip_height(),
        required_confirmations: REQUIRED_MONERO_CONFIRMATIONS,
        finality_observer_sent_transaction: false,
        public_rpc_used: false,
        faucet_used: false,
    }
}

fn is_pending(error: &MoneroEvidenceError) -> bool {
    matches!(
        error,
        MoneroEvidenceError::MissingWalletTransfer
            | MoneroEvidenceError::WalletTransferInPool
            | MoneroEvidenceError::OutputNotUnlocked
            | MoneroEvidenceError::DaemonMissedTransaction
            | MoneroEvidenceError::DaemonTransactionInPool
            | MoneroEvidenceError::InsufficientConfirmations
            | MoneroEvidenceError::NonzeroUnlockDistance
    )
}

fn write_pending() {
    println!("{{\"schema_version\":1,\"step\":\"sweep_monero_refund\",\"state\":\"pending\"}}");
}

fn monero_network(network: MoneroAddressNetworkV1) -> MoneroNetwork {
    match network {
        MoneroAddressNetworkV1::Regtest => MoneroNetwork::Regtest,
        MoneroAddressNetworkV1::Stagenet => MoneroNetwork::Stagenet,
    }
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
    while text.ends_with(['\n', '\r']) {
        text.pop();
    }
    ensure!(!text.is_empty(), "{label} is empty");
    Ok(text)
}

fn parse_view_key(bytes: &[u8]) -> Result<MoneroPrivateViewKey> {
    let mut text =
        Zeroizing::new(String::from_utf8(bytes.to_vec()).context("view key is not UTF-8")?);
    while text.ends_with(['\n', '\r']) {
        text.pop();
    }
    MoneroPrivateViewKey::from_monero_little_endian(decode_nonzero_hex32(&text, "Monero view key")?)
        .context("Monero view key is not a canonical scalar")
}

fn parse_canonical_address(value: &str) -> Result<MoneroAddress> {
    let address = value
        .parse::<MoneroAddress>()
        .context("refund destination address is invalid")?;
    ensure!(
        address.to_string() == value,
        "refund destination address is noncanonical"
    );
    Ok(address)
}

fn decode_nonzero_hex32(value: &str, label: &'static str) -> Result<[u8; 32]> {
    ensure!(
        value.len() == 64
            && value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "{label} is not canonical lowercase hex"
    );
    let mut decoded = [0_u8; 32];
    hex::decode_to_slice(value, &mut decoded).with_context(|| format!("decode {label}"))?;
    ensure!(decoded != [0; 32], "{label} is zero");
    Ok(decoded)
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

fn read_canonical_private_json<T>(path: &Path, label: &'static str) -> Result<(T, Vec<u8>)>
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
    let value = serde_json::from_slice(&bytes).with_context(|| format!("{label} is malformed"))?;
    ensure!(canonical_line(&value)? == bytes, "{label} is noncanonical");
    Ok((value, bytes))
}

fn canonical_line(value: &impl Serialize) -> Result<Vec<u8>> {
    let mut bytes = serde_json::to_vec(value).context("encode canonical JSON")?;
    bytes.push(b'\n');
    Ok(bytes)
}

fn persist_or_validate(path: &Path, bytes: &[u8]) -> Result<()> {
    if path.exists() {
        return validate_existing_evidence(path, bytes);
    }
    let parent = path
        .parent()
        .context("refund finality evidence has no parent")?;
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .context("refund finality evidence name is invalid")?;
    let staging = parent.join(format!(".{name}.tmp-{}", std::process::id()));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&staging)
        .context("reserve refund finality staging evidence")?;
    file.write_all(bytes)
        .context("write refund finality staging evidence")?;
    file.sync_all()
        .context("sync refund finality staging evidence")?;
    drop(file);
    match renameat_with(CWD, &staging, CWD, path, RenameFlags::NOREPLACE) {
        Ok(()) => File::open(parent)
            .context("open refund evidence directory")?
            .sync_all()
            .context("sync refund evidence directory"),
        Err(rustix::io::Errno::EXIST) => {
            fs::remove_file(&staging).context("remove redundant finality staging evidence")?;
            validate_existing_evidence(path, bytes)
        }
        Err(error) => {
            let _ = fs::remove_file(&staging);
            Err(error).context("publish refund finality evidence atomically")
        }
    }
}

fn validate_existing_evidence(path: &Path, bytes: &[u8]) -> Result<()> {
    let (_existing, existing_bytes): (RefundFinalityEvidence, Vec<u8>) =
        read_canonical_private_json(path, "refund finality evidence")?;
    ensure!(
        existing_bytes == bytes,
        "durable refund finality evidence changed"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_nonfinal_observations_are_pending() {
        assert!(is_pending(&MoneroEvidenceError::MissingWalletTransfer));
        assert!(is_pending(&MoneroEvidenceError::WalletTransferInPool));
        assert!(is_pending(&MoneroEvidenceError::InsufficientConfirmations));
        assert!(!is_pending(&MoneroEvidenceError::AmountMismatch));
        assert!(!is_pending(&MoneroEvidenceError::DoubleSpendSeen));
        assert!(!is_pending(&MoneroEvidenceError::UnstableTip));
    }

    #[test]
    fn finality_evidence_publication_is_atomic_idempotent_and_immutable() {
        let root = tempfile::tempdir().expect("finality evidence root");
        let path = root.path().join(FINAL_EVIDENCE_FILE);
        let evidence = RefundFinalityEvidence {
            schema: "lez_v02_m7_monero_refund_finality_v1".to_owned(),
            role: "maker".to_owned(),
            run_id: "m7-observer-publication".to_owned(),
            swap_id: "11".repeat(32),
            agreement_commitment: "22".repeat(32),
            activation_commitment: "33".repeat(32),
            submission_sha256: "44".repeat(32),
            sending_tool_plan_sha256: "55".repeat(32),
            monero_genesis_hash: "66".repeat(32),
            destination_address: "fixture-destination".to_owned(),
            received_amount_piconero: 7,
            transaction_id: "77".repeat(32),
            containing_block_hash: "88".repeat(32),
            containing_block_height: 9,
            confirmations: REQUIRED_MONERO_CONFIRMATIONS,
            stable_tip_hash: "99".repeat(32),
            stable_tip_height: 18,
            required_confirmations: REQUIRED_MONERO_CONFIRMATIONS,
            finality_observer_sent_transaction: false,
            public_rpc_used: false,
            faucet_used: false,
        };
        let bytes = canonical_line(&evidence).expect("canonical finality evidence");
        persist_or_validate(&path, &bytes).expect("publish finality evidence");
        assert_eq!(fs::read(&path).expect("published evidence"), bytes);
        persist_or_validate(&path, &bytes).expect("replay identical finality evidence");
        let mut changed = bytes.clone();
        changed[0] ^= 1;
        assert!(persist_or_validate(&path, &changed).is_err());
        assert_eq!(fs::read(&path).expect("immutable evidence"), bytes);
        assert!(
            fs::read_dir(root.path())
                .expect("read evidence root")
                .all(|entry| !entry
                    .expect("evidence entry")
                    .file_name()
                    .to_string_lossy()
                    .contains(".tmp-"))
        );
    }
}
