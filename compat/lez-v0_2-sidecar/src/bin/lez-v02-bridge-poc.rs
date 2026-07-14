#![forbid(unsafe_code)]

use std::{
    fs,
    io::{self, Write as _},
    net::SocketAddr,
    os::unix::fs::{MetadataExt as _, PermissionsExt as _},
    path::{Path, PathBuf},
    str::FromStr as _,
    sync::Arc,
    time::Duration,
};

use anyhow::{Context as _, Result, bail, ensure};
use clap::Parser;
use indexer_service_rpc::RpcClient as _;
use lez_bridge_protocol::{Hex32, RunId, RuntimeDescriptor};
use lez_v0_2_sidecar::{
    BridgeRuntime, BridgeServerCapability, BridgeServerConfig, NativeEscrowPlanner,
    OfficialNodeRpc, program_id_from_hex, start_bridge_server, validate_loopback_http_endpoint,
};
use nssa::{AccountId, PrivateKey, PublicKey};
use sequencer_service_rpc::SequencerClientBuilder;
use serde::Serialize;
use tokio::io::{AsyncBufReadExt as _, BufReader};
use zeroize::{Zeroize as _, Zeroizing};

const MAX_PUBLIC_CONFIG_BYTES: u64 = 16 * 1024;
const MAX_SECRET_FILE_BYTES: u64 = 256;
const MAX_NODE_REQUEST_BYTES: u32 = 2_800_000;
const MAX_NODE_RESPONSE_BYTES: u32 = 8 * 1024 * 1024;
const INDEXER_STARTUP_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Run one role-isolated exact-LEZ-v0.2 bridge for a local-devnet `PoC`.
#[derive(Debug, Parser)]
#[command(version, about)]
struct Arguments {
    /// Run-allocated literal-loopback nonzero listen address.
    #[arg(long)]
    listen_address: SocketAddr,
    /// Explicit literal-loopback official sequencer HTTP URL and port.
    #[arg(long)]
    sequencer_url: String,
    /// Explicit literal-loopback official indexer HTTP URL and port.
    #[arg(long)]
    indexer_url: String,
    /// Composed run identity shared with this actor's bridge client.
    #[arg(long)]
    run_id: String,
    /// Public JSON file containing the exact immutable runtime descriptor.
    #[arg(long)]
    runtime_file: PathBuf,
    /// Owner-only file containing the bridge bearer capability.
    #[arg(long)]
    capability_file: PathBuf,
    /// Owner-only file containing one lowercase-hex 32-byte LEZ private key.
    #[arg(long)]
    private_key_file: PathBuf,
    /// Existing owner-only 0700 directory dedicated to this actor and run.
    #[arg(long)]
    state_directory: PathBuf,
    /// Checked authenticated-transfer program ID as 64 lowercase hex characters.
    #[arg(long)]
    authenticated_transfer_program_id: String,
    /// Stop after one line is read from standard input (process-harness mode).
    #[arg(long)]
    shutdown_on_stdin: bool,
}

#[derive(Debug, Serialize)]
#[serde(deny_unknown_fields)]
struct Readiness<'a> {
    event: &'static str,
    endpoint: &'a str,
    run_id: &'a str,
    runtime: &'a RuntimeDescriptor,
    sequencer_observation: &'static str,
    indexer_health: &'static str,
    finality: &'static str,
}

#[tokio::main]
async fn main() {
    if let Err(error) = execute(Arguments::parse()).await {
        eprintln!("LEZ v0.2 bridge PoC failed: {error:#}");
        std::process::exit(1);
    }
}

