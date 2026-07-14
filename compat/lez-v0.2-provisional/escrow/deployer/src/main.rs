use std::{collections::BTreeMap, net::IpAddr, time::Duration};

use anyhow::{Context as _, Result, ensure};
use clap::{Parser, Subcommand};
use common::{HashType, transaction::LeeTransaction};
use lez_zec_escrow_v02_methods::{ZEC_ESCROW_V02_ELF, ZEC_ESCROW_V02_ID};
use nssa::{ProgramDeploymentTransaction, program::Program};
use sequencer_service_rpc::{RpcClient as _, SequencerClient, SequencerClientBuilder};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use url::{Host, Url};

const MANIFEST: &str = include_str!("../../methods/guest/deployment-manifest.toml");
const OFFICIAL_RPC_URL: &str = "https://testnet.lez.logos.co";
const OFFICIAL_CHANNEL_ID: &str =
    "0101010101010101010101010101010101010101010101010101010101010101";
const OFFICIAL_AUTHENTICATED_TRANSFER_PROGRAM_ID: [u32; 8] = [
    3_170_810_844,
    2_526_647_253,
    999_807_262,
    1_205_602_179,
    3_401_962_591,
    3_484_055_895,
    2_106_546_407,
    1_900_691_388,
];
const OFFICIAL_TOKEN_PROGRAM_ID: [u32; 8] = [
    2_282_739_141,
    348_907_455,
    1_046_946_228,
    3_735_699_860,
    585_462_133,
    3_426_087_150,
    772_528_164,
    2_090_518_099,
];
const OFFICIAL_ASSOCIATED_TOKEN_ACCOUNT_PROGRAM_ID: [u32; 8] = [
    3_357_312_149,
    3_615_960_253,
    3_351_583_505,
    2_234_166_003,
    4_153_433_811,
    2_743_238_177,
    2_886_052_503,
    4_160_755_157,
];
const OFFICIAL_ASSOCIATED_TOKEN_ACCOUNT_IDENTITY_SOURCE: &str =
    "lez-v0.2.0-checked-elf-rpc-map-omits-key";

#[derive(Debug, Parser)]
#[command(about = "Fail-closed LEZ v0.2 escrow deployment and observation")]
struct Arguments {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Validate the checked artifact and official testnet identities without mutation.
    Inspect,
    /// Submit the checked deployment exactly once and observe canonical inclusion.
    Deploy {
        /// Maximum wall time for observation after the one submission attempt.
        #[arg(long, default_value_t = 300)]
        timeout_seconds: u64,
    },
    /// Submit the same checked artifact to one explicit isolated local sequencer.
    DeployLocal {
        /// Dynamic loopback HTTP endpoint emitted by the local stack manifest.
        #[arg(long)]
        rpc_url: String,
        /// Exact nonzero channel identity emitted by the local stack manifest.
        #[arg(long)]
        channel_id: String,
        /// Maximum wall time for observation after the one submission attempt.
        #[arg(long, default_value_t = 300)]
        timeout_seconds: u64,
    },
}

#[derive(Debug, Deserialize)]
struct DeploymentManifest {
    artifact_status: String,
    target: Target,
    artifact: Artifact,
}

#[derive(Debug, Deserialize)]
struct Target {
    rpc_url: String,
    channel_id: String,
    authenticated_transfer_program_id: [u32; 8],
    token_program_id: [u32; 8],
    associated_token_account_program_id: [u32; 8],
    associated_token_account_identity_source: String,
}

#[derive(Debug, Deserialize)]
struct Artifact {
    elf_sha256: String,
    image_id: String,
    program_id_words: [u32; 8],
}

#[derive(Debug, Serialize)]
struct PreflightEvidence {
    rpc_url: String,
    channel_id: String,
    elf_sha256: String,
    image_id: String,
    program_id_words: [u32; 8],
    authenticated_transfer_program_id: [u32; 8],
    token_program_id: [u32; 8],
    associated_token_account_program_id: [u32; 8],
    associated_token_account_identity_source: String,
    rpc_program_names: Vec<String>,
    last_block_id: u64,
}

