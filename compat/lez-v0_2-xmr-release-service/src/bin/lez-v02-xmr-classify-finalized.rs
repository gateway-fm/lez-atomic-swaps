//! Typed operator bridge for one canonical M4 finalized-effect classification.

#![forbid(unsafe_code)]

use std::{
    fs::{self, OpenOptions},
    io::Write as _,
    os::unix::fs::OpenOptionsExt as _,
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Context as _, Result, bail, ensure};
use clap::{Parser, ValueEnum};
use lez_bridge_adapter::{CapabilityFileBridgeClientFactory, FreshLezBridgeTransportFactory as _};
use lez_bridge_protocol::{
    ClassifyFinalizedNativeXmrEffectV3Request, DiscoveryWindow,
    FinalizedNativeXmrTransactionTargetV3, MessageContext, Participant, RequestId, RunId,
    RuntimeDescriptor, XmrNativeEffectV3, XmrNativeEscrowTermsV3,
};

const MAX_PUBLIC_JSON_BYTES: u64 = 2 * 1024 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Perform one authenticated, bounded finalized classification and write result-only JSON.
#[derive(Debug, Parser)]
#[command(version, about)]
struct Arguments {
    /// Literal-loopback sidecar endpoint.
    #[arg(long)]
    sidecar_endpoint: String,
    /// Owner-private bearer capability file for this role's sidecar.
    #[arg(long)]
    capability_file: PathBuf,
    /// Public exact runtime descriptor JSON.
    #[arg(long)]
    runtime_file: PathBuf,
    /// Public exact XMR-native terms JSON.
    #[arg(long)]
    terms_file: PathBuf,
    /// Exact run identity shared with the role sidecar.
    #[arg(long)]
    run_id: String,
    /// Unique identity for this one classification call.
    #[arg(long)]
    request_id: String,
    /// Role performing the classification.
    #[arg(long, value_enum)]
    role: Role,
    /// Semantic finalized effect to classify.
    #[arg(long, value_enum)]
    effect: Effect,
    /// Exact prepared transaction JSON; omit for terms-based discovery.
    #[arg(long)]
    exact_transaction_file: Option<PathBuf>,
    /// First finalized height in the bounded inclusive scan.
    #[arg(long)]
    start_height: u64,
    /// Maximum block count in the bounded inclusive scan.
    #[arg(long)]
    max_blocks: u32,
    /// New canonical result-only JSON file; never overwritten.
    #[arg(long)]
    output_result: PathBuf,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum Role {
    Maker,
    Taker,
}

impl From<Role> for Participant {
    fn from(value: Role) -> Self {
        match value {
            Role::Maker => Self::Maker,
            Role::Taker => Self::Taker,
        }
    }
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum Effect {
    Initialize,
    Fund,
    AuthorizeClaim,
    Claim,
    Refund,
    Punish,
}

impl From<Effect> for XmrNativeEffectV3 {
    fn from(value: Effect) -> Self {
        match value {
            Effect::Initialize => Self::Initialize,
            Effect::Fund => Self::Fund,
            Effect::AuthorizeClaim => Self::AuthorizeClaim,
            Effect::Claim => Self::Claim,
            Effect::Refund => Self::Refund,
            Effect::Punish => Self::Punish,
        }
    }
}

#[tokio::main]
async fn main() {
    if let Err(error) = execute(Arguments::parse()).await {
        eprintln!("M4 finalized-effect classification failed: {error:#}");
        std::process::exit(1);
    }
}

async fn execute(arguments: Arguments) -> Result<()> {
    ensure_output_absent(&arguments.output_result)?;
    let runtime: RuntimeDescriptor = read_public_json(&arguments.runtime_file, "runtime")?;
    let terms: XmrNativeEscrowTermsV3 = read_public_json(&arguments.terms_file, "terms")?;
    let run_id = RunId::new(arguments.run_id).context("invalid run ID")?;
    let request_id = RequestId::new(arguments.request_id).context("invalid request ID")?;
    let role = Participant::from(arguments.role);
    ensure!(
        runtime.sidecar_role == role,
        "classifier role differs from the exact runtime"
    );
    let window = DiscoveryWindow::new(arguments.start_height, arguments.max_blocks)
        .context("invalid finalized discovery window")?;
    let target = match arguments.exact_transaction_file {
        Some(path) => FinalizedNativeXmrTransactionTargetV3::exact(read_public_json(
            &path,
            "exact transaction",
        )?),
        None => FinalizedNativeXmrTransactionTargetV3::DiscoverByTerms {},
    };
    let context = MessageContext::new(run_id.clone(), request_id, role);
    terms
        .validate_runtime_binding(&context, &runtime)
        .context("terms do not bind the selected role runtime")?;
    let client = CapabilityFileBridgeClientFactory::new(
        arguments.sidecar_endpoint,
        arguments.capability_file,
        run_id,
        runtime.clone(),
        REQUEST_TIMEOUT,
    )
    .fresh_transport()
    .context("authenticated sidecar client is unavailable")?;
    let result = client
        .classify_finalized_native_xmr_effect_v3(ClassifyFinalizedNativeXmrEffectV3Request::new(
            context,
            runtime,
            terms,
            arguments.effect.into(),
            target,
            window,
        ))
        .await
        .context("typed finalized classification failed")?;
    write_result(&arguments.output_result, &result)?;
    println!(
        "{}",
        serde_json::to_string(&result).context("encode classification result")?
    );
    Ok(())
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
        Ok(_) => bail!("classification result destination already exists"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error).context("inspect classification result destination"),
    }
    let parent = path
        .parent()
        .context("classification result has no parent")?;
    ensure!(
        fs::metadata(parent)
            .context("inspect classification result parent")?
            .is_dir(),
        "classification result parent is not a directory"
    );
    Ok(())
}

fn write_result(path: &Path, result: &impl serde::Serialize) -> Result<()> {
    let mut bytes = serde_json::to_vec(result).context("encode classification result")?;
    bytes.push(b'\n');
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .context("create classification result")?;
    file.write_all(&bytes)
        .context("write classification result")?;
    file.sync_all().context("sync classification result")?;
    Ok(())
}
