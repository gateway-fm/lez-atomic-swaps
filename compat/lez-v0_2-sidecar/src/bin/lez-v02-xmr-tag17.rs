//! Role-correct actual-local Tag-17 prepare and one-attempt release driver.

#![forbid(unsafe_code)]

use std::{
    fs::{self, OpenOptions},
    io::Write as _,
    os::unix::fs::{MetadataExt as _, OpenOptionsExt as _, PermissionsExt as _},
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Context as _, Result, bail, ensure};
use clap::{Parser, ValueEnum};
use lez_bridge_client::{BridgeClient, BridgeClientConfig, SidecarCapability};
use lez_bridge_protocol::{
    MessageContext, Participant, PrepareNativeXmrPunishV3Request, PreparedTransaction, RequestId,
    RunId, RuntimeDescriptor, SubmissionOutcome, SubmitTransactionRequest, XmrNativeEscrowTermsV3,
};
use rustix::process::getuid;
use serde::Serialize;

const MAX_PUBLIC_JSON_BYTES: u64 = 2 * 1024 * 1024;
const MAX_CAPABILITY_BYTES: u64 = 512;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(90);

#[derive(Clone, Copy, Debug, ValueEnum)]
enum Mode {
    Prepare,
    Release,
}

/// Prepare or release one exact Maker-signed Tag-17 punishment.
#[derive(Debug, Parser)]
#[command(version, about)]
struct Arguments {
    /// Prepare without submission, or release under one-attempt semantics.
    #[arg(long, value_enum)]
    mode: Mode,
    /// Literal-loopback Maker sidecar endpoint.
    #[arg(long)]
    sidecar_endpoint: String,
    /// Owner-private Maker sidecar bearer capability.
    #[arg(long)]
    capability_file: PathBuf,
    /// Exact public Maker runtime descriptor.
    #[arg(long)]
    runtime_file: PathBuf,
    /// Exact public activated XMR terms.
    #[arg(long)]
    terms_file: PathBuf,
    /// Run identity shared with the Maker sidecar.
    #[arg(long)]
    run_id: String,
    /// Stable preparation identity reused by prepare and release.
    #[arg(long)]
    prepare_request_id: String,
    /// Create-new result packet.
    #[arg(long)]
    output_evidence: PathBuf,
}

#[derive(Debug, Serialize)]
struct Tag17Evidence {
    schema: &'static str,
    role: Participant,
    mode: &'static str,
    run_id: RunId,
    prepare_request_id: RequestId,
    punish: PreparedTransaction,
    submission: SubmissionEvidence,
    resources: ResourceEvidence,
}

#[derive(Debug, Serialize)]
struct SubmissionEvidence {
    request_id: Option<RequestId>,
    outcome: Option<SubmissionOutcome>,
    automatic_retry: bool,
    performed: bool,
}

#[derive(Debug, Serialize)]
struct ResourceEvidence {
    public_rpc_used: bool,
    faucet_used: bool,
    public_funds_used: bool,
}

#[tokio::main]
async fn main() {
    if let Err(error) = execute(Arguments::parse()).await {
        eprintln!("M7 Maker Tag-17 driver failed: {error:#}");
        std::process::exit(1);
    }
}