#[derive(Debug, Serialize)]
struct DeploymentEvidence {
    preflight: PreflightEvidence,
    transaction_hash: String,
    inclusion_block_id: u64,
    inclusion_block_hash: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    let arguments = Arguments::parse();
    let manifest: DeploymentManifest =
        toml::from_str(MANIFEST).context("parse checked deployment manifest")?;

    match arguments.command {
        Command::Inspect => {
            let (_, preflight) = preflight(&manifest).await?;
            println!("{}", serde_json::to_string_pretty(&preflight)?);
        }
        Command::Deploy { timeout_seconds } => {
            ensure!(timeout_seconds > 0, "timeout-seconds must be non-zero");
            let (client, preflight) = preflight(&manifest).await?;
            let evidence = deploy_once_and_observe(client, preflight, timeout_seconds).await?;
            println!("{}", serde_json::to_string_pretty(&evidence)?);
        }
        Command::DeployLocal {
            rpc_url,
            channel_id,
            timeout_seconds,
        } => {
            ensure!(timeout_seconds > 0, "timeout-seconds must be non-zero");
            validate_local_rpc_url(&rpc_url)?;
            validate_channel_id(&channel_id)?;
            let client = bounded_client(&rpc_url)?;
            let preflight =
                preflight_local_with_client(&manifest, &client, &rpc_url, &channel_id).await?;
            let evidence = deploy_once_and_observe(client, preflight, timeout_seconds).await?;
            println!("{}", serde_json::to_string_pretty(&evidence)?);
        }
    }
    Ok(())
}

fn checked_artifact() -> Result<(String, String)> {
    ensure!(!ZEC_ESCROW_V02_ELF.is_empty(), "checked guest ELF is empty");
    let program = Program::new(ZEC_ESCROW_V02_ELF.into())
        .context("checked guest must decode as a canonical Risc0 program")?;
    ensure!(
        program.id() == ZEC_ESCROW_V02_ID,
        "embedded ImageID does not match the checked ELF"
    );
    let elf_sha256 = hex::encode(Sha256::digest(ZEC_ESCROW_V02_ELF));
    let image_id = risc0_zkvm::Digest::from(program.id()).to_string();
    Ok((elf_sha256, image_id))
}

async fn preflight(manifest: &DeploymentManifest) -> Result<(SequencerClient, PreflightEvidence)> {
    validate_immutable_target(manifest)?;
    let client = bounded_client(OFFICIAL_RPC_URL)?;
    let evidence = preflight_with_client(manifest, &client).await?;
    Ok((client, evidence))
}

fn bounded_client(rpc_url: &str) -> Result<SequencerClient> {
    SequencerClientBuilder::default()
        .max_request_size(16 * 1024 * 1024)
        .max_response_size(16 * 1024 * 1024)
        .request_timeout(Duration::from_secs(30))
        .max_concurrent_requests(1)
        .build(rpc_url)
        .context("build bounded LEZ RPC client")
}

fn validate_immutable_target(manifest: &DeploymentManifest) -> Result<(String, String)> {
    ensure!(
        manifest.artifact_status == "locally-built-artifact-checked",
        "immutable manifest artifact status is not locally checked"
    );
    ensure!(
        manifest.target.rpc_url == OFFICIAL_RPC_URL,
        "immutable manifest RPC URL differs from the accepted official LEZ v0.2 testnet"
    );
    ensure!(
        manifest.target.channel_id == OFFICIAL_CHANNEL_ID,
        "immutable manifest channel ID differs from the accepted LEZ v0.2 testnet"
    );
    ensure!(
        manifest.target.authenticated_transfer_program_id
            == OFFICIAL_AUTHENTICATED_TRANSFER_PROGRAM_ID,
        "immutable manifest authenticated-transfer ProgramId differs from checked LEZ v0.2"
    );
    ensure!(
        manifest.target.token_program_id == OFFICIAL_TOKEN_PROGRAM_ID,
        "immutable manifest token ProgramId differs from checked LEZ v0.2"
    );
    ensure!(
        manifest.target.associated_token_account_program_id
            == OFFICIAL_ASSOCIATED_TOKEN_ACCOUNT_PROGRAM_ID,
        "immutable manifest ATA ProgramId differs from the checked LEZ v0.2 ELF"
    );
    ensure!(
        manifest.target.associated_token_account_identity_source
            == OFFICIAL_ASSOCIATED_TOKEN_ACCOUNT_IDENTITY_SOURCE,
        "immutable manifest ATA provenance must disclose the public RPC omission"
    );
    let (elf_sha256, image_id) = checked_artifact()?;
    ensure!(
        manifest.artifact.elf_sha256 == elf_sha256,
        "immutable manifest ELF SHA-256 differs from the checked guest"
    );
    ensure!(
        manifest.artifact.image_id == image_id,
        "immutable manifest ImageID differs from the checked guest"
    );
    ensure!(
        manifest.artifact.program_id_words == ZEC_ESCROW_V02_ID,
        "immutable manifest ProgramId words differ from the checked guest"
    );
    Ok((elf_sha256, image_id))
}

