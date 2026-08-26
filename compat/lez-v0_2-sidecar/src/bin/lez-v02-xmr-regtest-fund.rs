//! Actual-local M4 shared-wallet creation and exact Monero funding effect.

#![forbid(unsafe_code)]

#[cfg(not(target_os = "linux"))]
compile_error!("the M4 actual-local funding command requires Linux file-safety semantics");

use std::{
    fs::{self, File, OpenOptions},
    io::{Read as _, Seek as _, SeekFrom, Write as _},
    os::unix::fs::{MetadataExt as _, OpenOptionsExt as _, PermissionsExt as _},
    path::{Path, PathBuf},
};

use anyhow::{Context as _, Result, ensure};
use clap::Parser;
use lez_v0_2_sidecar::validate_loopback_http_endpoint;
use lez_xmr_monero_adapter::{LoopbackRpcEndpoint, MoneroRegtestWalletEffects};
use lez_xmr_swap_sdk::{
    MAX_XMR_AGREEMENT_WIRE_BYTES, MoneroAddressNetworkV1, MoneroPrivateViewKey, XmrAgreementV1,
};
use serde::Serialize;
use zeroize::{Zeroize as _, Zeroizing};

const EVIDENCE_SCHEMA: &str = "lez_v02_m4_actual_local_monero_funding_v2";
const MAX_SECRET_BYTES: usize = 256;
const VIEW_KEY_HEX_BYTES: usize = 64;

/// Create the exact Stage-A view wallet, fund it once, and mine confirmations.
#[derive(Debug, Parser)]
#[command(version, about)]
struct Arguments {
    /// Canonical countersigned Stage-A agreement wire.
    #[arg(long)]
    agreement_wire_file: PathBuf,
    /// Owner-private lowercase-hex Monero view key bound by Stage A.
    #[arg(long)]
    monero_view_key_file: PathBuf,
    /// Literal-loopback Digest-authenticated monerod origin.
    #[arg(long)]
    daemon_url: String,
    /// Owner-private monerod RPC username file.
    #[arg(long)]
    daemon_username_file: PathBuf,
    /// Owner-private monerod RPC password file.
    #[arg(long)]
    daemon_password_file: PathBuf,
    /// Literal-loopback funding-wallet RPC origin.
    #[arg(long)]
    funding_wallet_url: String,
    /// Owner-private funding-wallet RPC username file.
    #[arg(long)]
    funding_wallet_username_file: PathBuf,
    /// Owner-private funding-wallet RPC password file.
    #[arg(long)]
    funding_wallet_password_file: PathBuf,
    /// Literal-loopback wallet RPC that will become the shared view wallet.
    #[arg(long)]
    shared_wallet_url: String,
    /// Owner-private shared-wallet RPC username file.
    #[arg(long)]
    shared_wallet_username_file: PathBuf,
    /// Owner-private shared-wallet RPC password file.
    #[arg(long)]
    shared_wallet_password_file: PathBuf,
    /// Owner-private password file for the newly created shared wallet.
    #[arg(long)]
    shared_wallet_file_password_file: PathBuf,
    /// Safe filename created inside the configured wallet-RPC directory.
    #[arg(long)]
    shared_wallet_filename: String,
    /// Earliest Monero height scanned by the new view-only wallet.
    #[arg(long, default_value_t = 0)]
    restore_height: u64,
    /// New owner-private JSON evidence file; never overwritten.
    #[arg(long)]
    output_evidence: PathBuf,
}

#[derive(Debug, Serialize)]
#[serde(deny_unknown_fields)]
struct FundingEvidence {
    schema: &'static str,
    attempt_state: &'static str,
    agreement_commitment: String,
    shared_address: String,
    amount_piconero: u64,
    transaction_id: Option<String>,
    confirmation_tip_height: Option<u64>,
    required_confirmations: u64,
    restore_height: u64,
    wallet_role: &'static str,
    network_scope: &'static str,
    public_rpc_used: bool,
    faucet_used: bool,
    automatic_submission_retry: bool,
}

#[tokio::main]
async fn main() {
    match execute(Arguments::parse()).await {
        Ok(evidence) => {
            if let Ok(json) = serde_json::to_string(&evidence) {
                println!("{json}");
            } else {
                eprintln!("M4 Monero funding evidence encoding failed");
                std::process::exit(1);
            }
        }
        Err(error) => {
            eprintln!("M4 actual-local Monero funding failed: {error:#}");
            std::process::exit(1);
        }
    }
}