async fn execute(arguments: Arguments) -> Result<()> {
    validate_arguments(&arguments)?;
    let run_id = RunId::new(arguments.run_id).context("invalid run ID")?;
    let runtime: RuntimeDescriptor = serde_json::from_slice(&read_public_file(
        &arguments.runtime_file,
        MAX_PUBLIC_CONFIG_BYTES,
    )?)
    .context("invalid runtime descriptor")?;
    let mut capability = read_secret_text(&arguments.capability_file)?;
    let capability = BridgeServerCapability::new(std::mem::take(&mut *capability))
        .context("invalid bridge capability")?;
    let signer_key = read_private_key(&arguments.private_key_file)?;
    let signer_account_id = AccountId::from(&PublicKey::new_from_private_key(&signer_key));
    ensure!(
        runtime.signer_account_id == Hex32::from_bytes(signer_account_id.into_value()),
        "runtime signer does not match the isolated private key"
    );
    let authenticated_transfer_program = parse_nonzero_hex(
        &arguments.authenticated_transfer_program_id,
        "authenticated-transfer program ID",
    )?;
    ensure!(
        authenticated_transfer_program != runtime.escrow_program_id,
        "escrow and authenticated-transfer programs must be distinct"
    );
    let node = Arc::new(
        OfficialNodeRpc::connect(&arguments.sequencer_url)
            .context("invalid official sequencer endpoint")?,
    );
    let indexer = SequencerClientBuilder::default()
        .max_request_size(MAX_NODE_REQUEST_BYTES)
        .max_response_size(MAX_NODE_RESPONSE_BYTES)
        .request_timeout(INDEXER_STARTUP_REQUEST_TIMEOUT)
        .max_concurrent_requests(1)
        .build(&arguments.indexer_url)
        .context("connect official indexer")?;
    let finalized_block_id = indexer
        .get_last_finalized_block_id()
        .await
        .context("official indexer finalized tip is unavailable")?
        .context("official indexer has no finalized block")?;
    ensure!(
        finalized_block_id >= 2,
        "official indexer has not finalized a non-genesis block"
    );
    let planner = Arc::new(NativeEscrowPlanner::new_durable(
        runtime.sidecar_role,
        signer_key,
        program_id_from_hex(runtime.escrow_program_id),
        program_id_from_hex(authenticated_transfer_program),
        runtime.clone(),
        Arc::clone(&node),
        &arguments.state_directory,
    )?);
    let bridge_runtime = Arc::new(BridgeRuntime::new(runtime.clone(), planner, node));
    let server = start_bridge_server(
        BridgeServerConfig::new(
            run_id.clone(),
            capability,
            arguments.state_directory.join("bridge-requests.v1.json"),
            arguments.listen_address,
        ),
        bridge_runtime,
    )
    .await?;
    serde_json::to_writer(
        io::stdout().lock(),
        &Readiness {
            event: "ready",
            endpoint: server.endpoint(),
            run_id: run_id.as_str(),
            runtime: &runtime,
            sequencer_observation: "bounded_canonical_inclusion_and_same_tip_accounts",
            indexer_health: "getLastFinalizedBlockId_non_genesis",
            finality: "not_observed_by_this_poc_bridge",
        },
    )
    .context("encode readiness")?;
    println!();
    io::stdout().flush().context("flush readiness")?;

    if arguments.shutdown_on_stdin {
        let mut line = String::new();
        BufReader::new(tokio::io::stdin())
            .read_line(&mut line)
            .await
            .context("read shutdown signal")?;
        line.zeroize();
    } else {
        tokio::signal::ctrl_c()
            .await
            .context("wait for interrupt")?;
    }
    server.stop().await?;
    Ok(())
}

fn validate_arguments(arguments: &Arguments) -> Result<()> {
    ensure!(
        arguments.listen_address.ip().is_loopback() && arguments.listen_address.port() != 0,
        "listen address must be a literal loopback IP and nonzero port"
    );
    validate_loopback_http_endpoint(&arguments.sequencer_url).context("invalid sequencer URL")?;
    validate_loopback_http_endpoint(&arguments.indexer_url).context("invalid indexer URL")?;
    ensure!(
        arguments.capability_file != arguments.private_key_file
            && arguments.runtime_file != arguments.capability_file
            && arguments.runtime_file != arguments.private_key_file,
        "configuration and secret files must be distinct"
    );
    let state = fs::symlink_metadata(&arguments.state_directory)
        .context("state directory is unavailable")?;
    ensure!(
        state.is_dir() && state.permissions().mode() & 0o7777 == 0o700,
        "state directory must be a real owner-only 0700 directory"
    );
    Ok(())
}

fn read_public_file(path: &Path, maximum: u64) -> Result<Vec<u8>> {
    let metadata = fs::symlink_metadata(path).context("public config is unavailable")?;
    ensure!(
        metadata.file_type().is_file() && metadata.len() != 0 && metadata.len() <= maximum,
        "public config must be one bounded regular file"
    );
    fs::read(path).context("read public config")
}

fn read_secret_text(path: &Path) -> Result<Zeroizing<String>> {
    let metadata = fs::symlink_metadata(path).context("secret file is unavailable")?;
    ensure!(
        metadata.file_type().is_file()
            && metadata.len() != 0
            && metadata.len() <= MAX_SECRET_FILE_BYTES
            && metadata.permissions().mode().trailing_zeros() >= 6
            && metadata.nlink() == 1,
        "secret must be one bounded owner-only regular file"
    );
    let mut bytes = Zeroizing::new(fs::read(path).context("read secret file")?);
    while bytes
        .last()
        .is_some_and(|byte| matches!(byte, b'\n' | b'\r'))
    {
        bytes.pop();
    }
    let value = String::from_utf8(bytes.to_vec()).context("secret file is not UTF-8")?;
    ensure!(
        !value.is_empty() && !value.contains(['\n', '\r']),
        "secret file contains invalid line breaks"
    );
    Ok(Zeroizing::new(value))
}

fn read_private_key(path: &Path) -> Result<PrivateKey> {
    let encoded = read_secret_text(path)?;
    ensure!(
        encoded.len() == 64
            && encoded
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "private key must be exactly 64 lowercase hex characters"
    );
    PrivateKey::from_str(encoded.as_str()).context("invalid private key")
}

fn parse_nonzero_hex(value: &str, name: &str) -> Result<Hex32> {
    let value = Hex32::from_hex(value).with_context(|| format!("invalid {name}"))?;
    if value.as_bytes() == &[0; 32] {
        bail!("{name} must be nonzero");
    }
    Ok(value)
}