async fn preflight_with_client(
    manifest: &DeploymentManifest,
    client: &SequencerClient,
) -> Result<PreflightEvidence> {
    preflight_checked_target(
        manifest,
        client,
        &manifest.target.rpc_url,
        &manifest.target.channel_id,
    )
    .await
}

async fn preflight_local_with_client(
    manifest: &DeploymentManifest,
    client: &SequencerClient,
    rpc_url: &str,
    channel_id: &str,
) -> Result<PreflightEvidence> {
    validate_local_rpc_url(rpc_url)?;
    validate_channel_id(channel_id)?;
    preflight_checked_target(manifest, client, rpc_url, channel_id).await
}

async fn preflight_checked_target(
    manifest: &DeploymentManifest,
    client: &SequencerClient,
    rpc_url: &str,
    expected_channel_id: &str,
) -> Result<PreflightEvidence> {
    let (elf_sha256, image_id) = validate_immutable_target(manifest)?;
    client
        .check_health()
        .await
        .context("LEZ health preflight")?;
    let channel_id = client
        .get_channel_id()
        .await
        .context("read LEZ channel ID")?
        .to_string();
    ensure!(
        channel_id == expected_channel_id,
        "LEZ channel ID differs from the selected deployment target"
    );
    let program_ids = client
        .get_program_ids()
        .await
        .context("read official LEZ program IDs")?;
    require_rpc_program(
        &program_ids,
        "authenticated_transfer",
        manifest.target.authenticated_transfer_program_id,
    )?;
    require_rpc_program(&program_ids, "token", manifest.target.token_program_id)?;
    ensure!(
        !program_ids.contains_key("associated_token_account"),
        "public RPC now exposes ATA: update manifest provenance and tests before deployment"
    );
    let last_block_id = client
        .get_last_block_id()
        .await
        .context("read preflight chain height")?;
    let evidence = PreflightEvidence {
        rpc_url: rpc_url.to_owned(),
        channel_id,
        elf_sha256,
        image_id,
        program_id_words: ZEC_ESCROW_V02_ID,
        authenticated_transfer_program_id: manifest.target.authenticated_transfer_program_id,
        token_program_id: manifest.target.token_program_id,
        associated_token_account_program_id: manifest.target.associated_token_account_program_id,
        associated_token_account_identity_source: manifest
            .target
            .associated_token_account_identity_source
            .clone(),
        rpc_program_names: program_ids.keys().cloned().collect(),
        last_block_id,
    };
    Ok(evidence)
}

fn validate_local_rpc_url(rpc_url: &str) -> Result<()> {
    let parsed = Url::parse(rpc_url).context("parse local LEZ RPC URL")?;
    let loopback = match parsed.host() {
        Some(Host::Ipv4(address)) => IpAddr::V4(address).is_loopback(),
        Some(Host::Ipv6(address)) => IpAddr::V6(address).is_loopback(),
        Some(Host::Domain(_)) | None => false,
    };
    ensure!(
        parsed.scheme() == "http"
            && loopback
            && parsed.port().is_some()
            && parsed.username().is_empty()
            && parsed.password().is_none()
            && parsed.query().is_none()
            && parsed.fragment().is_none()
            && parsed.path() == "/",
        "local LEZ RPC URL must be an explicit uncredentialed loopback HTTP endpoint and port"
    );
    Ok(())
}

