//! Actual-local Monero key reconstruction and role-correct sweep effect.

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
use clap::{Parser, ValueEnum};
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

const CLAIM_EVIDENCE_SCHEMA: &str = "lez_v02_m4_actual_local_monero_claim_sweep_v2";
const REFUND_EVIDENCE_SCHEMA: &str = "lez_v02_m5_actual_local_monero_refund_sweep_v3";
const MAX_SECRET_BYTES: usize = 256;
const SCALAR_HEX_BYTES: usize = 64;

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum Journey {
    Claim,
    Refund,
}

/// Reconstruct the Stage-A spend key and sweep the exact shared output once.
#[derive(Debug, Parser)]
#[command(version, about)]
struct Arguments {
    /// Role-correct settlement path. Claim remains the compatibility default.
    #[arg(long, value_enum)]
    journey: Option<Journey>,
    /// Same-swap run identity used by the authenticated topology capability.
    #[arg(long)]
    run_id: String,
    /// Canonical countersigned Stage-A agreement wire.
    #[arg(long)]
    agreement_wire_file: PathBuf,
    /// Claim-only Taker Monero spend-key share in lowercase little-endian hex.
    #[arg(
        long,
        required_unless_present = "journey",
        required_if_eq("journey", "claim"),
        conflicts_with_all = ["maker_share_file", "extracted_taker_adaptor_scalar_file"]
    )]
    taker_share_file: Option<PathBuf>,
    /// Claim-only extracted Maker adaptor scalar in lowercase big-endian hex.
    #[arg(
        long,
        required_unless_present = "journey",
        required_if_eq("journey", "claim"),
        conflicts_with_all = ["maker_share_file", "extracted_taker_adaptor_scalar_file"]
    )]
    extracted_maker_adaptor_scalar_file: Option<PathBuf>,
    /// Refund-only Maker Monero spend-key share in lowercase little-endian hex.
    #[arg(
        long,
        required_if_eq("journey", "refund"),
        conflicts_with_all = ["taker_share_file", "extracted_maker_adaptor_scalar_file"]
    )]
    maker_share_file: Option<PathBuf>,
    /// Refund-only extracted Taker adaptor scalar in lowercase big-endian hex.
    #[arg(
        long,
        required_if_eq("journey", "refund"),
        conflicts_with_all = ["taker_share_file", "extracted_maker_adaptor_scalar_file"]
    )]
    extracted_taker_adaptor_scalar_file: Option<PathBuf>,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    journey: Option<&'static str>,
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

struct KeyMaterial<'a> {
    retained_share_file: &'a Path,
    retained_share_label: &'static str,
    extracted_scalar_file: &'a Path,
    extracted_scalar_label: &'static str,
}

struct WalletRoles<'a> {
    destination: &'a LoopbackRpcEndpoint,
    confirmation_miner: &'a LoopbackRpcEndpoint,
    evidence_schema: &'static str,
    evidence_journey: Option<&'static str>,
    revealed_role: &'static str,
    sweeping_role: &'static str,
}

fn select_key_material(arguments: &Arguments) -> Result<KeyMaterial<'_>> {
    match arguments.journey() {
        Journey::Claim => {
            ensure!(
                arguments.maker_share_file.is_none()
                    && arguments.extracted_taker_adaptor_scalar_file.is_none(),
                "claim journey received refund-only key material"
            );
            Ok(KeyMaterial {
                retained_share_file: arguments
                    .taker_share_file
                    .as_deref()
                    .context("claim journey requires --taker-share-file")?,
                retained_share_label: "Taker Monero share",
                extracted_scalar_file: arguments
                    .extracted_maker_adaptor_scalar_file
                    .as_deref()
                    .context("claim journey requires --extracted-maker-adaptor-scalar-file")?,
                extracted_scalar_label: "extracted Maker adaptor scalar",
            })
        }
        Journey::Refund => {
            ensure!(
                arguments.taker_share_file.is_none()
                    && arguments.extracted_maker_adaptor_scalar_file.is_none(),
                "refund journey received claim-only key material"
            );
            Ok(KeyMaterial {
                retained_share_file: arguments
                    .maker_share_file
                    .as_deref()
                    .context("refund journey requires --maker-share-file")?,
                retained_share_label: "Maker Monero share",
                extracted_scalar_file: arguments
                    .extracted_taker_adaptor_scalar_file
                    .as_deref()
                    .context("refund journey requires --extracted-taker-adaptor-scalar-file")?,
                extracted_scalar_label: "extracted Taker adaptor scalar",
            })
        }
    }
}

