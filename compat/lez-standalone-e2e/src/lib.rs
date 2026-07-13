//! Reusable exact-LEZ-v0.1.2 standalone node helpers.

#![forbid(unsafe_code)]

use std::{
    io::Write as _,
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Context as _, Result, ensure};
use bytesize::ByteSize;
use common::{HashType, transaction::NSSATransaction};
use nssa::{ProgramDeploymentTransaction, program::Program};
use sequencer_service::{BedrockConfig, SequencerConfig, SequencerHandle};
use sequencer_service_rpc::{RpcClient as _, SequencerClient};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

/// Deterministic channel identity used only by the isolated compatibility node.
pub const LOCAL_CHANNEL_ID: [u8; 32] = [0; 32];
/// First canonical block ID produced by the pinned standalone sequencer.
pub const GENESIS_BLOCK_ID: u64 = 1;
/// Maximum time to wait for canonical standalone progress.
pub const NODE_OPERATION_TIMEOUT: Duration = Duration::from_secs(60);
/// Exact repository-tracked guest artifact manifest embedded at build time.
pub const TRACKED_GUEST_ARTIFACT_MANIFEST: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../spel-zec-escrow/methods/guest/artifact-manifest.toml"
));

/// Secret-bearing deterministic actor material for one local-only process.
///
/// This type deliberately does not implement `Debug` or `Display`.
#[derive(Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LocalActorManifest {
    /// Canonical base58 account identity.
    pub account_id: String,
    /// Exact deterministic signing key encoded as lowercase hexadecimal.
    pub private_key: String,
    /// Genesis-funded native balance confirmed through official RPC.
    pub balance: u128,
}

/// Private readiness handoff for one isolated checked-guest node process.
///
/// This type deliberately does not implement `Debug` or `Display` because it
/// contains local deterministic actor keys.
#[derive(Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LocalNodeReadinessManifest {
    /// Stable manifest schema.
    pub schema_version: u16,
    /// Dynamic loopback HTTP RPC endpoint.
    pub endpoint: String,
    /// Exact upstream genesis block ID.
    pub genesis_block_id: u64,
    /// Canonical genesis block hash encoded as lowercase hexadecimal.
    pub genesis_block_hash: String,
    /// Exact local Bedrock channel ID encoded as lowercase hexadecimal.
    pub channel_id: String,
    /// SHA-256 of the deployed ELF, matched to the tracked artifact manifest.
    pub elf_sha256: String,
    /// Risc0 ImageID, matched to the tracked artifact manifest.
    pub image_id: String,
    /// Checked escrow guest program identity.
    pub escrow_program_id: [u32; 8],
    /// Static built-in authenticated-transfer identity used by funded actors.
    pub authenticated_transfer_program_id: [u32; 8],
    /// Canonical checked-guest deployment transaction hash.
    pub deployment_transaction_hash: String,
    /// Canonical block containing the checked-guest deployment.
    pub deployment_block_id: u64,
    /// Canonical hash of the block containing the checked-guest deployment.
    pub deployment_block_hash: String,
    /// Verified funded deterministic genesis actors and their signing material.
    pub actors: Vec<LocalActorManifest>,
}

/// Canonical deployment evidence returned by the reusable helper.
pub struct CheckedGuestDeployment {
    /// Exact upstream program derived from the deployed ELF.
    pub program: Program,
    /// Canonical deployment transaction hash encoded as lowercase hexadecimal.
    pub transaction_hash: String,
    /// Canonical block ID containing the deployment.
    pub inclusion_block_id: u64,
    /// Canonical containing-block hash encoded as lowercase hexadecimal.
    pub inclusion_block_hash: String,
}