async fn execute(arguments: Arguments) -> Result<FundingEvidence> {
    validate_loopback_http_endpoint(&arguments.daemon_url)
        .context("daemon endpoint is not a literal-loopback HTTP root")?;
    validate_loopback_http_endpoint(&arguments.funding_wallet_url)
        .context("funding-wallet endpoint is not a literal-loopback HTTP root")?;
    validate_loopback_http_endpoint(&arguments.shared_wallet_url)
        .context("shared-wallet endpoint is not a literal-loopback HTTP root")?;
    let agreement_wire = read_owner_file(
        &arguments.agreement_wire_file,
        MAX_XMR_AGREEMENT_WIRE_BYTES,
        "Stage-A agreement",
    )?;
    let agreement = XmrAgreementV1::from_wire(&agreement_wire)
        .context("Stage-A agreement failed canonical validation")?;
    ensure!(
        agreement.body().monero().network() == MoneroAddressNetworkV1::Regtest,
        "Stage-A agreement is not Regtest"
    );
    let view_key = read_view_key(&arguments.monero_view_key_file)?;
    ensure!(
        view_key.public_key() == agreement.shared_address().public_view_key(),
        "private view key does not match the Stage-A shared address"
    );

    let daemon = endpoint(
        &arguments.daemon_url,
        &arguments.daemon_username_file,
        &arguments.daemon_password_file,
        "daemon",
    )?;
    let funding_wallet = endpoint(
        &arguments.funding_wallet_url,
        &arguments.funding_wallet_username_file,
        &arguments.funding_wallet_password_file,
        "funding wallet",
    )?;
    let shared_wallet = endpoint(
        &arguments.shared_wallet_url,
        &arguments.shared_wallet_username_file,
        &arguments.shared_wallet_password_file,
        "shared wallet",
    )?;
    ensure_distinct_origins([&daemon, &funding_wallet, &shared_wallet])?;
    let wallet_file_password = read_secret_text(
        &arguments.shared_wallet_file_password_file,
        "shared-wallet file password",
    )?;
    let mut evidence_file = reserve_evidence(&arguments.output_evidence)?;
    let mut evidence = FundingEvidence {
        schema: EVIDENCE_SCHEMA,
        attempt_state: "attempt_started_or_delivery_unknown",
        agreement_commitment: hex::encode(agreement.agreement_commitment()),
        shared_address: agreement.shared_address().address_string(),
        amount_piconero: agreement.body().monero().amount_piconero(),
        transaction_id: None,
        confirmation_tip_height: None,
        required_confirmations: lez_xmr_monero_adapter::REQUIRED_MONERO_CONFIRMATIONS,
        restore_height: arguments.restore_height,
        wallet_role: "stage_a_shared_view_only",
        network_scope: "isolated_official_monero_regtest",
        public_rpc_used: false,
        faucet_used: false,
        automatic_submission_retry: false,
    };
    write_evidence(&mut evidence_file, &evidence)?;

    let observer = MoneroRegtestWalletEffects::new(&daemon, &shared_wallet)
        .context("shared-wallet effect boundary is invalid")?;
    observer
        .restore_shared_view_only(
            agreement.shared_address(),
            view_key,
            arguments.shared_wallet_filename,
            wallet_file_password.to_string(),
            arguments.restore_height,
        )
        .await
        .context("official shared view-wallet creation failed")?;

    let funder = MoneroRegtestWalletEffects::new(&daemon, &funding_wallet)
        .context("funding-wallet effect boundary is invalid")?;
    // A prior accepted application may have mined confirmations after spending
    // from this same Maker wallet. Refresh is observation-only and makes the
    // unlocked change visible before the next one-shot transfer.
    funder
        .refresh_from_height(arguments.restore_height)
        .await
        .context("funding wallet refresh before transfer failed")?;
    let funding = funder
        .fund_shared_exact_and_confirm(
            agreement.shared_address(),
            agreement.body().monero().amount_piconero(),
        )
        .await
        .context("exact shared-wallet funding failed")?;
    observer
        .refresh_from_height(arguments.restore_height)
        .await
        .context("shared view-wallet refresh failed")?;

    evidence.attempt_state = "confirmed";
    evidence.transaction_id = Some(hex::encode(funding.transaction_id().0));
    evidence.confirmation_tip_height = Some(funding.confirmation_tip_height());
    write_evidence(&mut evidence_file, &evidence)?;
    Ok(evidence)
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

fn ensure_distinct_origins(endpoints: [&LoopbackRpcEndpoint; 3]) -> Result<()> {
    for (index, endpoint) in endpoints.iter().enumerate() {
        ensure!(
            endpoints[index + 1..]
                .iter()
                .all(|other| endpoint.base_url() != other.base_url()),
            "daemon, funding-wallet, and shared-wallet origins must be distinct"
        );
    }
    Ok(())
}

fn read_view_key(path: &Path) -> Result<MoneroPrivateViewKey> {
    let mut text = read_secret_text(path, "Monero view key")?;
    ensure!(
        text.len() == VIEW_KEY_HEX_BYTES
            && text
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "Monero view key is not exact lowercase hex"
    );
    let mut bytes = Zeroizing::new([0_u8; 32]);
    hex::decode_to_slice(text.as_bytes(), &mut *bytes).context("decode Monero view key")?;
    text.zeroize();
    MoneroPrivateViewKey::from_monero_little_endian(*bytes)
        .context("Monero view key is not a canonical nonzero scalar")
}

fn read_secret_text(path: &Path, label: &'static str) -> Result<Zeroizing<String>> {
    let bytes = Zeroizing::new(read_owner_file(path, MAX_SECRET_BYTES, label)?);
    let mut text = std::str::from_utf8(&bytes)
        .with_context(|| format!("{label} is not UTF-8"))?
        .to_owned();
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
    let parent = path.parent().context("funding evidence has no parent")?;
    let metadata = fs::metadata(parent).context("inspect funding evidence parent")?;
    ensure!(
        metadata.is_dir()
            && metadata.uid() == rustix::process::getuid().as_raw()
            && metadata.permissions().mode().trailing_zeros() >= 6,
        "funding evidence parent is not owner-private"
    );
    OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .context("reserve new funding attempt evidence")
}

fn write_evidence(output: &mut File, evidence: &FundingEvidence) -> Result<()> {
    let mut bytes = serde_json::to_vec(evidence).context("encode funding evidence")?;
    bytes.push(b'\n');
    output
        .set_len(0)
        .context("truncate funding evidence state")?;
    output
        .seek(SeekFrom::Start(0))
        .context("rewind funding evidence state")?;
    output.write_all(&bytes).context("write funding evidence")?;
    output.sync_all().context("sync funding evidence")?;
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;

    fn evidence(state: &'static str) -> FundingEvidence {
        FundingEvidence {
            schema: EVIDENCE_SCHEMA,
            attempt_state: state,
            agreement_commitment: "11".repeat(32),
            shared_address: "fixture-address".to_owned(),
            amount_piconero: 1_000_000,
            transaction_id: None,
            confirmation_tip_height: None,
            required_confirmations: 10,
            restore_height: 0,
            wallet_role: "stage_a_shared_view_only",
            network_scope: "isolated_official_monero_regtest",
            public_rpc_used: false,
            faucet_used: false,
            automatic_submission_retry: false,
        }
    }

    #[test]
    fn funding_attempt_is_create_new_and_durably_transitions() {
        let temporary = tempfile::tempdir().expect("temporary owner directory");
        fs::set_permissions(temporary.path(), fs::Permissions::from_mode(0o700))
            .expect("owner-only temporary directory");
        let path = temporary.path().join("funding-attempt.json");
        let mut file = reserve_evidence(&path).expect("reserve first attempt");
        let mut state = evidence("attempt_started_or_delivery_unknown");
        write_evidence(&mut file, &state).expect("persist unknown-delivery state");
        assert!(reserve_evidence(&path).is_err());
        let initial: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).expect("read initial state"))
                .expect("decode initial state");
        assert_eq!(
            initial["attempt_state"],
            "attempt_started_or_delivery_unknown"
        );

        state.attempt_state = "confirmed";
        state.transaction_id = Some("22".repeat(32));
        state.confirmation_tip_height = Some(42);
        write_evidence(&mut file, &state).expect("persist confirmed state");
        let confirmed: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).expect("read confirmed state"))
                .expect("decode confirmed state");
        assert_eq!(confirmed["attempt_state"], "confirmed");
        assert_eq!(confirmed["confirmation_tip_height"], 42);
    }
    #[test]
    fn canonical_origin_aliases_are_rejected() {
        let daemon = LoopbackRpcEndpoint::new("http://127.0.0.1:18081", "user", "pass")
            .expect("daemon endpoint");
        let alias = LoopbackRpcEndpoint::new("http://127.0.0.1:18081/", "other", "pass")
            .expect("canonical alias endpoint");
        let distinct = LoopbackRpcEndpoint::new("http://127.0.0.1:18082", "user", "pass")
            .expect("distinct endpoint");
        assert!(ensure_distinct_origins([&daemon, &alias, &distinct]).is_err());

        let third = LoopbackRpcEndpoint::new("http://127.0.0.1:18083", "user", "pass")
            .expect("third endpoint");
        assert!(ensure_distinct_origins([&daemon, &distinct, &third]).is_ok());
    }
}