fn select_wallet_roles<'a>(
    journey: Journey,
    taker_wallet: &'a LoopbackRpcEndpoint,
    maker_wallet: &'a LoopbackRpcEndpoint,
) -> WalletRoles<'a> {
    match journey {
        Journey::Claim => WalletRoles {
            destination: taker_wallet,
            confirmation_miner: maker_wallet,
            evidence_schema: CLAIM_EVIDENCE_SCHEMA,
            evidence_journey: None,
            revealed_role: "maker_claim_signature",
            sweeping_role: "taker",
        },
        Journey::Refund => WalletRoles {
            destination: maker_wallet,
            confirmation_miner: taker_wallet,
            evidence_schema: REFUND_EVIDENCE_SCHEMA,
            evidence_journey: Some("refund"),
            revealed_role: "taker_refund_signature",
            sweeping_role: "maker",
        },
    }
}

#[tokio::main]
async fn main() {
    if let Err(error) = execute(Arguments::parse()).await {
        eprintln!("actual-local Monero sweep failed: {error:#}");
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
    let key_material = select_key_material(&arguments)?;
    let retained_share = CrossCurveScalar::from_monero_little_endian(*read_scalar_hex(
        key_material.retained_share_file,
        key_material.retained_share_label,
    )?)
    .with_context(|| {
        format!(
            "{} is not a canonical cross-curve scalar",
            key_material.retained_share_label
        )
    })?;
    let extracted_counterparty_scalar = read_scalar_hex(
        key_material.extracted_scalar_file,
        key_material.extracted_scalar_label,
    )?;
    let view_key = read_view_key(&arguments.monero_view_key_file)?;
    ensure!(
        view_key.public_key() == agreement.shared_address().public_view_key(),
        "private view key does not match the Stage-A shared address"
    );
    let reconstructed = ReconstructedMoneroSpendKey::reconstruct(
        agreement.shared_address(),
        match arguments.journey() {
            Journey::Claim => agreement.maker_proof(),
            Journey::Refund => agreement.taker_proof(),
        },
        retained_share,
        extracted_counterparty_scalar,
    )
    .context("extracted counterparty scalar cannot reconstruct the exact Stage-A spend key")?;
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

    let wallet_roles = select_wallet_roles(arguments.journey(), &taker_wallet, &funding_wallet);
    let topology = MoneroTopologyVerifier::new(
        run_id.clone(),
        identity,
        &daemon,
        wallet_roles.destination,
        &shared_wallet,
    )
    .context("construct destination receipt topology verifier")?
    .verify()
    .await
    .context("verify authenticated peerless destination receipt topology")?;

    let destination_wallet = MoneroRegtestWalletEffects::new(&daemon, wallet_roles.destination)
        .context("destination-wallet effect boundary is invalid")?;
    let destination = destination_wallet
        .primary_standard_address()
        .await
        .context("primary destination is unavailable")?;
    let destination_address = destination.to_string();
    let confirmation_miner =
        MoneroRegtestWalletEffects::new(&daemon, wallet_roles.confirmation_miner)
            .context("confirmation-mining wallet effect boundary is invalid")?;
    let mining_address = confirmation_miner
        .primary_standard_address()
        .await
        .context("confirmation-mining wallet address is unavailable")?;
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
    destination_wallet
        .refresh_from_height(arguments.restore_height)
        .await
        .context("refresh destination wallet after sweep confirmations")?;
    let expected_receipt = ExpectedMoneroOutput::new(
        sweep.transaction_id(),
        destination,
        sweep.received_amount_piconero(),
    )
    .context("construct exact expected sweep receipt")?;
    let receipt = MoneroOutputVerifier::new(identity, &daemon, wallet_roles.destination)
        .context("construct destination receipt verifier")?
        .verify(&expected_receipt)
        .await
        .context("verify canonical sweep receipt")?;
    topology
        .validate_observation(&run_id, &receipt)
        .context("cross-bind receipt to authenticated peerless topology")?;

    let evidence = SweepEvidence {
        schema: wallet_roles.evidence_schema,
        journey: wallet_roles.evidence_journey,
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
        revealed_role: wallet_roles.revealed_role,
        sweeping_role: wallet_roles.sweeping_role,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn common_arguments() -> Vec<&'static str> {
        vec![
            "lez-v02-xmr-regtest-sweep",
            "--run-id",
            "m5-refund-unit",
            "--agreement-wire-file",
            "/private/agreement",
            "--monero-view-key-file",
            "/private/view-key",
            "--daemon-url",
            "http://127.0.0.1:18081",
            "--daemon-username-file",
            "/private/daemon-user",
            "--daemon-password-file",
            "/private/daemon-password",
            "--shared-wallet-url",
            "http://127.0.0.1:18082",
            "--shared-wallet-username-file",
            "/private/shared-user",
            "--shared-wallet-password-file",
            "/private/shared-password",
            "--shared-wallet-file-password-file",
            "/private/reconstructed-password",
            "--taker-wallet-url",
            "http://127.0.0.1:18083",
            "--taker-wallet-username-file",
            "/private/taker-user",
            "--taker-wallet-password-file",
            "/private/taker-password",
            "--funding-wallet-url",
            "http://127.0.0.1:18084",
            "--funding-wallet-username-file",
            "/private/maker-user",
            "--funding-wallet-password-file",
            "/private/maker-password",
            "--reconstructed-wallet-filename",
            "reconstructed",
            "--output-evidence",
            "/private/evidence.json",
        ]
    }

    fn claim_key_arguments() -> [&'static str; 4] {
        [
            "--taker-share-file",
            "/private/taker-share",
            "--extracted-maker-adaptor-scalar-file",
            "/private/maker-scalar",
        ]
    }

    fn refund_key_arguments() -> [&'static str; 4] {
        [
            "--maker-share-file",
            "/private/maker-share",
            "--extracted-taker-adaptor-scalar-file",
            "/private/taker-scalar",
        ]
    }

    #[test]
    fn legacy_claim_cli_remains_the_default_and_requires_exact_claim_keys() {
        let mut legacy = common_arguments();
        legacy.extend(claim_key_arguments());
        let parsed = Arguments::try_parse_from(legacy).expect("legacy claim CLI remains accepted");
        assert_eq!(parsed.journey(), Journey::Claim);

        let keys = select_key_material(&parsed).expect("claim key selection");
        assert_eq!(keys.retained_share_file, Path::new("/private/taker-share"));
        assert_eq!(
            keys.extracted_scalar_file,
            Path::new("/private/maker-scalar")
        );

        assert!(Arguments::try_parse_from(common_arguments()).is_err());
        let mut refund_only = common_arguments();
        refund_only.extend(refund_key_arguments());
        assert!(Arguments::try_parse_from(refund_only).is_err());
    }

    #[test]
    fn refund_cli_requires_exact_refund_keys_and_rejects_claim_keys() {
        let mut refund = common_arguments();
        refund.extend(["--journey", "refund"]);
        refund.extend(refund_key_arguments());
        let parsed = Arguments::try_parse_from(refund).expect("refund CLI is accepted");
        assert_eq!(parsed.journey(), Journey::Refund);

        let keys = select_key_material(&parsed).expect("refund key selection");
        assert_eq!(keys.retained_share_file, Path::new("/private/maker-share"));
        assert_eq!(
            keys.extracted_scalar_file,
            Path::new("/private/taker-scalar")
        );

        let mut missing = common_arguments();
        missing.extend(["--journey", "refund"]);
        assert!(Arguments::try_parse_from(missing).is_err());
        let mut claim_only = common_arguments();
        claim_only.extend(["--journey", "refund"]);
        claim_only.extend(claim_key_arguments());
        assert!(Arguments::try_parse_from(claim_only).is_err());
    }

    #[test]
    fn claim_and_refund_choose_opposite_wallet_and_evidence_roles() {
        let taker = LoopbackRpcEndpoint::new("http://127.0.0.1:18083", "taker", "secret")
            .expect("Taker endpoint");
        let maker = LoopbackRpcEndpoint::new("http://127.0.0.1:18084", "maker", "secret")
            .expect("Maker endpoint");

        let claim = select_wallet_roles(Journey::Claim, &taker, &maker);
        assert_eq!(claim.destination.base_url(), taker.base_url());
        assert_eq!(claim.confirmation_miner.base_url(), maker.base_url());
        assert_eq!(claim.evidence_schema, CLAIM_EVIDENCE_SCHEMA);
        assert_eq!(claim.evidence_journey, None);
        assert_eq!(claim.revealed_role, "maker_claim_signature");
        assert_eq!(claim.sweeping_role, "taker");

        let refund = select_wallet_roles(Journey::Refund, &taker, &maker);
        assert_eq!(refund.destination.base_url(), maker.base_url());
        assert_eq!(refund.confirmation_miner.base_url(), taker.base_url());
        assert_eq!(refund.evidence_schema, REFUND_EVIDENCE_SCHEMA);
        assert_eq!(refund.evidence_journey, Some("refund"));
        assert_eq!(refund.revealed_role, "taker_refund_signature");
        assert_eq!(refund.sweeping_role, "maker");
    }
}

impl Arguments {
    fn journey(&self) -> Journey {
        self.journey.unwrap_or(Journey::Claim)
    }
}

#[cfg(test)]
mod evidence_tests {
    use super::*;

    fn sample_evidence(wallet_roles: &WalletRoles<'_>) -> SweepEvidence {
        SweepEvidence {
            schema: wallet_roles.evidence_schema,
            journey: wallet_roles.evidence_journey,
            run_id: "run".into(),
            agreement_commitment: "agreement".into(),
            monero_genesis_hash: "genesis".into(),
            shared_address: "shared".into(),
            reconstructed_public_spend_key: "spend".into(),
            destination_address: "destination".into(),
            funded_amount_piconero: 10,
            received_amount_piconero: 9,
            fee_piconero: 1,
            transaction_id: "transaction".into(),
            containing_block_hash: "containing".into(),
            containing_block_height: 1,
            confirmations: 2,
            stable_tip_hash: "tip".into(),
            stable_tip_height: 3,
            generated_confirmation_tip_height: 3,
            required_confirmations: 2,
            peer_count: 0,
            restore_height: 0,
            revealed_role: wallet_roles.revealed_role,
            sweeping_role: wallet_roles.sweeping_role,
            network_scope: "isolated_official_monero_regtest",
            public_rpc_used: false,
            faucet_used: false,
            automatic_submission_retry: false,
        }
    }

    #[test]
    fn claim_keeps_v2_evidence_shape_while_refund_is_honest_v3() {
        let taker = LoopbackRpcEndpoint::new("http://127.0.0.1:18083", "taker", "secret")
            .expect("Taker endpoint");
        let maker = LoopbackRpcEndpoint::new("http://127.0.0.1:18084", "maker", "secret")
            .expect("Maker endpoint");

        let claim_roles = select_wallet_roles(Journey::Claim, &taker, &maker);
        let claim = serde_json::to_value(sample_evidence(&claim_roles)).expect("claim evidence");
        assert_eq!(claim["schema"], CLAIM_EVIDENCE_SCHEMA);
        assert!(claim.get("journey").is_none());
        assert_eq!(claim["revealed_role"], "maker_claim_signature");
        assert_eq!(claim["sweeping_role"], "taker");

        let refund_roles = select_wallet_roles(Journey::Refund, &taker, &maker);
        let refund = serde_json::to_value(sample_evidence(&refund_roles)).expect("refund evidence");
        assert_eq!(refund["schema"], REFUND_EVIDENCE_SCHEMA);
        assert_eq!(refund["journey"], "refund");
        assert_eq!(refund["revealed_role"], "taker_refund_signature");
        assert_eq!(refund["sweeping_role"], "maker");
    }
}