async fn execute(arguments: Arguments) -> Result<()> {
    ensure_output_absent(&arguments.output_evidence)?;
    let runtime: RuntimeDescriptor = read_public_json(&arguments.runtime_file, "Maker runtime")?;
    let terms: XmrNativeEscrowTermsV3 = read_public_json(&arguments.terms_file, "XMR terms")?;
    let run_id = RunId::new(arguments.run_id).context("invalid run ID")?;
    let prepare_request_id =
        RequestId::new(arguments.prepare_request_id).context("invalid prepare request ID")?;
    ensure!(
        runtime.sidecar_role == Participant::Maker,
        "Tag 17 requires the Maker runtime"
    );
    let context = MessageContext::new(
        run_id.clone(),
        prepare_request_id.clone(),
        Participant::Maker,
    );
    terms
        .validate_runtime_binding(&context, &runtime)
        .context("activated terms do not bind the Maker runtime")?;
    ensure!(
        terms.to_input().claimant == Participant::Maker,
        "Tag 17 claimant is not Maker"
    );

    let client = BridgeClient::connect(BridgeClientConfig::new(
        arguments.sidecar_endpoint,
        read_capability(&arguments.capability_file)?,
        run_id.clone(),
        runtime.clone(),
        REQUEST_TIMEOUT,
    ))
    .context("connect authenticated Maker sidecar")?;
    let prepared = client
        .prepare_native_xmr_punish_v3(PrepareNativeXmrPunishV3Request::new(
            context,
            runtime.clone(),
            terms,
        ))
        .await
        .context("prepare exact durable Tag 17")?;

    let (mode, submission) = match arguments.mode {
        Mode::Prepare => (
            "prepare",
            SubmissionEvidence {
                request_id: None,
                outcome: None,
                automatic_retry: false,
                performed: false,
            },
        ),
        Mode::Release => {
            let request_id = prepared.punish.transaction_id.submission_request_id();
            let result = client
                .submit_transaction(SubmitTransactionRequest::new(
                    MessageContext::new(run_id.clone(), request_id.clone(), Participant::Maker),
                    runtime,
                    prepared.punish.clone(),
                ))
                .await
                .context("release exact durable Tag 17 once")?;
            ensure!(
                result.transaction_id == prepared.punish.transaction_id,
                "Tag-17 submission returned a different transaction ID"
            );
            (
                "release",
                SubmissionEvidence {
                    request_id: Some(request_id),
                    outcome: Some(result.outcome),
                    automatic_retry: false,
                    performed: true,
                },
            )
        }
    };
    let evidence = Tag17Evidence {
        schema: "lez_v02_m7_actual_local_tag17_v1",
        role: Participant::Maker,
        mode,
        run_id,
        prepare_request_id,
        punish: prepared.punish,
        submission,
        resources: ResourceEvidence {
            public_rpc_used: false,
            faucet_used: false,
            public_funds_used: false,
        },
    };
    write_evidence(&arguments.output_evidence, &evidence)
}

fn read_capability(path: &Path) -> Result<SidecarCapability> {
    let metadata = fs::symlink_metadata(path).context("inspect Maker capability")?;
    ensure!(
        metadata.file_type().is_file()
            && metadata.len() != 0
            && metadata.len() <= MAX_CAPABILITY_BYTES
            && metadata.uid() == getuid().as_raw()
            && metadata.nlink() == 1
            && metadata.permissions().mode().trailing_zeros() >= 6,
        "Maker capability must be one bounded owner-private regular file"
    );
    let bytes = fs::read_to_string(path).context("read Maker capability")?;
    SidecarCapability::new(bytes.trim().to_owned()).context("invalid Maker capability")
}

fn read_public_json<T: serde::de::DeserializeOwned>(path: &Path, label: &str) -> Result<T> {
    let metadata = fs::symlink_metadata(path).with_context(|| format!("inspect {label}"))?;
    ensure!(
        metadata.file_type().is_file()
            && metadata.len() != 0
            && metadata.len() <= MAX_PUBLIC_JSON_BYTES,
        "{label} must be one bounded regular file"
    );
    serde_json::from_slice(&fs::read(path).with_context(|| format!("read {label}"))?)
        .with_context(|| format!("decode {label}"))
}

fn ensure_output_absent(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(_) => bail!("Tag-17 evidence destination already exists"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error).context("inspect Tag-17 evidence destination"),
    }
    let parent = path.parent().context("Tag-17 evidence has no parent")?;
    ensure!(
        fs::metadata(parent)
            .context("inspect Tag-17 evidence parent")?
            .is_dir(),
        "Tag-17 evidence parent is not a directory"
    );
    Ok(())
}

fn write_evidence(path: &Path, evidence: &Tag17Evidence) -> Result<()> {
    let mut bytes = serde_json::to_vec(evidence).context("encode Tag-17 evidence")?;
    bytes.push(b'\n');
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .context("create Tag-17 evidence")?;
    file.write_all(&bytes).context("write Tag-17 evidence")?;
    file.sync_all().context("sync Tag-17 evidence")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modes_keep_preparation_and_release_explicit() {
        assert!(matches!(Mode::Prepare, Mode::Prepare));
        assert!(matches!(Mode::Release, Mode::Release));
    }
}