/// Public identity of one ELF verified against the tracked artifact manifest.
#[derive(Clone, Eq, PartialEq)]
pub struct CheckedGuestIdentity {
    /// Exact lowercase SHA-256 of the ELF bytes.
    pub elf_sha256: String,
    /// Exact lowercase Risc0 ImageID computed by the pinned LEZ program parser.
    pub image_id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GuestArtifactManifest {
    format_version: u16,
    program: String,
    spel_commit: String,
    lez_commit: String,
    risc0_version: String,
    risc0_guest_builder: String,
    elf_sha256: String,
    image_id: String,
}

/// Verifies ELF bytes against the repository's tracked artifact manifest.
///
/// SHA-256 is checked independently from the Risc0 ImageID. The latter is
/// computed by the same pinned upstream [`Program`] parser used for deployment,
/// avoiding a second implementation of Risc0 ELF identity semantics.
///
/// # Errors
///
/// Rejects unreadable or malformed manifests, unsupported/empty manifest
/// identity fields, digest mismatch, ImageID mismatch, or invalid guest bytes.
pub fn verify_checked_guest_artifact(
    elf: &[u8],
    artifact_manifest_path: &Path,
) -> Result<CheckedGuestIdentity> {
    let source = std::fs::read_to_string(artifact_manifest_path).with_context(|| {
        format!(
            "read checked-guest artifact manifest at {}",
            artifact_manifest_path.display()
        )
    })?;
    ensure!(
        source == TRACKED_GUEST_ARTIFACT_MANIFEST,
        "artifact manifest does not equal the repository-tracked build identity"
    );
    let manifest: GuestArtifactManifest =
        toml::from_str(&source).context("decode checked-guest artifact manifest")?;
    ensure!(
        manifest.format_version == 1,
        "unsupported artifact manifest"
    );
    ensure!(manifest.program == "zec_escrow", "unexpected guest program");
    ensure!(
        !manifest.spel_commit.is_empty()
            && !manifest.lez_commit.is_empty()
            && !manifest.risc0_version.is_empty()
            && !manifest.risc0_guest_builder.is_empty(),
        "artifact build identity must be complete"
    );
    ensure_canonical_digest("ELF SHA-256", &manifest.elf_sha256)?;
    ensure_canonical_digest("Risc0 ImageID", &manifest.image_id)?;

    let elf_sha256 = hex::encode(Sha256::digest(elf));
    ensure!(
        elf_sha256 == manifest.elf_sha256,
        "guest ELF SHA-256 does not match the tracked artifact manifest"
    );
    let program = Program::new(elf.to_vec()).context("guest must be a canonical LEZ program")?;
    let image_id = risc0_zkvm::Digest::from(program.id()).to_string();
    ensure!(
        image_id == manifest.image_id,
        "guest ImageID does not match the tracked artifact manifest"
    );
    Ok(CheckedGuestIdentity {
        elf_sha256,
        image_id,
    })
}

fn ensure_canonical_digest(name: &str, digest: &str) -> Result<()> {
    ensure!(
        digest.len() == 64
            && digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "{name} must be exactly 32 lowercase hexadecimal bytes"
    );
    Ok(())
}

/// Atomically creates a private readiness manifest with Unix mode `0600`.
///
/// The destination must not already exist. A randomly named same-directory
/// private temporary file is fully flushed and persisted atomically without
/// clobbering an existing path.
///
/// # Errors
///
/// Fails on a pre-existing destination, serialization, permission, flush, or
/// filesystem operation error.
pub fn write_private_readiness_manifest(
    path: &Path,
    manifest: &LocalNodeReadinessManifest,
) -> Result<()> {
    let file_name = path
        .file_name()
        .context("readiness manifest requires a file name")?
        .to_string_lossy();
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let temporary_prefix = format!(".{file_name}.");
    let mut builder = tempfile::Builder::new();
    builder.prefix(&temporary_prefix);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        builder.permissions(std::fs::Permissions::from_mode(0o600));
    }
    let mut temporary = builder
        .tempfile_in(parent)
        .with_context(|| format!("create private readiness temporary at {}", parent.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        temporary
            .as_file()
            .set_permissions(std::fs::Permissions::from_mode(0o600))
            .context("set readiness temporary mode 0600")?;
    }
    serde_json::to_writer(temporary.as_file_mut(), manifest)
        .context("serialize readiness manifest")?;
    temporary
        .write_all(b"\n")
        .context("terminate readiness manifest")?;
    temporary
        .as_file()
        .sync_all()
        .context("flush readiness manifest")?;
    temporary
        .persist_noclobber(path)
        .map_err(|error| error.error)
        .with_context(|| format!("publish private readiness manifest at {}", path.display()))?;
    Ok(())
}

/// Builds the exact isolated v0.1.2 standalone configuration.
///
/// The upstream sequencer remains responsible for genesis/state construction;
/// this helper only selects deterministic testnet genesis inputs and inert
/// loopback placeholders for integrations unused by standalone mode.
#[must_use]
pub fn isolated_config(home: PathBuf) -> SequencerConfig {
    SequencerConfig {
        home,
        genesis_id: GENESIS_BLOCK_ID,
        is_genesis_random: false,
        max_num_tx_in_block: 20,
        max_block_size: ByteSize::mib(4),
        mempool_max_size: 1_000,
        block_create_timeout: Duration::from_millis(250),
        retry_pending_blocks_timeout: Duration::from_millis(100),
        signing_key: [37; 32],
        bedrock_config: BedrockConfig {
            backoff: Default::default(),
            channel_id: LOCAL_CHANNEL_ID.into(),
            node_url: "http://127.0.0.1:1".parse().expect("static URL"),
            auth: None,
        },
        indexer_rpc_url: "ws://127.0.0.1:1".parse().expect("static URL"),
        initial_public_accounts: Some(testnet_initial_state::initial_accounts()),
        initial_private_accounts: Some(testnet_initial_state::initial_commitments()),
    }
}

/// Waits until the exact upstream sequencer advances beyond `before`.
///
/// # Errors
///
/// Fails if the sequencer stops, RPC fails, or the bounded wait expires.
pub async fn wait_for_chain_advance(
    client: &SequencerClient,
    handle: &SequencerHandle,
    before: u64,
) -> Result<u64> {
    tokio::time::timeout(NODE_OPERATION_TIMEOUT, async {
        loop {
            ensure!(handle.is_healthy(), "standalone sequencer stopped");
            let current = client
                .get_last_block_id()
                .await
                .context("poll canonical block id")?;
            if current > before {
                break Ok::<_, anyhow::Error>(current);
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .context("canonical chain did not advance before timeout")?
}

/// Deploys one checked guest through the exact upstream public RPC path.
///
/// Returns the upstream [`Program`], exact deployment hash, and containing
/// canonical block identity only after the same ELF is present in the block
/// store.
///
/// # Errors
///
/// Rejects an invalid/empty guest, unhealthy node, noncanonical returned hash,
/// missing inclusion, wrong included transaction kind, or no chain advance.
pub async fn deploy_checked_guest(
    client: &SequencerClient,
    handle: &SequencerHandle,
    elf: Vec<u8>,
) -> Result<CheckedGuestDeployment> {
    ensure!(!elf.is_empty(), "guest ELF must not be empty");
    let program = Program::new(elf.clone()).context("guest must be a canonical LEZ program")?;
    let before = client
        .get_last_block_id()
        .await
        .context("pre-deployment block id")?;
    let message = nssa::program_deployment_transaction::Message::new(elf);
    let deployment = ProgramDeploymentTransaction::new(message);
    let expected_hash = HashType(deployment.hash());
    let submitted_hash = client
        .send_transaction(NSSATransaction::ProgramDeployment(deployment.clone()))
        .await
        .context("submit deployment through sendTransaction")?;
    ensure!(
        submitted_hash == expected_hash,
        "deployment hash must be canonical"
    );

    tokio::time::timeout(NODE_OPERATION_TIMEOUT, async {
        loop {
            ensure!(
                handle.is_healthy(),
                "standalone sequencer stopped while deployment was pending"
            );
            if let Some(included) = client
                .get_transaction(expected_hash)
                .await
                .context("poll canonical deployment")?
            {
                ensure!(
                    included.hash() == expected_hash,
                    "included hash must be exact"
                );
                let NSSATransaction::ProgramDeployment(included) = included else {
                    anyhow::bail!("included transaction must be the deployment");
                };
                ensure!(
                    included == deployment,
                    "included deployment bytes must be exact"
                );
                let included_program = Program::new(included.into_message().into_bytecode())
                    .context("included deployment must contain a canonical program")?;
                ensure!(
                    included_program.id() == program.id(),
                    "included deployment program identity must be exact"
                );
                break Ok::<_, anyhow::Error>(());
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .context("guest deployment was not included before timeout")??;
    let after = client
        .get_last_block_id()
        .await
        .context("included block id")?;
    ensure!(
        after > before,
        "deployment must advance the canonical block chain"
    );

    let mut inclusion = None;
    for block_id in before.saturating_add(1)..=after {
        let block = client
            .get_block(block_id)
            .await
            .with_context(|| format!("query candidate deployment block {block_id}"))?
            .with_context(|| format!("candidate deployment block {block_id} must exist"))?;
        for transaction in &block.body.transactions {
            if transaction.hash() != expected_hash {
                continue;
            }
            ensure!(
                inclusion.is_none(),
                "deployment transaction must occur in exactly one canonical block"
            );
            let NSSATransaction::ProgramDeployment(included) = transaction else {
                anyhow::bail!("matching canonical transaction must be a deployment");
            };
            ensure!(
                included == &deployment,
                "canonical block deployment bytes must be exact"
            );
            let included_program = Program::new(included.clone().into_message().into_bytecode())
                .context("canonical block deployment must contain a valid program")?;
            ensure!(
                included_program.id() == program.id(),
                "canonical block deployment program identity must be exact"
            );
            inclusion = Some((block.header.block_id, block.header.hash.to_string()));
        }
    }
    let (inclusion_block_id, inclusion_block_hash) =
        inclusion.context("deployment transaction must be located in a canonical block")?;
    Ok(CheckedGuestDeployment {
        program,
        transaction_hash: expected_hash.to_string(),
        inclusion_block_id,
        inclusion_block_hash,
    })
}
