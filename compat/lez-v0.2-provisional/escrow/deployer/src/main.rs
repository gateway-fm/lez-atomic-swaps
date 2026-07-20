#[cfg(unix)]
use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::{Read as _, Write as _},
    net::IpAddr,
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Context as _, Result, ensure};
use clap::{Parser, Subcommand};
use common::{HashType, transaction::LeeTransaction};
use hmac::{Hmac, Mac as _};
use lez_zec_escrow_v02::{Instruction as EscrowInstruction, PROGRAM_IDL_JSON};
use lez_zec_escrow_v02_methods::{ZEC_ESCROW_V02_ELF, ZEC_ESCROW_V02_ID};
use nssa::{AccountId, ProgramDeploymentTransaction, program::Program};
use nssa_core::program::PdaSeed;
use sequencer_service_rpc::{RpcClient as _, SequencerClient, SequencerClientBuilder};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use tempfile::NamedTempFile;
use url::{Host, Url};
use zeroize::Zeroizing;

const MANIFEST: &str = include_str!("../../methods/guest/deployment-manifest.toml");
const M4_MANIFEST: &str = include_str!("../../methods/guest/m4-deployment-manifest.toml");
const M4_CHECKED_ELF_SHA256: &str =
    "dc370bc34b432317730c51b49342760dbc675fca700e300b30b5fadefe5b7292";
const M4_CHECKED_PROGRAM_ID: &str =
    "4d6590332948743c2db88a183755815354ef92560550cd206ac27bddeea12c82";
const OFFICIAL_RPC_URL: &str = "https://testnet.lez.logos.co";
const OFFICIAL_CHANNEL_ID: &str =
    "0101010101010101010101010101010101010101010101010101010101010101";
