//! Read-only live verification of one exact M4 Monero lock and topology.

#![forbid(unsafe_code)]

use std::{
    fs::{self, File, OpenOptions},
    io::Write as _,
    os::unix::fs::{MetadataExt as _, OpenOptionsExt as _, PermissionsExt as _},
    path::{Path, PathBuf},
    str::FromStr as _,
};

use anyhow::{Context as _, Result, ensure};
use clap::Parser;
use lez_bridge_protocol::{Hex32, RunId};
use lez_xmr_monero_adapter::{
    ExpectedMoneroOutput, LoopbackRpcEndpoint, MoneroAddress, MoneroChainIdentity, MoneroNetwork,
    MoneroOutputVerifier, MoneroTopologyVerifier, MoneroTransactionId,
};
use lez_xmr_swap_sdk::{MAX_XMR_AGREEMENT_WIRE_BYTES, MoneroAddressNetworkV1, XmrAgreementV1};
use serde::Serialize;
use zeroize::Zeroizing;

const MAX_SECRET_BYTES: u64 = 256;

/// Verify peerless authenticated Regtest topology and one exact Stage-A or post-sweep output.
#[derive(Debug, Parser)]
#[command(version, about)]
struct Arguments {
    #[arg(long)]
    agreement_wire_file: PathBuf,
    #[arg(long)]
    monero_transaction_id: String,
    /// Optional exact destination for post-sweep receipt verification.
    #[arg(long, requires = "amount_piconero")]
    destination_address: Option<String>,
    /// Optional exact post-fee amount for post-sweep receipt verification.
    #[arg(long, requires = "destination_address")]
    amount_piconero: Option<u64>,
    #[arg(long)]
    run_id: String,
    #[arg(long)]
    daemon_url: String,
    #[arg(long)]
    daemon_username_file: PathBuf,
    #[arg(long)]
    daemon_password_file: PathBuf,
    #[arg(long)]
    target_wallet_url: String,
    #[arg(long)]
    target_wallet_username_file: PathBuf,
    #[arg(long)]
    target_wallet_password_file: PathBuf,
    #[arg(long)]
    foreign_wallet_url: String,
    #[arg(long)]
    foreign_wallet_username_file: PathBuf,
    #[arg(long)]
    foreign_wallet_password_file: PathBuf,
    #[arg(long)]
    output_evidence: PathBuf,
}

#[derive(Debug, Serialize)]
struct VerificationReport {
    schema: &'static str,
    run_id: String,
    agreement_commitment: String,
    monero_genesis_hash: String,
    transaction_id: String,
    destination_address: String,
    amount_piconero: u64,
    containing_block_hash: String,
    containing_block_height: u64,
    confirmations: u64,
    stable_tip_hash: String,
    stable_tip_height: u64,
    peer_count: u64,
    daemon_version: String,
    target_wallet_version: u32,
    foreign_wallet_version: u32,
    network_scope: &'static str,
    public_rpc_used: bool,
    faucet_used: bool,
}

#[tokio::main]
async fn main() {
    if let Err(error) = execute(Arguments::parse()).await {
        eprintln!("M4 Monero verification failed: {error:#}");
        std::process::exit(1);
    }
}

