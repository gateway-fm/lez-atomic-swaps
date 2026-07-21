//! Actual-local M4 claim-path Monero key reconstruction and sweep effect.

#![forbid(unsafe_code)]

#[cfg(not(target_os = "linux"))]
compile_error!("the M4 actual-local sweep command requires Linux file-safety semantics");

use std::{
    fs::{self, File, OpenOptions},
    io::{Read as _, Write as _},
    os::unix::fs::{MetadataExt as _, OpenOptionsExt as _, PermissionsExt as _},
    path::{Path, PathBuf},
};

use anyhow::{Context as _, Result, ensure};
use clap::Parser;
use lez_bridge_protocol::RunId;
use lez_v0_2_sidecar::validate_loopback_http_endpoint;
use lez_xmr_monero_adapter::{
    ExpectedMoneroOutput, LoopbackRpcEndpoint, MoneroChainIdentity, MoneroNetwork,
    MoneroOutputVerifier, MoneroRegtestWalletEffects, MoneroTopologyVerifier,
};
use lez_xmr_swap_sdk::{
    CrossCurveScalar, MAX_XMR_AGREEMENT_WIRE_BYTES, MoneroAddressNetworkV1, MoneroPrivateViewKey,
    ReconstructedMoneroSpendKey, XmrAgreementV1,
};
use serde::Serialize;
use zeroize::{Zeroize as _, Zeroizing};

const EVIDENCE_SCHEMA: &str = "lez_v02_m4_actual_local_monero_claim_sweep_v2";
const MAX_SECRET_BYTES: usize = 256;
const SCALAR_HEX_BYTES: usize = 64;

/// Reconstruct the Stage-A spend key and sweep the exact shared output once.
#[derive(Debug, Parser)]
#[command(version, about)]
struct Arguments {
    /// Same-swap run identity used by the authenticated topology capability.
    #[arg(long)]
    run_id: String,
    /// Canonical countersigned Stage-A agreement wire.
    #[arg(long)]
    agreement_wire_file: PathBuf,
    /// Owner-private Taker Monero spend-key share in lowercase little-endian hex.
    #[arg(long)]
    taker_share_file: PathBuf,
    /// Owner-private extracted Maker adaptor scalar in lowercase big-endian hex.
    #[arg(long)]
    extracted_maker_adaptor_scalar_file: PathBuf,
    /// Owner-private lowercase-hex Monero view key bound by Stage A.
    #[arg(long)]
    monero_view_key_file: PathBuf,
    /// Literal-loopback Digest-authenticated monerod origin.
    #[arg(long)]
    daemon_url: String,
    #[arg(long)]
    daemon_username_file: PathBuf,
    #[arg(long)]
    daemon_password_file: PathBuf,
    /// Literal-loopback wallet RPC currently holding the shared view wallet.
    #[arg(long)]
    shared_wallet_url: String,
    #[arg(long)]
    shared_wallet_username_file: PathBuf,
    #[arg(long)]
    shared_wallet_password_file: PathBuf,
    /// Password for the reconstructed wallet file created by shared wallet RPC.
    #[arg(long)]
    shared_wallet_file_password_file: PathBuf,
    /// Literal-loopback Taker wallet RPC whose primary address receives the sweep.
    #[arg(long)]
    taker_wallet_url: String,
    #[arg(long)]
    taker_wallet_username_file: PathBuf,
    #[arg(long)]
    taker_wallet_password_file: PathBuf,
    /// Literal-loopback funding wallet RPC whose address receives mined rewards.
    #[arg(long)]
    funding_wallet_url: String,
    #[arg(long)]
    funding_wallet_username_file: PathBuf,
    #[arg(long)]
    funding_wallet_password_file: PathBuf,
    /// Safe new filename created inside the configured shared-wallet directory.
    #[arg(long)]
    reconstructed_wallet_filename: String,
    /// Earliest Monero height scanned by the reconstructed wallet.
    #[arg(long, default_value_t = 0)]
    restore_height: u64,
    /// New owner-private JSON evidence file; reserved before the first RPC.
    #[arg(long)]
    output_evidence: PathBuf,
}

