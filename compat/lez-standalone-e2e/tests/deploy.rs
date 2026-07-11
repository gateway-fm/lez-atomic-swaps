use std::{path::PathBuf, time::Duration};

use anyhow::{Context as _, Result, ensure};
use bytesize::ByteSize;
use common::{HashType, transaction::NSSATransaction};
use nssa::{ProgramDeploymentTransaction, program::Program};
use sequencer_service::{BedrockConfig, SequencerConfig};
use sequencer_service_rpc::{RpcClient as _, SequencerClientBuilder};

const LEZ_PIN: &str = "v0.1.2/cf3639d8252040d13b3d4e933feb19b42c76e14a";

fn isolated_config(home: PathBuf) -> SequencerConfig {
    SequencerConfig {
        home,
        genesis_id: 1,
        is_genesis_random: false,
        max_num_tx_in_block: 20,
        max_block_size: ByteSize::mib(4),
        mempool_max_size: 1_000,
        block_create_timeout: Duration::from_millis(250),
        retry_pending_blocks_timeout: Duration::from_millis(100),
        signing_key: [37; 32],
        bedrock_config: BedrockConfig {
            backoff: Default::default(),
            channel_id: [0; 32].into(),
            node_url: "http://127.0.0.1:1".parse().expect("static URL"),
            auth: None,
        },
        indexer_rpc_url: "ws://127.0.0.1:1".parse().expect("static URL"),
        initial_public_accounts: Some(testnet_initial_state::initial_accounts()),
        initial_private_accounts: Some(testnet_initial_state::initial_commitments()),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires the separately built Risc0 guest ELF"]
async fn deploys_exact_guest_through_isolated_standalone_rpc_and_block() -> Result<()> {
    let _ = env_logger::builder().is_test(true).try_init();
    let elf_path = std::env::var_os("LEZ_ESCROW_GUEST_ELF")
        .map(PathBuf::from)
        .context("LEZ_ESCROW_GUEST_ELF must name the checked guest ELF")?;
    let elf = std::fs::read(&elf_path)
        .with_context(|| format!("read guest ELF at {}", elf_path.display()))?;
    ensure!(!elf.is_empty(), "guest ELF must not be empty");
    let program = Program::new(elf.clone()).context("guest must be a canonical LEZ program")?;

    let home = tempfile::tempdir().context("create isolated sequencer state")?;
    let handle = sequencer_service::run(isolated_config(home.path().to_path_buf()), 0)
        .await
        .context("start exact pinned standalone sequencer")?;
    ensure!(handle.is_healthy(), "standalone sequencer must be healthy");
    let url = format!("http://127.0.0.1:{}", handle.addr().port());
    let client = SequencerClientBuilder::default()
        .build(url)
        .context("build standalone RPC client")?;
    client
        .check_health()
        .await
        .context("standalone health RPC")?;
    let genesis = client
        .get_last_block_id()
        .await
        .context("genesis block id")?;
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            ensure!(
                handle.is_healthy(),
                "standalone sequencer stopped before readiness"
            );
            let current = client
                .get_last_block_id()
                .await
                .context("readiness block id")?;
            if current > genesis {
                break Ok::<_, anyhow::Error>(current);
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .context("standalone did not produce its mandatory-clock readiness block")??;
    let before = client
        .get_last_block_id()
        .await
        .context("pre-deployment block id")?;

    let message = nssa::program_deployment_transaction::Message::new(elf);
    let deployment = ProgramDeploymentTransaction::new(message);
    let expected_hash = HashType(deployment.hash());
    let submitted_hash = client
        .send_transaction(NSSATransaction::ProgramDeployment(deployment))
        .await
        .context("submit deployment through sendTransaction")?;
    ensure!(
        submitted_hash == expected_hash,
        "deployment hash must be canonical"
    );

    let included = tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            ensure!(
                handle.is_healthy(),
                "standalone sequencer stopped while deployment was pending"
            );
            if let Some(transaction) = client
                .get_transaction(expected_hash)
                .await
                .context("poll getTransaction")?
            {
                break Ok::<_, anyhow::Error>(transaction);
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await;
    let included = match included {
        Ok(Ok(transaction)) => transaction,
        Ok(Err(_poll_error)) if !handle.is_healthy() => {
            return match handle.failed().await {
                Ok(never) => match never {},
                Err(error) => {
                    Err(error).context("standalone sequencer failed before including deployment")
                }
            };
        }
        Ok(Err(poll_error)) => return Err(poll_error),
        Err(_) => {
            let last = client
                .get_last_block_id()
                .await
                .context("diagnostic last block id")?;
            anyhow::bail!(
                "deployment was not included before 60s timeout; chain advanced from {before:?} to {last:?}; sequencer remained healthy"
            );
        }
    };
    ensure!(
        matches!(included, NSSATransaction::ProgramDeployment(_)),
        "included transaction must be the deployment"
    );
    let after = client
        .get_last_block_id()
        .await
        .context("included block id")?;
    ensure!(
        after > before,
        "deployment must advance the canonical block chain"
    );

    println!(
        "proved LEZ {LEZ_PIN} guest deployment: program_id={:?} tx={} block={after:?}",
        program.id(),
        expected_hash
    );
    drop(handle);
    drop(home);
    Ok(())
}