#[allow(clippy::too_many_lines)]
async fn execute(arguments: Arguments) -> Result<()> {
    let agreement_bytes = read_public_file(
        &arguments.agreement_wire_file,
        u64::try_from(MAX_XMR_AGREEMENT_WIRE_BYTES).unwrap_or(u64::MAX),
    )?;
    let agreement = XmrAgreementV1::from_wire(&agreement_bytes)
        .context("Stage-A agreement failed canonical validation")?;
    ensure!(
        agreement.body().monero().network() == MoneroAddressNetworkV1::Regtest,
        "Stage-A agreement is not Regtest"
    );
    let transaction_id = Hex32::from_hex(&arguments.monero_transaction_id)
        .context("Monero transaction ID is invalid")?;
    let run_id = RunId::new(arguments.run_id.clone()).context("run ID is invalid")?;
    let daemon = endpoint(
        &arguments.daemon_url,
        &arguments.daemon_username_file,
        &arguments.daemon_password_file,
    )?;
    let target = endpoint(
        &arguments.target_wallet_url,
        &arguments.target_wallet_username_file,
        &arguments.target_wallet_password_file,
    )?;
    let foreign = endpoint(
        &arguments.foreign_wallet_url,
        &arguments.foreign_wallet_username_file,
        &arguments.foreign_wallet_password_file,
    )?;
    let identity = MoneroChainIdentity::new(
        MoneroNetwork::Regtest,
        agreement.body().monero().genesis_hash(),
    )
    .context("Stage-A Monero chain identity is invalid")?;
    let (address, amount_piconero) = match (
        arguments.destination_address.as_deref(),
        arguments.amount_piconero,
    ) {
        (Some(address), Some(amount)) => (
            MoneroAddress::from_str(address).context("exact destination address is invalid")?,
            amount,
        ),
        (None, None) => (
            MoneroAddress::from_str(agreement.body().monero().address())
                .context("Stage-A shared address is invalid")?,
            agreement.body().monero().amount_piconero(),
        ),
        (Some(_), None) | (None, Some(_)) => {
            return Err(anyhow::anyhow!(
                "destination and amount must be supplied together"
            ));
        }
    };
    let expected = ExpectedMoneroOutput::new(
        MoneroTransactionId(*transaction_id.as_bytes()),
        address,
        amount_piconero,
    )
    .context("expected Monero output is invalid")?;
    let mut evidence_file = reserve_evidence(&arguments.output_evidence)?;
    let topology =
        MoneroTopologyVerifier::new(run_id.clone(), identity, &daemon, &target, &foreign)
            .context("construct topology verifier")?
            .verify()
            .await
            .context("verify authenticated peerless topology")?;
    let observation = MoneroOutputVerifier::new(identity, &daemon, &target)
        .context("construct output verifier")?
        .verify(&expected)
        .await
        .context("verify exact canonical Monero output")?;
    topology
        .validate_observation(&run_id, &observation)
        .context("cross-bind topology and output")?;
    let report = VerificationReport {
        schema: "lez_v02_m4_actual_local_monero_verification_v2",
        run_id: arguments.run_id,
        agreement_commitment: hex::encode(agreement.agreement_commitment()),
        monero_genesis_hash: hex::encode(observation.genesis_hash()),
        transaction_id: arguments.monero_transaction_id,
        destination_address: observation.destination().to_string(),
        amount_piconero: observation.amount_piconero(),
        containing_block_hash: hex::encode(observation.containing_block_hash()),
        containing_block_height: observation.containing_block_height(),
        confirmations: observation.confirmations(),
        stable_tip_hash: hex::encode(observation.stable_tip_hash()),
        stable_tip_height: observation.stable_tip_height(),
        peer_count: topology.peer_count(),
        daemon_version: topology.daemon_version().to_owned(),
        target_wallet_version: topology.target_wallet_version(),
        foreign_wallet_version: topology.foreign_wallet_version(),
        network_scope: "isolated_official_monero_regtest",
        public_rpc_used: false,
        faucet_used: false,
    };
    write_evidence(&mut evidence_file, &report)?;
    println!(
        "{}",
        serde_json::to_string(&report).context("encode verification report")?
    );
    Ok(())
}

fn reserve_evidence(path: &Path) -> Result<File> {
    let parent = path
        .parent()
        .context("verification evidence has no parent")?;
    let metadata = fs::metadata(parent).context("inspect verification evidence parent")?;
    ensure!(
        metadata.is_dir()
            && metadata.uid() == rustix::process::getuid().as_raw()
            && metadata.permissions().mode().trailing_zeros() >= 6,
        "verification evidence parent is not owner-private"
    );
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .context("reserve new verification evidence")
}

fn write_evidence(output: &mut File, evidence: &VerificationReport) -> Result<()> {
    let mut bytes = serde_json::to_vec(evidence).context("encode verification evidence")?;
    bytes.push(b'\n');
    output
        .write_all(&bytes)
        .context("write verification evidence")?;
    output.sync_all().context("sync verification evidence")?;
    Ok(())
}

fn endpoint(url: &str, username_file: &Path, password_file: &Path) -> Result<LoopbackRpcEndpoint> {
    let username = read_secret(username_file)?;
    let password = read_secret(password_file)?;
    LoopbackRpcEndpoint::new(url, username.to_string(), password.to_string())
        .context("Monero endpoint or credentials are invalid")
}

fn read_public_file(path: &Path, maximum: u64) -> Result<Vec<u8>> {
    let metadata = fs::symlink_metadata(path).context("public input is unavailable")?;
    ensure!(
        metadata.file_type().is_file() && metadata.len() != 0 && metadata.len() <= maximum,
        "public input is not one bounded regular file"
    );
    fs::read(path).context("read public input")
}

fn read_secret(path: &Path) -> Result<Zeroizing<String>> {
    let metadata = fs::symlink_metadata(path).context("secret input is unavailable")?;
    ensure!(
        metadata.file_type().is_file()
            && metadata.len() != 0
            && metadata.len() <= MAX_SECRET_BYTES
            && metadata.uid() == rustix::process::getuid().as_raw()
            && metadata.nlink() == 1
            && metadata.permissions().mode().trailing_zeros() >= 6,
        "secret input is not owner-private and single-link"
    );
    let mut text = fs::read_to_string(path).context("read secret input")?;
    while text.ends_with(['\n', '\r']) {
        text.pop();
    }
    ensure!(!text.is_empty(), "secret input is empty");
    Ok(Zeroizing::new(text))
}