#[derive(Debug, Serialize)]
#[serde(deny_unknown_fields)]
struct SweepEvidence {
    schema: &'static str,
    run_id: String,
    agreement_commitment: String,
    monero_genesis_hash: String,
    shared_address: String,
    reconstructed_public_spend_key: String,
    destination_address: String,
    funded_amount_piconero: u64,
    received_amount_piconero: u64,
    fee_piconero: u64,
    transaction_id: String,
    containing_block_hash: String,
    containing_block_height: u64,
    confirmations: u64,
    stable_tip_hash: String,
    stable_tip_height: u64,
    generated_confirmation_tip_height: u64,
    required_confirmations: u64,
    peer_count: u64,
    restore_height: u64,
    revealed_role: &'static str,
    sweeping_role: &'static str,
    network_scope: &'static str,
    public_rpc_used: bool,
    faucet_used: bool,
    automatic_submission_retry: bool,
}

#[tokio::main]
async fn main() {
    if let Err(error) = execute(Arguments::parse()).await {
        eprintln!("M4 actual-local Monero claim sweep failed: {error:#}");
        std::process::exit(1);
    }
}

#[allow(clippy::too_many_lines)]
async fn execute(arguments: Arguments) -> Result<()> {
    for (url, label) in [
        (&arguments.daemon_url, "daemon"),
        (&arguments.shared_wallet_url, "shared wallet"),
        (&arguments.taker_wallet_url, "Taker wallet"),
        (&arguments.funding_wallet_url, "funding wallet"),
    ] {
        validate_loopback_http_endpoint(url)
            .with_context(|| format!("{label} endpoint is not a literal-loopback HTTP root"))?;
    }
    let agreement = XmrAgreementV1::from_wire(&read_owner_file(
        &arguments.agreement_wire_file,
        MAX_XMR_AGREEMENT_WIRE_BYTES,
        "Stage-A agreement",
    )?)
    .context("Stage-A agreement failed canonical validation")?;
    ensure!(
        agreement.body().monero().network() == MoneroAddressNetworkV1::Regtest,
        "Stage-A agreement is not Regtest"
    );
    let run_id = RunId::new(arguments.run_id.clone()).context("run ID is invalid")?;
    let retained_taker_share = CrossCurveScalar::from_monero_little_endian(*read_scalar_hex(
        &arguments.taker_share_file,
        "Taker Monero share",
    )?)
    .context("Taker Monero share is not a canonical cross-curve scalar")?;
    let extracted_maker_scalar = read_scalar_hex(
        &arguments.extracted_maker_adaptor_scalar_file,
        "extracted Maker adaptor scalar",
    )?;
    let view_key = read_view_key(&arguments.monero_view_key_file)?;
    ensure!(
        view_key.public_key() == agreement.shared_address().public_view_key(),
        "private view key does not match the Stage-A shared address"
    );
    let reconstructed = ReconstructedMoneroSpendKey::reconstruct(
        agreement.shared_address(),
        agreement.maker_proof(),
        retained_taker_share,
        extracted_maker_scalar,
    )
    .context("extracted Maker scalar cannot reconstruct the exact Stage-A spend key")?;
    let reconstructed_public_spend_key = hex::encode(reconstructed.public_key());

    let daemon = endpoint(
        &arguments.daemon_url,
        &arguments.daemon_username_file,
        &arguments.daemon_password_file,
        "daemon",
    )?;
    let shared_wallet = endpoint(
        &arguments.shared_wallet_url,
        &arguments.shared_wallet_username_file,
        &arguments.shared_wallet_password_file,
        "shared wallet",
    )?;
    let taker_wallet = endpoint(
        &arguments.taker_wallet_url,
        &arguments.taker_wallet_username_file,
        &arguments.taker_wallet_password_file,
        "Taker wallet",
    )?;
    let funding_wallet = endpoint(
        &arguments.funding_wallet_url,
        &arguments.funding_wallet_username_file,
        &arguments.funding_wallet_password_file,
        "funding wallet",
    )?;
    ensure_distinct_origins([&daemon, &shared_wallet, &taker_wallet, &funding_wallet])?;
    let reconstructed_wallet_password = read_secret_text(
        &arguments.shared_wallet_file_password_file,
        "reconstructed-wallet file password",
    )?;
    let identity = MoneroChainIdentity::new(
        MoneroNetwork::Regtest,
        agreement.body().monero().genesis_hash(),
    )
    .context("Stage-A Monero chain identity is invalid")?;
    let mut evidence_file = reserve_evidence(&arguments.output_evidence)?;

    let topology = MoneroTopologyVerifier::new(
        run_id.clone(),
        identity,
        &daemon,
        &taker_wallet,
        &shared_wallet,
    )
    .context("construct Taker receipt topology verifier")?
    .verify()
    .await
    .context("verify authenticated peerless Taker receipt topology")?;

    let taker = MoneroRegtestWalletEffects::new(&daemon, &taker_wallet)
        .context("Taker-wallet effect boundary is invalid")?;
    let destination = taker
        .primary_standard_address()
        .await
        .context("Taker primary destination is unavailable")?;
    let destination_address = destination.to_string();
    let funder = MoneroRegtestWalletEffects::new(&daemon, &funding_wallet)
        .context("funding-wallet effect boundary is invalid")?;
    let mining_address = funder
        .primary_standard_address()
        .await
        .context("funding-wallet mining address is unavailable")?;
    let sweeper = MoneroRegtestWalletEffects::new(&daemon, &shared_wallet)
        .context("shared-wallet effect boundary is invalid")?;
    let sweep = sweeper
        .restore_shared_and_sweep(
            agreement.shared_address(),
            reconstructed,
            view_key,
            arguments.reconstructed_wallet_filename,
            reconstructed_wallet_password.to_string(),
            arguments.restore_height,
            agreement.body().monero().amount_piconero(),
            destination,
            mining_address,
        )
        .await
        .context("official reconstructed-wallet sweep failed")?;
    taker
        .refresh_from_height(arguments.restore_height)
        .await
        .context("refresh Taker wallet after sweep confirmations")?;
    let expected_receipt = ExpectedMoneroOutput::new(
        sweep.transaction_id(),
        destination,
        sweep.received_amount_piconero(),
    )
    .context("construct exact expected Taker sweep receipt")?;
    let receipt = MoneroOutputVerifier::new(identity, &daemon, &taker_wallet)
        .context("construct Taker receipt verifier")?
        .verify(&expected_receipt)
        .await
        .context("verify canonical Taker sweep receipt")?;
    topology
        .validate_observation(&run_id, &receipt)
        .context("cross-bind Taker receipt to authenticated peerless topology")?;

    let evidence = SweepEvidence {
        schema: EVIDENCE_SCHEMA,
        run_id: arguments.run_id,
        agreement_commitment: hex::encode(agreement.agreement_commitment()),
        monero_genesis_hash: hex::encode(receipt.genesis_hash()),
        shared_address: agreement.shared_address().address_string(),
        reconstructed_public_spend_key,
        destination_address,
        funded_amount_piconero: sweep.funded_amount_piconero(),
        received_amount_piconero: sweep.received_amount_piconero(),
        fee_piconero: sweep.fee_piconero(),
        transaction_id: hex::encode(sweep.transaction_id().0),
        containing_block_hash: hex::encode(receipt.containing_block_hash()),
        containing_block_height: receipt.containing_block_height(),
        confirmations: receipt.confirmations(),
        stable_tip_hash: hex::encode(receipt.stable_tip_hash()),
        stable_tip_height: receipt.stable_tip_height(),
        generated_confirmation_tip_height: sweep.confirmation_tip_height(),
        required_confirmations: lez_xmr_monero_adapter::REQUIRED_MONERO_CONFIRMATIONS,
        peer_count: topology.peer_count(),
        restore_height: arguments.restore_height,
        revealed_role: "maker_claim_signature",
        sweeping_role: "taker",
        network_scope: "isolated_official_monero_regtest",
        public_rpc_used: false,
        faucet_used: false,
        automatic_submission_retry: false,
    };
    write_reserved_evidence(&mut evidence_file, &evidence)?;
    println!(
        "{}",
        serde_json::to_string(&evidence).context("encode sweep evidence")?
    );
    Ok(())
}

