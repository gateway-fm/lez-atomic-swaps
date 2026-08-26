#![forbid(unsafe_code)]

use std::{
    fs,
    os::unix::fs::{MetadataExt as _, PermissionsExt as _},
    path::{Path, PathBuf},
    str::FromStr as _,
    sync::Arc,
};

use anyhow::{Context as _, Result, bail, ensure};
use clap::{Parser, ValueEnum};
use lez_bridge_protocol::{
    DiscoveryWindow, Hex32, MessageContext, Participant, RequestId, RunId, RuntimeCompatibility,
    RuntimeDescriptor, TransactionId,
};
use lez_v0_2_sidecar::{
    OfficialNodeRpc, PrepareVaultClaimRequest, RuntimeBoundary, VaultClaimAllocation,
    VaultClaimBeforeState, VaultClaimEffectJournal, VaultClaimEffectState, VaultClaimPlanner,
    VaultClaimSubmissionOutcome, VaultClaimSubmissionUncertainty, VaultClaimSubmitter,
};
use nssa::{Account, AccountId, PrivateKey, PublicKey};
use serde::Serialize;
use zeroize::Zeroizing;

const EVIDENCE_SCHEMA: &str = "lez_v02_vault_claim_poc_v1";
const DEFAULT_SCAN_BLOCKS: u32 = 128;

#[derive(Clone, Copy, Debug, ValueEnum)]
enum RoleArgument {
    Maker,
    Taker,
}

impl From<RoleArgument> for Participant {
    fn from(value: RoleArgument) -> Self {
        match value {
            RoleArgument::Maker => Self::Maker,
            RoleArgument::Taker => Self::Taker,
        }
    }
}

/// Submit one owner-authorized Vault Claim to an actual local LEZ v0.2 sequencer.
#[derive(Debug, Parser)]
#[command(version, about)]
struct Arguments {
    /// Isolated swap actor whose key and state this process owns.
    #[arg(long, value_enum)]
    role: RoleArgument,

    /// Bounded run identifier shared by the composed local-devnet run.
    #[arg(long)]
    run_id: String,

    /// Bounded idempotency/correlation identifier for this exact Claim.
    #[arg(long)]
    request_id: String,

    /// Existing owner-only 0700 directory dedicated to this actor.
    #[arg(long)]
    state_directory: PathBuf,

    /// Owner-only file containing one lowercase-hex 32-byte LEZ private key.
    #[arg(long)]
    private_key_file: PathBuf,

    /// Explicit literal-loopback official sequencer HTTP URL and port.
    #[arg(long)]
    sequencer_url: String,

    /// Configured local-devnet chain identity as 64 lowercase hex characters.
    #[arg(long)]
    chain_id: String,

    /// Configured/deployed escrow program ID as 64 lowercase hex characters.
    #[arg(long)]
    escrow_program_id: String,

    /// Exact genesis allocation held by this owner's Vault.
    #[arg(long)]
    allocation: u128,

    /// Bound retained for later inclusion/finality scans.
    #[arg(long, default_value_t = DEFAULT_SCAN_BLOCKS)]
    max_scan_blocks: u32,
}

#[derive(Debug, Serialize)]
#[serde(deny_unknown_fields)]
struct PublicAccountEvidence {
    account_id: Hex32,
    program_owner: Hex32,
    balance: u128,
    nonce: u128,
}

#[derive(Debug, Serialize)]
#[serde(deny_unknown_fields)]
struct NodeStateEvidence {
    sequencer_tip: u64,
    owner: PublicAccountEvidence,
    vault: PublicAccountEvidence,
}

#[derive(Debug, Serialize)]
#[serde(tag = "decision", rename_all = "snake_case")]
enum SubmissionEvidence {
    Admitted,
    Rejected,
    Unknown {
        uncertainty: VaultClaimSubmissionUncertainty,
    },
    ObserveOnly {
        state: VaultClaimEffectState,
        uncertainty: Option<VaultClaimSubmissionUncertainty>,
    },
}

impl SubmissionEvidence {
    const fn proves_admission(&self) -> bool {
        matches!(
            self,
            Self::Admitted
                | Self::ObserveOnly {
                    state: VaultClaimEffectState::Admitted,
                    ..
                }
        )
    }
}

