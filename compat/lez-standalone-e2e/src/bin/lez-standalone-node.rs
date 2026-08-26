//! External exact-LEZ-v0.1.2 checked-guest local node process.

#![forbid(unsafe_code)]

use std::path::PathBuf;

use anyhow::{Context as _, Result, bail, ensure};
use lez_standalone_e2e::{
    GENESIS_BLOCK_ID, LOCAL_CHANNEL_ID, LocalActorManifest, LocalNodeReadinessManifest,
    deploy_checked_guest, isolated_config, verify_checked_guest_artifact, wait_for_chain_advance,
    write_private_readiness_manifest,
};
use nssa::program::Program;
use sequencer_service_rpc::{RpcClient as _, SequencerClientBuilder};
use tokio::io::AsyncReadExt as _;

struct Args {
    home: PathBuf,
    guest_elf: PathBuf,
    artifact_manifest: PathBuf,
    readiness_manifest: PathBuf,
}

impl Args {
    fn parse() -> Result<Self> {
        let mut home = None;
        let mut guest_elf = None;
        let mut artifact_manifest = None;
        let mut readiness_manifest = None;
        let mut arguments = std::env::args_os().skip(1);
        while let Some(flag) = arguments.next() {
            let value = arguments
                .next()
                .with_context(|| format!("missing value for {}", flag.to_string_lossy()))?;
            match flag.to_str() {
                Some("--home") if home.is_none() => home = Some(PathBuf::from(value)),
                Some("--guest-elf") if guest_elf.is_none() => {
                    guest_elf = Some(PathBuf::from(value));
                }
                Some("--artifact-manifest") if artifact_manifest.is_none() => {
                    artifact_manifest = Some(PathBuf::from(value));
                }
                Some("--readiness-manifest") if readiness_manifest.is_none() => {
                    readiness_manifest = Some(PathBuf::from(value));
                }
                _ => bail!("unknown or duplicate argument {}", flag.to_string_lossy()),
            }
        }
        Ok(Self {
            home: home.context("--home is required")?,
            guest_elf: guest_elf.context("--guest-elf is required")?,
            artifact_manifest: artifact_manifest.context("--artifact-manifest is required")?,
            readiness_manifest: readiness_manifest.context("--readiness-manifest is required")?,
        })
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse()?;
    ensure!(
        !args.readiness_manifest.exists(),
        "readiness manifest already exists"
    );
    let guest_elf = std::fs::read(&args.guest_elf)
        .with_context(|| format!("read checked guest ELF at {}", args.guest_elf.display()))?;
    let guest_identity = verify_checked_guest_artifact(&guest_elf, &args.artifact_manifest)
        .context("verify guest against tracked artifact manifest")?;
    create_private_node_home(&args.home)?;

    let handle = sequencer_service::run(isolated_config(args.home), 0)
        .await
        .context("start exact pinned standalone sequencer on a dynamic port")?;
    ensure!(handle.is_healthy(), "standalone sequencer must be healthy");
    let endpoint = format!("http://127.0.0.1:{}", handle.addr().port());
    let client = SequencerClientBuilder::default()
        .build(endpoint.clone())
        .context("build official standalone RPC client")?;
    client
        .check_health()
        .await
        .context("official standalone health RPC")?;
    let genesis = client
        .get_block(GENESIS_BLOCK_ID)
        .await
        .context("official genesis block RPC")?
        .context("standalone genesis block must exist")?;
    wait_for_chain_advance(&client, &handle, GENESIS_BLOCK_ID)
        .await
        .context("standalone mandatory-clock readiness block")?;
    let escrow_deployment = deploy_checked_guest(&client, &handle, guest_elf)
        .await
        .context("deploy checked escrow guest")?;

    let built_in_programs = client
        .get_program_ids()
        .await
        .context("official built-in program identity RPC")?;
    let native_program = *built_in_programs
        .get("authenticated_transfer")
        .context("authenticated-transfer built-in must be advertised")?;
    ensure!(
        native_program == Program::authenticated_transfer_program().id(),
        "advertised authenticated-transfer identity must match the pinned built-in"
    );
    let mut actor_manifest = Vec::new();
    for actor in testnet_initial_state::initial_pub_accounts_private_keys()
        .into_iter()
        .take(2)
    {
        let account = client
            .get_account(actor.account_id)
            .await
            .context("fetch deterministic genesis actor through official RPC")?;
        ensure!(
            account.program_owner == native_program && account.balance > 0,
            "deterministic actor must be funded and authenticated-transfer owned"
        );
        actor_manifest.push(LocalActorManifest {
            account_id: actor.account_id.to_string(),
            private_key: actor.pub_sign_key.to_string(),
            balance: account.balance,
        });
    }
    ensure!(
        actor_manifest.len() == 2,
        "two deterministic actors are required"
    );
    let readiness = LocalNodeReadinessManifest {
        schema_version: 2,
        endpoint,
        genesis_block_id: GENESIS_BLOCK_ID,
        genesis_block_hash: genesis.header.hash.to_string(),
        channel_id: hex::encode(LOCAL_CHANNEL_ID),
        elf_sha256: guest_identity.elf_sha256,
        image_id: guest_identity.image_id,
        escrow_program_id: escrow_deployment.program.id(),
        authenticated_transfer_program_id: native_program,
        deployment_transaction_hash: escrow_deployment.transaction_hash,
        deployment_block_id: escrow_deployment.inclusion_block_id,
        deployment_block_hash: escrow_deployment.inclusion_block_hash,
        actors: actor_manifest,
    };
    write_private_readiness_manifest(&args.readiness_manifest, &readiness)?;
    println!("ready");

    wait_for_shutdown()
        .await
        .context("wait for shutdown signal")?;
    drop(handle);
    Ok(())
}

fn create_private_node_home(home: &std::path::Path) -> Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt as _;
        builder.mode(0o700);
    }
    builder
        .create(home)
        .with_context(|| format!("create fresh isolated node home at {}", home.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(home, std::fs::Permissions::from_mode(0o700))
            .context("set isolated node home mode 0700")?;
    }
    Ok(())
}

async fn wait_for_shutdown() -> Result<()> {
    let mut byte = [0_u8; 1];
    let mut stdin = tokio::io::stdin();
    tokio::select! {
        read = stdin.read(&mut byte) => {
            read.context("read stdin shutdown signal")?;
        }
        signal = tokio::signal::ctrl_c() => {
            signal.context("install or receive Ctrl-C shutdown signal")?;
        }
    }
    Ok(())
}