const DEPLOYMENT_EVIDENCE_SCHEMA_VERSION: u16 = 1;
const AUTHENTICATED_EVIDENCE_SCHEMA_VERSION: u16 = 1;
const PROVISIONED_IDENTITY_SCHEMA_VERSION: u16 = 1;
const PUBLIC_INSTRUCTION_ASSEMBLY_SCHEMA_VERSION: u16 = 1;
const MAX_DEPLOYMENT_EVIDENCE_BYTES: usize = 64 * 1024;
const EVIDENCE_AUTHENTICATION_KEY_BYTES: usize = 32;
const EVIDENCE_AUTHENTICATION_ALGORITHM: &str = "hmac-sha256-v1";
const EVIDENCE_AUTHENTICATION_DOMAIN: &[u8] =
    b"lez-atomic-swaps/deployment-evidence/hmac-sha256-v1\0";
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
    /// Print the artifact-bound public SpEL IDL without RPC access or mutation.
    Interface,
    /// Assemble exact public words/accounts for witnessed custom-token initialization.
    AssembleInitializeTokenWitnessed {
        #[arg(long)]
        metadata_account_id: String,
        #[arg(long)]
        depositor_owner_account_id: String,
        #[arg(long)]
        claimant_owner_account_id: String,
        #[arg(long)]
        token_definition_account_id: String,
        #[arg(long)]
        aggregate_authority_account_id: String,
        /// Exactly 32 lowercase or uppercase hex bytes.
        #[arg(long)]
        swap_id: String,
        /// Exactly 32 nonzero hex bytes.
        #[arg(long)]
        terms_hash: String,
        /// Exact aggregate BIP-340 x-only key as 32 hex bytes.
        #[arg(long)]
        aggregate_x_only_public_key: String,
        #[arg(long)]
        amount: u128,
        #[arg(long)]
        refund_at: u64,
    },
    /// Assemble exact public words/accounts for a witnessed custom-token claim.
    AssembleClaimTokenWitnessed {
        #[arg(long)]
        metadata_account_id: String,
        #[arg(long)]
        custody_account_id: String,
        #[arg(long)]
        claimant_owner_account_id: String,
        #[arg(long)]
        claimant_asset_account_id: String,
        /// Definition is validation-only and is not an instruction account.
        #[arg(long)]
        token_definition_account_id: String,
        #[arg(long)]
        aggregate_authority_account_id: String,
        /// Exactly 32 lowercase or uppercase hex bytes.
        #[arg(long)]
        swap_id: String,
    },
    /// Submit the checked deployment exactly once and observe canonical inclusion.
    Deploy {
        /// Maximum wall time for observation after the one submission attempt.
        #[arg(long, default_value_t = 300)]
        timeout_seconds: u64,
        /// Owner-only 32-byte key that authenticates retained dynamic chain evidence.
        #[arg(long)]
        evidence_authentication_key_file: PathBuf,
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
    /// Submit the exact checked M4 guest to one explicit isolated local sequencer.
    DeployM4Local {
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
    /// Verify retained deployment evidence offline and write an exact no-clobber runtime identity.
    ProvisionIdentity {
        /// Bounded authenticated JSON emitted by this exact deployer after canonical inclusion.
        #[arg(long)]
        evidence_file: PathBuf,
        /// The same owner-only 32-byte key used by the authorized deployment command.
        #[arg(long)]
        evidence_authentication_key_file: PathBuf,
        /// New file that receives the verified public runtime identity.
        #[arg(long)]
        output_file: PathBuf,
    },
}

#[derive(Debug, Deserialize)]
struct DeploymentManifest {
    artifact_status: String,
    target: Target,
    artifact: Artifact,
    interface: InterfaceArtifact,
}

#[derive(Clone, Debug, Deserialize)]
struct M4DeploymentManifest {
    format_version: u16,
    milestone: String,
    program: String,
    artifact_status: String,
    public_deployment: bool,
    artifact: M4Artifact,
    interface: M4Interface,
}

#[derive(Clone, Debug, Deserialize)]
struct M4Artifact {
    elf_sha256: String,
    image_id: String,
}

#[derive(Clone, Debug, Deserialize)]
struct M4Interface {
    instruction_count: usize,
    initialize_native_xmr_variant: u32,
    authorize_native_xmr_claim_variant: u32,
    claim_native_xmr_variant: u32,
    refund_native_xmr_variant: u32,
    punish_native_xmr_variant: u32,
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

#[derive(Debug, Deserialize)]
struct InterfaceArtifact {
    idl_sha256: String,
    instruction_count: usize,
    initialize_token_witnessed_variant: u32,
    claim_token_witnessed_variant: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PublicInterfaceSummary {
    idl_sha256: String,
    instruction_count: usize,
    initialize_token_witnessed_variant: u32,
    claim_token_witnessed_variant: u32,
}

#[derive(Clone, Debug)]
struct InitializeTokenWitnessedAssembly {
    metadata: AccountId,
    depositor_owner: AccountId,
    claimant_owner: AccountId,
    token_definition: AccountId,
    aggregate_authority: AccountId,
    swap_id: [u8; 32],
    terms_hash: [u8; 32],
    aggregate_x_only_public_key: [u8; 32],
    amount: u128,
    refund_at: u64,
}

#[derive(Clone, Debug)]
struct ClaimTokenWitnessedAssembly {
    metadata: AccountId,
    custody: AccountId,
    claimant_owner: AccountId,
    claimant_asset: AccountId,
    token_definition: AccountId,
    aggregate_authority: AccountId,
    swap_id: [u8; 32],
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct PublicInstructionAccount {
    name: String,
    account_id: String,
    writable: bool,
    signer: bool,
    init: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct PublicInstructionAssembly {
    schema_version: u16,
    program_id_words: [u32; 8],
    program_id_hex: String,
    elf_sha256: String,
    image_id: String,
    idl_sha256: String,
    instruction_name: String,
    variant_index: u32,
    accounts: Vec<PublicInstructionAccount>,
    instruction_data_words: Vec<u32>,
}

#[derive(Clone, Debug)]
struct TrustedExpectedTarget {
    rpc_url: String,
    channel_id: String,
    elf_sha256: String,
    image_id: String,
    program_id_words: [u32; 8],
    authenticated_transfer_program_id: [u32; 8],
    token_program_id: [u32; 8],
    associated_token_account_program_id: [u32; 8],
    associated_token_account_identity_source: String,
    deployment_transaction_hash: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct PreflightEvidence {
    rpc_url: String,
    channel_id: String,
    genesis_block_id: u64,
    genesis_block_hash: String,
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

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct DeploymentEvidence {
    schema_version: u16,
    preflight: PreflightEvidence,
    transaction_hash: String,
    inclusion_block_id: u64,
    inclusion_block_hash: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct AuthenticatedDeploymentEvidence {
    schema_version: u16,
    algorithm: String,
    evidence: DeploymentEvidence,
    authentication_tag: String,
}

#[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct ProvisionedRuntimeIdentity {
    schema_version: u16,
    status: String,
    environment: String,
    compatibility: String,
    rpc_url: String,
    chain_id: String,
    channel_id: String,
    genesis_block_id: u64,
    genesis_block_hash: String,
    escrow_program_id_words: [u32; 8],
    escrow_program_id_hex: String,
    elf_sha256: String,
    image_id: String,
    deployment_evidence_sha256: String,
    deployment_transaction_hash: String,
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
        Command::Interface => {
            checked_public_interface(&manifest)?;
            println!("{PROGRAM_IDL_JSON}");
        }
        Command::AssembleInitializeTokenWitnessed {
            metadata_account_id,
            depositor_owner_account_id,
            claimant_owner_account_id,
            token_definition_account_id,
            aggregate_authority_account_id,
            swap_id,
            terms_hash,
            aggregate_x_only_public_key,
            amount,
            refund_at,
        } => {
            let assembly = assemble_initialize_token_witnessed(
                &manifest,
                InitializeTokenWitnessedAssembly {
                    metadata: parse_account_id(&metadata_account_id, "metadata account ID")?,
                    depositor_owner: parse_account_id(
                        &depositor_owner_account_id,
                        "depositor owner account ID",
                    )?,
                    claimant_owner: parse_account_id(
                        &claimant_owner_account_id,
                        "claimant owner account ID",
                    )?,
                    token_definition: parse_account_id(
                        &token_definition_account_id,
                        "token definition account ID",
                    )?,
                    aggregate_authority: parse_account_id(
                        &aggregate_authority_account_id,
                        "aggregate authority account ID",
                    )?,
                    swap_id: parse_hex_32(&swap_id, "swap ID")?,
                    terms_hash: parse_hex_32(&terms_hash, "terms hash")?,
                    aggregate_x_only_public_key: parse_hex_32(
                        &aggregate_x_only_public_key,
                        "aggregate x-only public key",
                    )?,
                    amount,
                    refund_at,
                },
            )?;
            println!("{}", serde_json::to_string_pretty(&assembly)?);
        }
        Command::AssembleClaimTokenWitnessed {
            metadata_account_id,
            custody_account_id,
            claimant_owner_account_id,
            claimant_asset_account_id,
            token_definition_account_id,
            aggregate_authority_account_id,
            swap_id,
        } => {
            let assembly = assemble_claim_token_witnessed(
                &manifest,
                ClaimTokenWitnessedAssembly {
                    metadata: parse_account_id(&metadata_account_id, "metadata account ID")?,
                    custody: parse_account_id(&custody_account_id, "custody account ID")?,
                    claimant_owner: parse_account_id(
                        &claimant_owner_account_id,
                        "claimant owner account ID",
                    )?,
                    claimant_asset: parse_account_id(
                        &claimant_asset_account_id,
                        "claimant asset account ID",
                    )?,
                    token_definition: parse_account_id(
                        &token_definition_account_id,
                        "token definition account ID",
                    )?,
                    aggregate_authority: parse_account_id(
                        &aggregate_authority_account_id,
                        "aggregate authority account ID",
                    )?,
                    swap_id: parse_hex_32(&swap_id, "swap ID")?,
                },
            )?;
            println!("{}", serde_json::to_string_pretty(&assembly)?);
        }
        Command::Deploy {
            timeout_seconds,
            evidence_authentication_key_file,
        } => {
            ensure!(timeout_seconds > 0, "timeout-seconds must be non-zero");
            let authentication_key =
                read_evidence_authentication_key(&evidence_authentication_key_file)?;
            let (client, preflight) = preflight(&manifest).await?;
            let evidence = deploy_once_and_observe(client, preflight, timeout_seconds).await?;
            let authenticated = authenticate_deployment_evidence(&evidence, &authentication_key)?;
            println!("{}", serde_json::to_string_pretty(&authenticated)?);
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
        Command::DeployM4Local {
            rpc_url,
            channel_id,
            timeout_seconds,
        } => {
            ensure!(timeout_seconds > 0, "timeout-seconds must be non-zero");
            validate_local_rpc_url(&rpc_url)?;
            validate_channel_id(&channel_id)?;
            let m4_manifest: M4DeploymentManifest =
                toml::from_str(M4_MANIFEST).context("parse checked M4 deployment manifest")?;
            let client = bounded_client(&rpc_url)?;
            let preflight = preflight_m4_local_with_client(
                &manifest,
                &m4_manifest,
                &client,
                &rpc_url,
                &channel_id,
            )
            .await?;
            let evidence = deploy_once_and_observe(client, preflight, timeout_seconds).await?;
            println!("{}", serde_json::to_string_pretty(&evidence)?);
        }
        Command::ProvisionIdentity {
            evidence_file,
            evidence_authentication_key_file,
            output_file,
        } => provision_runtime_identity(
            &manifest,
            &evidence_file,
            &evidence_authentication_key_file,
            &output_file,
        )?,
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

fn parse_account_id(value: &str, name: &str) -> Result<AccountId> {
    value
        .parse()
        .with_context(|| format!("parse canonical base58 {name}"))
}

fn parse_hex_32(value: &str, name: &str) -> Result<[u8; 32]> {
    let mut bytes = [0_u8; 32];
    hex::decode_to_slice(value, &mut bytes)
        .with_context(|| format!("{name} must be exactly 32 hex bytes"))?;
    Ok(bytes)
}

fn generated_public_interface(manifest: &DeploymentManifest) -> Result<PublicInterfaceSummary> {
    let idl: serde_json::Value =
        serde_json::from_str(PROGRAM_IDL_JSON).context("parse generated public SpEL IDL")?;
    let instructions = idl
        .get("instructions")
        .and_then(serde_json::Value::as_array)
        .context("generated public SpEL IDL omitted instructions")?;
    let expected_names = [
        "initialize_native",
        "initialize_native_witnessed",
        "fund_native",
        "claim_native",
        "claim_native_witnessed",
        "refund_native",
        "initialize_token",
        "create_token_custody",
        "fund_token",
        "claim_token",
        "refund_token",
        "initialize_token_witnessed",
        "claim_token_witnessed",
    ];
    ensure!(
        instructions.len() == expected_names.len()
            && instructions
                .iter()
                .zip(expected_names)
                .all(|(instruction, expected)| {
                    instruction.get("name").and_then(serde_json::Value::as_str) == Some(expected)
                }),
        "generated public SpEL IDL changed existing tags or witnessed-token append order"
    );
    let idl_sha256 = hex::encode(Sha256::digest(PROGRAM_IDL_JSON.as_bytes()));
    validate_nonzero_lower_hex(&idl_sha256, "public IDL SHA-256")?;
    ensure!(
        manifest.interface.idl_sha256 == idl_sha256,
        "immutable manifest public IDL SHA-256 differs from generated interface: generated {idl_sha256}"
    );
    ensure!(
        manifest.interface.instruction_count == instructions.len()
            && manifest.interface.initialize_token_witnessed_variant == 11
            && manifest.interface.claim_token_witnessed_variant == 12,
        "immutable manifest public instruction tags differ from generated append-only interface"
    );
    Ok(PublicInterfaceSummary {
        idl_sha256,
        instruction_count: instructions.len(),
        initialize_token_witnessed_variant: 11,
        claim_token_witnessed_variant: 12,
    })
}

fn checked_public_interface(manifest: &DeploymentManifest) -> Result<PublicInterfaceSummary> {
    validate_immutable_target(manifest)?;
    generated_public_interface(manifest)
}

fn public_interface_instruction<'a>(
    idl: &'a serde_json::Value,
    name: &str,
) -> Result<(u32, &'a serde_json::Value)> {
    let instructions = idl
        .get("instructions")
        .and_then(serde_json::Value::as_array)
        .context("generated public SpEL IDL omitted instructions")?;
    let (variant, instruction) = instructions
        .iter()
        .enumerate()
        .find(|(_, instruction)| {
            instruction.get("name").and_then(serde_json::Value::as_str) == Some(name)
        })
        .with_context(|| format!("generated public SpEL IDL omitted {name}"))?;
    Ok((
        u32::try_from(variant).context("public instruction variant exceeds u32")?,
        instruction,
    ))
}

fn assemble_checked_public_instruction<T: Serialize>(
    manifest: &DeploymentManifest,
    instruction_name: &str,
    accounts: &[(&str, AccountId)],
    instruction: T,
) -> Result<PublicInstructionAssembly> {
    let (elf_sha256, image_id) = validate_immutable_target(manifest)?;
    let interface = generated_public_interface(manifest)?;
    let idl: serde_json::Value =
        serde_json::from_str(PROGRAM_IDL_JSON).context("parse generated public SpEL IDL")?;
    let (variant_index, idl_instruction) = public_interface_instruction(&idl, instruction_name)?;
    let idl_accounts = idl_instruction
        .get("accounts")
        .and_then(serde_json::Value::as_array)
        .context("public IDL instruction omitted accounts")?;
    ensure!(
        idl_accounts.len() == accounts.len(),
        "public instruction account count differs from exact IDL"
    );

    let mut prepared_accounts = Vec::with_capacity(accounts.len());
    for (index, ((provided_name, account_id), idl_account)) in
        accounts.iter().zip(idl_accounts).enumerate()
    {
        let expected_name = idl_account
            .get("name")
            .and_then(serde_json::Value::as_str)
            .context("public IDL account omitted its name")?;
        ensure!(
            *provided_name == expected_name,
            "public instruction account {index} must be {expected_name}, got {provided_name}"
        );
        prepared_accounts.push(PublicInstructionAccount {
            name: expected_name.to_owned(),
            account_id: account_id.to_string(),
            writable: idl_account
                .get("writable")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
            signer: idl_account
                .get("signer")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
            init: idl_account
                .get("init")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
        });
    }

    let instruction_data_words = Program::serialize_instruction(instruction)
        .context("serialize public instruction with the official Risc0 codec")?;
    let actual_variant = instruction_data_words
        .first()
        .copied()
        .context("serialized public instruction is empty")?;
    ensure!(
        actual_variant == variant_index,
        "serialized public instruction variant {actual_variant} differs from IDL tag {variant_index}"
    );
    Ok(PublicInstructionAssembly {
        schema_version: PUBLIC_INSTRUCTION_ASSEMBLY_SCHEMA_VERSION,
        program_id_words: ZEC_ESCROW_V02_ID,
        program_id_hex: program_id_hex(ZEC_ESCROW_V02_ID),
        elf_sha256,
        image_id,
        idl_sha256: interface.idl_sha256,
        instruction_name: instruction_name.to_owned(),
        variant_index,
        accounts: prepared_accounts,
        instruction_data_words,
    })
}

fn metadata_account_id(swap_id: [u8; 32]) -> AccountId {
    AccountId::for_public_pda(&ZEC_ESCROW_V02_ID, &PdaSeed::new(swap_id))
}

fn associated_token_account(
    ata_program: [u32; 8],
    owner: AccountId,
    definition: AccountId,
) -> AccountId {
    ata_core::get_associated_token_account_id(
        &ata_program,
        &ata_core::compute_ata_seed(owner, definition),
    )
}

fn assemble_initialize_token_witnessed(
    manifest: &DeploymentManifest,
    request: InitializeTokenWitnessedAssembly,
) -> Result<PublicInstructionAssembly> {
    ensure!(
        request.metadata == metadata_account_id(request.swap_id),
        "metadata account is not the exact public swap PDA"
    );
    ensure!(
        request.terms_hash != [0; 32]
            && request.aggregate_x_only_public_key != [0; 32]
            && request.amount > 0
            && request.refund_at > 0,
        "witnessed-token initialize terms must be nonzero"
    );
    ensure!(
        request.aggregate_authority != request.claimant_owner,
        "aggregate authority must remain separate from the fixed claimant"
    );
    let accounts = [
        ("metadata", request.metadata),
        ("depositor_owner", request.depositor_owner),
        ("claimant_owner", request.claimant_owner),
        ("token_definition", request.token_definition),
        ("aggregate_authority", request.aggregate_authority),
    ];
    assemble_checked_public_instruction(
        manifest,
        "initialize_token_witnessed",
        &accounts,
        EscrowInstruction::InitializeTokenWitnessed {
            swap_id: request.swap_id,
            terms_hash: request.terms_hash,
            aggregate_x_only_public_key: request.aggregate_x_only_public_key,
            amount: request.amount,
            refund_at: request.refund_at,
            ata_program: manifest.target.associated_token_account_program_id,
        },
    )
}

fn assemble_claim_token_witnessed(
    manifest: &DeploymentManifest,
    request: ClaimTokenWitnessedAssembly,
) -> Result<PublicInstructionAssembly> {
    ensure!(
        request.metadata == metadata_account_id(request.swap_id),
        "metadata account is not the exact public swap PDA"
    );
    ensure!(
        request.custody
            == associated_token_account(
                manifest.target.associated_token_account_program_id,
                request.metadata,
                request.token_definition,
            ),
        "custody is not the exact metadata-owned ATA for the token definition"
    );
    ensure!(
        request.claimant_asset
            == associated_token_account(
                manifest.target.associated_token_account_program_id,
                request.claimant_owner,
                request.token_definition,
            ),
        "claimant asset is not the exact fixed claimant ATA for the token definition"
    );
    ensure!(
        request.aggregate_authority != request.claimant_owner,
        "aggregate authority must remain separate from the fixed claimant"
    );
    let accounts = [
        ("metadata", request.metadata),
        ("custody", request.custody),
        ("claimant_owner", request.claimant_owner),
        ("claimant_asset", request.claimant_asset),
        ("aggregate_authority", request.aggregate_authority),
    ];
    assemble_checked_public_instruction(
        manifest,
        "claim_token_witnessed",
        &accounts,
        EscrowInstruction::ClaimTokenWitnessed {
            swap_id: request.swap_id,
        },
    )
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
    generated_public_interface(manifest)?;
    Ok((elf_sha256, image_id))
}

fn validate_m4_local_target(
    runtime_manifest: &DeploymentManifest,
    manifest: &M4DeploymentManifest,
) -> Result<(String, String)> {
    ensure!(
        runtime_manifest.target.rpc_url == OFFICIAL_RPC_URL
            && runtime_manifest.target.channel_id == OFFICIAL_CHANNEL_ID
            && runtime_manifest.target.authenticated_transfer_program_id
                == OFFICIAL_AUTHENTICATED_TRANSFER_PROGRAM_ID
            && runtime_manifest.target.token_program_id == OFFICIAL_TOKEN_PROGRAM_ID
            && runtime_manifest.target.associated_token_account_program_id
                == OFFICIAL_ASSOCIATED_TOKEN_ACCOUNT_PROGRAM_ID
            && runtime_manifest
                .target
                .associated_token_account_identity_source
                == OFFICIAL_ASSOCIATED_TOKEN_ACCOUNT_IDENTITY_SOURCE,
        "immutable M4 runtime program identities differ from checked LEZ v0.2"
    );
    ensure!(
        manifest.format_version == 1
            && manifest.milestone == "M4"
            && manifest.program == "zec_escrow_v02"
            && manifest.artifact_status == "local-checked-artifact"
            && !manifest.public_deployment,
        "immutable M4 manifest does not authorize local-only checked deployment"
    );
    ensure!(
        manifest.interface.instruction_count == 18
            && manifest.interface.initialize_native_xmr_variant == 13
            && manifest.interface.authorize_native_xmr_claim_variant == 14
            && manifest.interface.claim_native_xmr_variant == 15
            && manifest.interface.refund_native_xmr_variant == 16
            && manifest.interface.punish_native_xmr_variant == 17,
        "immutable M4 manifest instruction boundary differs from tags 13 through 17"
    );
    ensure!(
        manifest.artifact.elf_sha256 == M4_CHECKED_ELF_SHA256
            && manifest.artifact.image_id == M4_CHECKED_PROGRAM_ID,
        "immutable M4 manifest artifact identity differs from the checked M4 guest"
    );
    let (elf_sha256, image_id) = checked_artifact()?;
    ensure!(
        elf_sha256 == M4_CHECKED_ELF_SHA256,
        "embedded M4 ELF SHA-256 differs from the exact checked artifact"
    );
    ensure!(
        image_id == M4_CHECKED_PROGRAM_ID
            && program_id_hex(ZEC_ESCROW_V02_ID) == M4_CHECKED_PROGRAM_ID,
        "embedded M4 ImageID or ProgramId differs from the exact checked program"
    );

    let idl: serde_json::Value =
        serde_json::from_str(PROGRAM_IDL_JSON).context("parse generated M4 public SpEL IDL")?;
    let instructions = idl
        .get("instructions")
        .and_then(serde_json::Value::as_array)
        .context("generated M4 public SpEL IDL omitted instructions")?;
    let xmr_names = [
        "initialize_native_xmr",
        "authorize_native_xmr_claim",
        "claim_native_xmr",
        "refund_native_xmr",
        "punish_native_xmr",
    ];
    ensure!(
        instructions.len() == manifest.interface.instruction_count
            && xmr_names.iter().enumerate().all(|(offset, expected)| {
                instructions[13 + offset]
                    .get("name")
                    .and_then(serde_json::Value::as_str)
                    == Some(*expected)
            }),
        "generated M4 public SpEL IDL differs from exact append-only tags 13 through 17"
    );
    Ok((elf_sha256, image_id))
}

fn trusted_expected_target(manifest: &DeploymentManifest) -> Result<TrustedExpectedTarget> {
    let (elf_sha256, image_id) = validate_immutable_target(manifest)?;
    let transaction = ProgramDeploymentTransaction::new(
        nssa::program_deployment_transaction::Message::new(ZEC_ESCROW_V02_ELF.to_vec()),
    );
    Ok(TrustedExpectedTarget {
        rpc_url: manifest.target.rpc_url.clone(),
        channel_id: manifest.target.channel_id.clone(),
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
        deployment_transaction_hash: HashType(transaction.hash()).to_string(),
    })
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

async fn preflight_m4_local_with_client(
    runtime_manifest: &DeploymentManifest,
    m4_manifest: &M4DeploymentManifest,
    client: &SequencerClient,
    rpc_url: &str,
    channel_id: &str,
) -> Result<PreflightEvidence> {
    validate_local_rpc_url(rpc_url)?;
    validate_channel_id(channel_id)?;
    let artifact_identity = validate_m4_local_target(runtime_manifest, m4_manifest)?;
    preflight_verified_target(
        runtime_manifest,
        client,
        rpc_url,
        channel_id,
        artifact_identity,
    )
    .await
}

async fn preflight_checked_target(
    manifest: &DeploymentManifest,
    client: &SequencerClient,
    rpc_url: &str,
    expected_channel_id: &str,
) -> Result<PreflightEvidence> {
    let artifact_identity = validate_immutable_target(manifest)?;
    preflight_verified_target(
        manifest,
        client,
        rpc_url,
        expected_channel_id,
        artifact_identity,
    )
    .await
}

async fn preflight_verified_target(
    manifest: &DeploymentManifest,
    client: &SequencerClient,
    rpc_url: &str,
    expected_channel_id: &str,
    artifact_identity: (String, String),
) -> Result<PreflightEvidence> {
    let (elf_sha256, image_id) = artifact_identity;
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
    let genesis = client
        .get_block(nssa::GENESIS_BLOCK_ID)
        .await
        .context("read LEZ genesis block")?
        .context("LEZ genesis block is unavailable")?;
    ensure!(
        genesis.header.block_id == nssa::GENESIS_BLOCK_ID,
        "LEZ genesis block has an unexpected ID"
    );
    let genesis_block_hash = genesis.header.hash.to_string();
    validate_nonzero_lower_hex(&genesis_block_hash, "genesis block hash")?;
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
    ensure!(
        last_block_id >= nssa::GENESIS_BLOCK_ID,
        "LEZ tip predates genesis"
    );
    let evidence = PreflightEvidence {
        rpc_url: rpc_url.to_owned(),
        channel_id,
        genesis_block_id: nssa::GENESIS_BLOCK_ID,
        genesis_block_hash,
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
        schema_version: DEPLOYMENT_EVIDENCE_SCHEMA_VERSION,
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

fn provision_runtime_identity(
    manifest: &DeploymentManifest,
    evidence_path: &Path,
    authentication_key_path: &Path,
    output_path: &Path,
) -> Result<()> {
    let authentication_key = read_evidence_authentication_key(authentication_key_path)?;
    let expected = trusted_expected_target(manifest)?;
    provision_runtime_identity_for_target(
        &expected,
        evidence_path,
        &authentication_key,
        output_path,
    )
}

fn provision_runtime_identity_for_target(
    expected: &TrustedExpectedTarget,
    evidence_path: &Path,
    authentication_key: &[u8],
    output_path: &Path,
) -> Result<()> {
    let (authenticated, evidence_sha256) = read_deployment_evidence(evidence_path)?;
    let evidence = verify_authenticated_deployment_evidence(&authenticated, authentication_key)?;
    validate_deployment_evidence(expected, &evidence)?;
    let identity = ProvisionedRuntimeIdentity {
        schema_version: PROVISIONED_IDENTITY_SCHEMA_VERSION,
        status: "deployed_and_observed".to_owned(),
        environment: "public_testnet_v0_2".to_owned(),
        compatibility: "lee_v0_2".to_owned(),
        rpc_url: evidence.preflight.rpc_url.clone(),
        chain_id: evidence.preflight.channel_id.clone(),
        channel_id: evidence.preflight.channel_id.clone(),
        genesis_block_id: evidence.preflight.genesis_block_id,
        genesis_block_hash: evidence.preflight.genesis_block_hash.clone(),
        escrow_program_id_words: evidence.preflight.program_id_words,
        escrow_program_id_hex: program_id_hex(evidence.preflight.program_id_words),
        elf_sha256: evidence.preflight.elf_sha256.clone(),
        image_id: evidence.preflight.image_id.clone(),
        deployment_evidence_sha256: evidence_sha256,
        deployment_transaction_hash: evidence.transaction_hash.clone(),
        inclusion_block_id: evidence.inclusion_block_id,
        inclusion_block_hash: evidence.inclusion_block_hash.clone(),
    };
    let mut bytes = serde_json::to_vec_pretty(&identity).context("encode provisioned identity")?;
    bytes.push(b'\n');
    ensure!(
        bytes.len() <= MAX_DEPLOYMENT_EVIDENCE_BYTES,
        "provisioned identity exceeds its finite bound"
    );
    write_no_clobber(output_path, &bytes)
}

fn read_deployment_evidence(path: &Path) -> Result<(AuthenticatedDeploymentEvidence, String)> {
    let metadata = fs::symlink_metadata(path).context("deployment evidence is unavailable")?;
    ensure!(
        metadata.file_type().is_file()
            && metadata.len() != 0
            && metadata.len() <= MAX_DEPLOYMENT_EVIDENCE_BYTES as u64,
        "deployment evidence must be one bounded regular file"
    );
    let file = File::open(path).context("open deployment evidence")?;
    let opened = file
        .metadata()
        .context("inspect opened deployment evidence")?;
    ensure!(
        opened.is_file() && opened.len() != 0,
        "opened deployment evidence is not a regular file"
    );
    #[cfg(unix)]
    ensure!(
        metadata.dev() == opened.dev() && metadata.ino() == opened.ino(),
        "deployment evidence changed while it was opened"
    );
    let mut bytes = Vec::with_capacity(MAX_DEPLOYMENT_EVIDENCE_BYTES + 1);
    file.take((MAX_DEPLOYMENT_EVIDENCE_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .context("read bounded deployment evidence")?;
    ensure!(
        !bytes.is_empty() && bytes.len() <= MAX_DEPLOYMENT_EVIDENCE_BYTES,
        "deployment evidence changed outside its finite bound"
    );
    let sha256 = hex::encode(Sha256::digest(&bytes));
    let evidence = serde_json::from_slice(&bytes).context("parse deployment evidence")?;
    Ok((evidence, sha256))
}

fn authenticate_deployment_evidence(
    evidence: &DeploymentEvidence,
    authentication_key: &[u8],
) -> Result<AuthenticatedDeploymentEvidence> {
    ensure!(
        authentication_key.len() == EVIDENCE_AUTHENTICATION_KEY_BYTES,
        "deployment evidence authentication key has an invalid length"
    );
    let payload = serde_json::to_vec(evidence)
        .context("encode deployment evidence authentication payload")?;
    let mut mac = Hmac::<Sha256>::new_from_slice(authentication_key)
        .context("initialize deployment evidence authentication")?;
    mac.update(EVIDENCE_AUTHENTICATION_DOMAIN);
    mac.update(&payload);
    Ok(AuthenticatedDeploymentEvidence {
        schema_version: AUTHENTICATED_EVIDENCE_SCHEMA_VERSION,
        algorithm: EVIDENCE_AUTHENTICATION_ALGORITHM.to_owned(),
        evidence: evidence.clone(),
        authentication_tag: hex::encode(mac.finalize().into_bytes()),
    })
}

fn verify_authenticated_deployment_evidence(
    authenticated: &AuthenticatedDeploymentEvidence,
    authentication_key: &[u8],
) -> Result<DeploymentEvidence> {
    ensure!(
        authenticated.schema_version == AUTHENTICATED_EVIDENCE_SCHEMA_VERSION
            && authenticated.algorithm == EVIDENCE_AUTHENTICATION_ALGORITHM,
        "unsupported deployment evidence authentication"
    );
    ensure!(
        authentication_key.len() == EVIDENCE_AUTHENTICATION_KEY_BYTES,
        "deployment evidence authentication key has an invalid length"
    );
    validate_nonzero_lower_hex(
        &authenticated.authentication_tag,
        "deployment evidence authentication tag",
    )?;
    let payload = serde_json::to_vec(&authenticated.evidence)
        .context("encode deployment evidence authentication payload")?;
    let tag = hex::decode(&authenticated.authentication_tag)
        .context("decode deployment evidence authentication tag")?;
    let mut mac = Hmac::<Sha256>::new_from_slice(authentication_key)
        .context("initialize deployment evidence authentication")?;
    mac.update(EVIDENCE_AUTHENTICATION_DOMAIN);
    mac.update(&payload);
    mac.verify_slice(&tag)
        .map_err(|_| anyhow::anyhow!("deployment evidence authentication failed"))?;
    Ok(authenticated.evidence.clone())
}

fn read_evidence_authentication_key(path: &Path) -> Result<Zeroizing<Vec<u8>>> {
    let metadata =
        fs::symlink_metadata(path).context("deployment evidence authentication key unavailable")?;
    ensure!(
        metadata.file_type().is_file()
            && metadata.len() == EVIDENCE_AUTHENTICATION_KEY_BYTES as u64,
        "deployment evidence authentication key must be one 32-byte regular file"
    );
    #[cfg(unix)]
    ensure!(
        metadata.permissions().mode() & 0o077 == 0,
        "deployment evidence authentication key must not grant group or other permissions"
    );
    let file = File::open(path).context("open deployment evidence authentication key")?;
    let opened = file
        .metadata()
        .context("inspect opened deployment evidence authentication key")?;
    ensure!(
        opened.is_file() && opened.len() == EVIDENCE_AUTHENTICATION_KEY_BYTES as u64,
        "opened deployment evidence authentication key is invalid"
    );
    #[cfg(unix)]
    ensure!(
        metadata.dev() == opened.dev() && metadata.ino() == opened.ino(),
        "deployment evidence authentication key changed while it was opened"
    );
    let mut key = Zeroizing::new(Vec::with_capacity(EVIDENCE_AUTHENTICATION_KEY_BYTES + 1));
    file.take((EVIDENCE_AUTHENTICATION_KEY_BYTES + 1) as u64)
        .read_to_end(&mut key)
        .context("read deployment evidence authentication key")?;
    ensure!(
        key.len() == EVIDENCE_AUTHENTICATION_KEY_BYTES,
        "deployment evidence authentication key changed while it was read"
    );
    Ok(key)
}

fn validate_deployment_evidence(
    expected: &TrustedExpectedTarget,
    evidence: &DeploymentEvidence,
) -> Result<()> {
    ensure!(
        evidence.schema_version == DEPLOYMENT_EVIDENCE_SCHEMA_VERSION,
        "unsupported deployment evidence schema"
    );
    let preflight = &evidence.preflight;
    ensure!(
        preflight.rpc_url == expected.rpc_url,
        "deployment evidence RPC URL differs from the trusted target"
    );
    ensure!(
        preflight.channel_id == expected.channel_id,
        "deployment evidence channel differs from the trusted target"
    );
    validate_channel_id(&preflight.channel_id)?;
    ensure!(
        preflight.genesis_block_id == nssa::GENESIS_BLOCK_ID,
        "deployment evidence has an unexpected genesis block ID"
    );
    validate_nonzero_lower_hex(&preflight.genesis_block_hash, "genesis block hash")?;
    ensure!(
        preflight.elf_sha256 == expected.elf_sha256
            && preflight.image_id == expected.image_id
            && preflight.program_id_words == expected.program_id_words,
        "deployment evidence artifact identity differs from the checked guest"
    );
    ensure!(
        program_id_hex(preflight.program_id_words) == preflight.image_id,
        "deployment evidence ProgramId words differ from its Risc0 ImageID encoding"
    );
    ensure!(
        preflight.authenticated_transfer_program_id == expected.authenticated_transfer_program_id
            && preflight.token_program_id == expected.token_program_id
            && preflight.associated_token_account_program_id
                == expected.associated_token_account_program_id
            && preflight.associated_token_account_identity_source
                == expected.associated_token_account_identity_source,
        "deployment evidence built-in identity differs from the immutable manifest"
    );
    ensure!(
        preflight
            .rpc_program_names
            .windows(2)
            .all(|names| names[0] < names[1])
            && preflight
                .rpc_program_names
                .iter()
                .any(|name| name == "authenticated_transfer")
            && preflight
                .rpc_program_names
                .iter()
                .any(|name| name == "token")
            && !preflight
                .rpc_program_names
                .iter()
                .any(|name| name == "associated_token_account"),
        "deployment evidence RPC program inventory is invalid"
    );
    ensure!(
        preflight.last_block_id >= preflight.genesis_block_id,
        "deployment evidence preflight tip predates genesis"
    );
    validate_nonzero_lower_hex(&evidence.transaction_hash, "deployment transaction hash")?;
    ensure!(
        evidence.transaction_hash == expected.deployment_transaction_hash,
        "deployment evidence transaction hash differs from the checked deployment bytes"
    );
    ensure!(
        evidence.inclusion_block_id > preflight.last_block_id,
        "deployment evidence inclusion does not follow the pre-submission tip"
    );
    validate_nonzero_lower_hex(&evidence.inclusion_block_hash, "inclusion block hash")?;
    ensure!(
        evidence.inclusion_block_hash != preflight.genesis_block_hash,
        "deployment evidence aliases genesis and inclusion blocks"
    );
    Ok(())
}

fn validate_nonzero_lower_hex(value: &str, name: &str) -> Result<()> {
    ensure!(
        value.len() == 64
            && value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            && value.bytes().any(|byte| byte != b'0'),
        "{name} must be nonzero canonical lowercase 32-byte hex"
    );
    Ok(())
}

/// LEZ represents its Risc0 ProgramId as eight native words; its canonical
/// runtime hex is the concatenation of each word's little-endian bytes, which
/// is the same 32-byte encoding printed as the checked Risc0 ImageID.
fn program_id_hex(words: [u32; 8]) -> String {
    let bytes = words
        .into_iter()
        .flat_map(u32::to_le_bytes)
        .collect::<Vec<_>>();
    hex::encode(bytes)
}

fn write_no_clobber(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let parent_metadata =
        fs::symlink_metadata(parent).context("inspect provisioned identity output directory")?;
    ensure!(
        parent_metadata.is_dir(),
        "provisioned identity output parent must be a directory"
    );
    #[cfg(unix)]
    ensure!(
        parent_metadata.permissions().mode() & 0o022 == 0,
        "provisioned identity output directory must not be group or other writable"
    );
    let mut temporary =
        NamedTempFile::new_in(parent).context("create temporary identity output")?;
    temporary
        .write_all(bytes)
        .context("write temporary identity output")?;
    temporary
        .as_file()
        .sync_all()
        .context("sync temporary identity output")?;
    match temporary.persist_noclobber(path) {
        Ok(_) => {
            #[cfg(unix)]
            File::open(parent)
                .context("open provisioned identity output directory")?
                .sync_all()
                .context("sync provisioned identity output directory")?;
            Ok(())
        }
        Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {
            anyhow::bail!("provisioned identity output already exists")
        }
        Err(error) => Err(error.error).context("persist provisioned identity output"),
    }
}

#[cfg(test)]
mod tests;