impl From<VaultClaimSubmissionOutcome> for SubmissionEvidence {
    fn from(value: VaultClaimSubmissionOutcome) -> Self {
        match value {
            VaultClaimSubmissionOutcome::Admitted => Self::Admitted,
            VaultClaimSubmissionOutcome::Rejected => Self::Rejected,
            VaultClaimSubmissionOutcome::Unknown(uncertainty) => Self::Unknown { uncertainty },
            VaultClaimSubmissionOutcome::ObserveOnly { state, uncertainty } => {
                Self::ObserveOnly { state, uncertainty }
            }
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(deny_unknown_fields)]
struct RuntimeEvidence {
    compatibility: RuntimeCompatibility,
    chain_id: Hex32,
    chain_id_source: &'static str,
    channel_id: Hex32,
    channel_id_source: &'static str,
    genesis_block_hash: Hex32,
    genesis_block_hash_source: &'static str,
    escrow_program_id: Hex32,
    escrow_program_id_source: &'static str,
}

#[derive(Debug, Serialize)]
#[serde(deny_unknown_fields)]
struct VaultClaimEvidence {
    schema: &'static str,
    role: Participant,
    run_id: RunId,
    request_id: RequestId,
    runtime: RuntimeEvidence,
    allocation: u128,
    transaction_id: TransactionId,
    submission: SubmissionEvidence,
    durable_state: VaultClaimEffectState,
    durable_attempt_count: u32,
    durable_revision: u64,
    before: NodeStateEvidence,
    post: Option<NodeStateEvidence>,
    post_observation: &'static str,
    finality: &'static str,
}

#[tokio::main]
async fn main() {
    match execute(Arguments::parse()).await {
        Ok(evidence) => {
            let admitted = evidence.submission.proves_admission();
            let Ok(json) = serde_json::to_string(&evidence) else {
                eprintln!("failed to encode redacted Vault Claim evidence");
                std::process::exit(1);
            };
            println!("{json}");
            if !admitted {
                std::process::exit(1);
            }
        }
        Err(error) => {
            eprintln!("Vault Claim PoC failed: {error:#}");
            std::process::exit(1);
        }
    }
}

#[allow(
    clippy::too_many_lines,
    reason = "the PoC keeps fact capture, durable attempt-before-call ordering, and evidence assembly in one auditable linear flow"
)]
async fn execute(arguments: Arguments) -> Result<VaultClaimEvidence> {
    validate_state_directory(&arguments.state_directory)?;
    let signer_key = read_private_key(&arguments.private_key_file)?;
    let role = Participant::from(arguments.role);
    let run_id = RunId::new(arguments.run_id).context("invalid run ID")?;
    let request_id = RequestId::new(arguments.request_id).context("invalid request ID")?;
    let chain_id = Hex32::from_hex(&arguments.chain_id).context("invalid chain ID")?;
    let escrow_program_id =
        Hex32::from_hex(&arguments.escrow_program_id).context("invalid escrow program ID")?;
    ensure!(chain_id.as_bytes() != &[0; 32], "chain ID must be nonzero");
    ensure!(
        escrow_program_id.as_bytes() != &[0; 32],
        "escrow program ID must be nonzero"
    );

    let owner_account_id = AccountId::from(&PublicKey::new_from_private_key(&signer_key));
    let vault_account_id =
        vault_core::compute_vault_account_id(programs::vault().id(), owner_account_id);
    let node = Arc::new(
        OfficialNodeRpc::connect(&arguments.sequencer_url)
            .context("invalid local sequencer endpoint")?,
    );
    let facts = node
        .vault_claim_facts(owner_account_id, vault_account_id)
        .await
        .context("could not read one-tip official Vault Claim facts")?;
    let descriptor = RuntimeDescriptor::new(
        role,
        RuntimeCompatibility::LeeV0_2_0,
        chain_id,
        Hex32::from_bytes(facts.channel_id()),
        Hex32::from_bytes(facts.genesis_block_hash()),
        escrow_program_id,
        Hex32::from_bytes(owner_account_id.into_value()),
    );
    RuntimeBoundary::new(descriptor.clone(), role, owner_account_id, node.clone())?
        .verify_health()
        .await
        .context("official sequencer health/channel verification failed")?;

    let allocation = VaultClaimAllocation::new(
        role,
        Hex32::from_bytes(owner_account_id.into_value()),
        arguments.allocation,
    )?;
    let owner_nonce = u128::from(facts.owner_account().nonce);
    let context = MessageContext::new(run_id.clone(), request_id.clone(), role);
    let request =
        PrepareVaultClaimRequest::new(context, descriptor.clone(), allocation.clone(), owner_nonce);
    let planner = VaultClaimPlanner::new_durable(
        role,
        signer_key,
        descriptor.clone(),
        allocation,
        node.clone(),
        &arguments.state_directory,
    )?;
    let result = planner.prepare(request.clone()).await?;
    let before_state = VaultClaimBeforeState::new(
        owner_account_id,
        facts.owner_account().clone(),
        vault_account_id,
        facts.vault_account().clone(),
        facts.sequencer_tip(),
        None,
    )?;
    let sequencer_start = facts
        .sequencer_tip()
        .checked_add(1)
        .context("sequencer tip cannot advance")?;
    let effect = lez_v0_2_sidecar::PreparedVaultClaimEffect::new(
        &planner,
        request,
        result.clone(),
        before_state,
        DiscoveryWindow::new(sequencer_start, arguments.max_scan_blocks)?,
        DiscoveryWindow::new(nssa::GENESIS_BLOCK_ID, arguments.max_scan_blocks)?,
    )?;
    let actor_binding = effect.actor_binding().clone();
    let effect_identity = effect.identity().clone();
    let journal = VaultClaimEffectJournal::open(&arguments.state_directory, actor_binding.clone())?;
    journal.record_prepared(&effect)?;
    let submission = VaultClaimSubmitter::new(journal, &planner)?
        .submit_or_observe(&effect_identity, node.as_ref())
        .await?;

    let journal = VaultClaimEffectJournal::open(&arguments.state_directory, actor_binding)?;
    let durable = journal
        .load(&effect_identity)?
        .context("submitted effect is absent from its durable journal")?;
    let post_facts = node
        .vault_claim_facts(owner_account_id, vault_account_id)
        .await
        .ok()
        .filter(|post| {
            post.channel_id() == facts.channel_id()
                && post.genesis_block_hash() == facts.genesis_block_hash()
        });
    let (post, post_observation) =
        post_facts
            .as_ref()
            .map_or((None, "unavailable_or_non_atomic"), |post| {
                (
                    Some(node_state_evidence(
                        owner_account_id,
                        vault_account_id,
                        post.owner_account(),
                        post.vault_account(),
                        post.sequencer_tip(),
                    )),
                    "same_runtime_same_tip",
                )
            });

    Ok(VaultClaimEvidence {
        schema: EVIDENCE_SCHEMA,
        role,
        run_id,
        request_id,
        runtime: RuntimeEvidence {
            compatibility: RuntimeCompatibility::LeeV0_2_0,
            chain_id,
            chain_id_source: "explicit_configuration_no_v02_rpc",
            channel_id: Hex32::from_bytes(facts.channel_id()),
            channel_id_source: "sequencer_getChannelId",
            genesis_block_hash: Hex32::from_bytes(facts.genesis_block_hash()),
            genesis_block_hash_source: "sequencer_getBlock_genesis",
            escrow_program_id,
            escrow_program_id_source: "explicit_configuration",
        },
        allocation: arguments.allocation,
        transaction_id: result.claim.transaction_id,
        submission: submission.into(),
        durable_state: durable.state(),
        durable_attempt_count: durable.attempt_count(),
        durable_revision: durable.revision(),
        before: node_state_evidence(
            owner_account_id,
            vault_account_id,
            facts.owner_account(),
            facts.vault_account(),
            facts.sequencer_tip(),
        ),
        post,
        post_observation,
        finality: "not_observed_in_this_poc_slice",
    })
}