fn endpoint(
    url: &str,
    username_path: &Path,
    password_path: &Path,
    label: &'static str,
) -> Result<LoopbackRpcEndpoint> {
    let username = read_secret_text(username_path, label)?;
    let password = read_secret_text(password_path, label)?;
    LoopbackRpcEndpoint::new(url, username.to_string(), password.to_string())
        .with_context(|| format!("{label} endpoint or credentials are invalid"))
}

fn ensure_distinct_origins(endpoints: [&LoopbackRpcEndpoint; 4]) -> Result<()> {
    for (index, endpoint) in endpoints.iter().enumerate() {
        ensure!(
            endpoints[index + 1..]
                .iter()
                .all(|other| endpoint.base_url() != other.base_url()),
            "daemon and wallet RPC origins must all be distinct"
        );
    }
    Ok(())
}

fn read_view_key(path: &Path) -> Result<MoneroPrivateViewKey> {
    MoneroPrivateViewKey::from_monero_little_endian(*read_scalar_hex(path, "Monero view key")?)
        .context("Monero view key is not a canonical nonzero scalar")
}

fn read_scalar_hex(path: &Path, label: &'static str) -> Result<Zeroizing<[u8; 32]>> {
    let mut text = read_secret_text(path, label)?;
    ensure!(
        text.len() == SCALAR_HEX_BYTES
            && text
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "{label} is not exact lowercase hex"
    );
    let mut bytes = Zeroizing::new([0_u8; 32]);
    hex::decode_to_slice(text.as_bytes(), &mut *bytes)
        .with_context(|| format!("decode {label}"))?;
    text.zeroize();
    ensure!(*bytes != [0_u8; 32], "{label} is zero");
    Ok(bytes)
}