fn validate_channel_id(channel_id: &str) -> Result<()> {
    let mut bytes = [0_u8; 32];
    hex::decode_to_slice(channel_id, &mut bytes)
        .context("local LEZ channel ID must be exactly 32 hex bytes")?;
    ensure!(bytes != [0; 32], "local LEZ channel ID must be nonzero");
    Ok(())
}

fn require_rpc_program(
    program_ids: &BTreeMap<String, [u32; 8]>,
    name: &str,
    expected: [u32; 8],
) -> Result<()> {
    let actual = program_ids
        .get(name)
        .with_context(|| format!("official RPC omitted required {name} ProgramId"))?;
    ensure!(
        *actual == expected,
        "official RPC {name} ProgramId differs from the immutable manifest"
    );
    Ok(())
}

async fn deploy_once_and_observe(
    client: SequencerClient,
    preflight: PreflightEvidence,
    timeout_seconds: u64,
) -> Result<DeploymentEvidence> {
    deploy_once_and_observe_with_timeout(client, preflight, Duration::from_secs(timeout_seconds))
        .await
}

async fn deploy_once_and_observe_with_timeout(
    client: SequencerClient,
    preflight: PreflightEvidence,
    observation_timeout: Duration,
) -> Result<DeploymentEvidence> {
    let transaction = ProgramDeploymentTransaction::new(
        nssa::program_deployment_transaction::Message::new(ZEC_ESCROW_V02_ELF.to_vec()),
    );
    let expected_hash = HashType(transaction.hash());
    let submitted_hash = client
        .send_transaction(LeeTransaction::ProgramDeployment(transaction.clone()))
        .await
        .with_context(|| {
            format!(
                "deployment submission outcome is unknown for {}; observe before any retry",
                expected_hash
            )
        })?;
    ensure!(
        submitted_hash == expected_hash,
        "RPC returned a deployment hash different from the locally checked transaction"
    );

    let observe = observe_inclusion(
        &client,
        &transaction,
        submitted_hash,
        preflight.last_block_id,
    );
    let (inclusion_block_id, inclusion_block_hash) =
        tokio::time::timeout(observation_timeout, observe)
            .await
            .with_context(|| {
                format!(
                    "timed out observing submitted deployment {}; do not resubmit",
                    submitted_hash
                )
            })??;
    Ok(DeploymentEvidence {
        preflight,
        transaction_hash: submitted_hash.to_string(),
        inclusion_block_id,
        inclusion_block_hash,
    })
}

async fn observe_inclusion(
    client: &SequencerClient,
    expected_deployment: &ProgramDeploymentTransaction,
    transaction_hash: HashType,
    pre_submission_tip: u64,
) -> Result<(u64, String)> {
    let mut scanned = pre_submission_tip;
    loop {
        let tip = client
            .get_last_block_id()
            .await
            .context("read deployment observation tip")?;
        if tip > scanned {
            let blocks = client
                .get_block_range(scanned + 1, tip)
                .await
                .context("read bounded deployment block range")?;
            for block in blocks {
                ensure!(
                    (scanned + 1..=tip).contains(&block.header.block_id),
                    "RPC returned a deployment block outside the requested canonical range"
                );
                for transaction in &block.body.transactions {
                    if transaction.hash() != transaction_hash {
                        continue;
                    }
                    ensure!(
                        block.header.block_id > pre_submission_tip,
                        "deployment inclusion predates the one submission attempt"
                    );
                    let LeeTransaction::ProgramDeployment(included_deployment) = transaction else {
                        anyhow::bail!("matching canonical transaction must be a ProgramDeployment");
                    };
                    ensure!(
                        included_deployment == expected_deployment,
                        "canonical block deployment bytes differ from the submitted transaction"
                    );
                    let included_program = Program::new(
                        included_deployment
                            .clone()
                            .into_message()
                            .into_bytecode()
                            .into(),
                    )
                    .context("canonical block deployment must contain a valid Risc0 program")?;
                    ensure!(
                        included_program.id() == ZEC_ESCROW_V02_ID,
                        "canonical block deployment ImageID differs from the checked ProgramId"
                    );
                    return Ok((block.header.block_id, block.header.hash.to_string()));
                }
            }
            scanned = tip;
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

#[cfg(test)]
mod tests;