fn node_state_evidence(
    owner_account_id: AccountId,
    vault_account_id: AccountId,
    owner: &Account,
    vault: &Account,
    sequencer_tip: u64,
) -> NodeStateEvidence {
    NodeStateEvidence {
        sequencer_tip,
        owner: account_evidence(owner_account_id, owner),
        vault: account_evidence(vault_account_id, vault),
    }
}

fn account_evidence(account_id: AccountId, account: &Account) -> PublicAccountEvidence {
    PublicAccountEvidence {
        account_id: Hex32::from_bytes(account_id.into_value()),
        program_owner: program_id_hex(account.program_owner),
        balance: account.balance,
        nonce: u128::from(account.nonce),
    }
}

fn program_id_hex(program_id: [u32; 8]) -> Hex32 {
    let mut bytes = [0_u8; 32];
    for (chunk, word) in bytes.chunks_exact_mut(4).zip(program_id) {
        chunk.copy_from_slice(&word.to_le_bytes());
    }
    Hex32::from_bytes(bytes)
}

fn validate_state_directory(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path).context("state directory is unavailable")?;
    ensure!(
        metadata.is_dir(),
        "state directory must be a real directory"
    );
    ensure!(
        metadata.permissions().mode() & 0o777 == 0o700,
        "state directory must already have mode 0700"
    );
    Ok(())
}

fn read_private_key(path: &Path) -> Result<PrivateKey> {
    let metadata = fs::symlink_metadata(path).context("private key file is unavailable")?;
    ensure!(metadata.is_file(), "private key must be a regular file");
    ensure!(
        metadata.permissions().mode().trailing_zeros() >= 6,
        "private key file must not be accessible by group or others"
    );
    ensure!(metadata.nlink() == 1, "private key file must have one link");
    let encoded = Zeroizing::new(fs::read(path).context("private key file could not be read")?);
    ensure!(encoded.len() <= 128, "private key file is too large");
    let encoded = std::str::from_utf8(encoded.as_slice())
        .context("private key file must be UTF-8")?
        .trim();
    if encoded.len() != 64 {
        bail!("private key file must contain exactly 64 hex characters");
    }
    PrivateKey::from_str(encoded).context("private key file contains an invalid key")
}