fn read_secret_text(path: &Path, label: &'static str) -> Result<Zeroizing<String>> {
    let bytes = read_owner_file(path, MAX_SECRET_BYTES, label)?;
    let mut text = String::from_utf8(bytes).with_context(|| format!("{label} is not UTF-8"))?;
    if text.ends_with('\n') {
        text.pop();
        if text.ends_with('\r') {
            text.pop();
        }
    }
    ensure!(!text.is_empty(), "{label} is empty");
    Ok(Zeroizing::new(text))
}

fn read_owner_file(path: &Path, maximum: usize, label: &'static str) -> Result<Vec<u8>> {
    let before = fs::symlink_metadata(path).with_context(|| format!("open {label}"))?;
    ensure!(
        before.file_type().is_file(),
        "{label} is not a regular file"
    );
    ensure!(
        before.uid() == rustix::process::getuid().as_raw()
            && before.nlink() == 1
            && before.permissions().mode().trailing_zeros() >= 6,
        "{label} is not owner-private and single-link"
    );
    ensure!(
        usize::try_from(before.len())
            .ok()
            .is_some_and(|length| length <= maximum),
        "{label} exceeds its size bound"
    );
    let mut opened = File::open(path).with_context(|| format!("open {label}"))?;
    let opened_before = opened.metadata().with_context(|| format!("stat {label}"))?;
    ensure!(
        same_file(&before, &opened_before),
        "{label} changed before read"
    );
    let mut bytes = Vec::with_capacity(usize::try_from(before.len()).unwrap_or(maximum));
    std::io::Read::by_ref(&mut opened)
        .take(u64::try_from(maximum).unwrap_or(u64::MAX).saturating_add(1))
        .read_to_end(&mut bytes)
        .with_context(|| format!("read {label}"))?;
    ensure!(bytes.len() <= maximum, "{label} exceeds its size bound");
    let opened_after = opened
        .metadata()
        .with_context(|| format!("restat {label}"))?;
    let path_after = fs::symlink_metadata(path).with_context(|| format!("restat {label}"))?;
    ensure!(
        same_file(&before, &opened_after) && same_file(&before, &path_after),
        "{label} changed during read"
    );
    Ok(bytes)
}

fn same_file(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    left.dev() == right.dev()
        && left.ino() == right.ino()
        && left.len() == right.len()
        && left.uid() == right.uid()
        && left.nlink() == right.nlink()
        && left.permissions().mode() == right.permissions().mode()
}

fn reserve_evidence(path: &Path) -> Result<File> {
    let parent = path.parent().context("sweep evidence has no parent")?;
    let metadata = fs::metadata(parent).context("inspect sweep evidence parent")?;
    ensure!(
        metadata.is_dir()
            && metadata.uid() == rustix::process::getuid().as_raw()
            && metadata.permissions().mode().trailing_zeros() >= 6,
        "sweep evidence parent is not owner-private"
    );
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .context("reserve new sweep evidence")
}

fn write_reserved_evidence(output: &mut File, evidence: &SweepEvidence) -> Result<()> {
    let mut bytes = serde_json::to_vec(evidence).context("encode sweep evidence")?;
    bytes.push(b'\n');
    output.write_all(&bytes).context("write sweep evidence")?;
    output.sync_all().context("sync sweep evidence")?;
    Ok(())
}
