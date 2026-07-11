use std::{path::PathBuf, time::Duration};

use anyhow::{Context as _, Result, ensure};
use borsh::BorshDeserialize as _;
use bytesize::ByteSize;
use common::{HashType, transaction::NSSATransaction};
use lez_zec_escrow_compat::{EscrowMetadata, EscrowStatus, Instruction as EscrowInstruction};
use nssa::{
    AccountId, PrivateKey, ProgramDeploymentTransaction, PublicKey, PublicTransaction,
    program::Program,
    public_transaction::{Message, WitnessSet},
};
use sequencer_service::{BedrockConfig, SequencerConfig};
use sequencer_service_rpc::{RpcClient as _, SequencerClient, SequencerClientBuilder};
use serde::Serialize;
use sha2::{Digest as _, Sha256};
use spel_framework_core::pda::{compute_pda, seed_from_str};
use token_core::{TokenDefinition, TokenHolding};

const LEZ_PIN: &str = "v0.1.2/cf3639d8252040d13b3d4e933feb19b42c76e14a";
const TX_TIMEOUT: Duration = Duration::from_secs(60);

async fn wait_for_chain_advance(
    client: &SequencerClient,
    handle: &sequencer_service::SequencerHandle,
    before: u64,
) -> Result<u64> {
    tokio::time::timeout(TX_TIMEOUT, async {
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

async fn wait_for_inclusion(
    client: &SequencerClient,
    handle: &sequencer_service::SequencerHandle,
    hash: HashType,
) -> Result<NSSATransaction> {
    tokio::time::timeout(TX_TIMEOUT, async {
        loop {
            ensure!(
                handle.is_healthy(),
                "standalone sequencer stopped while {hash} was pending"
            );
            if let Some(transaction) = client
                .get_transaction(hash)
                .await
                .with_context(|| format!("poll getTransaction for {hash}"))?
            {
                break Ok::<_, anyhow::Error>(transaction);
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .with_context(|| format!("transaction {hash} was not included before timeout"))?
}

async fn submit_public<T: Serialize>(
    client: &SequencerClient,
    program_id: [u32; 8],
    account_ids: Vec<AccountId>,
    signers: &[(AccountId, &PrivateKey)],
    instruction: T,
) -> Result<HashType> {
    let signer_ids = signers.iter().map(|(id, _)| *id).collect();
    let nonces = client
        .get_accounts_nonces(signer_ids)
        .await
        .context("fetch signer nonces")?;
    let message = Message::try_new(program_id, account_ids, nonces, instruction)
        .context("serialize public transaction message")?;
    let keys = signers.iter().map(|(_, key)| *key).collect::<Vec<_>>();
    let transaction =
        PublicTransaction::new(message.clone(), WitnessSet::for_message(&message, &keys));
    let expected_hash = HashType(transaction.hash());
    let submitted_hash = client
        .send_transaction(NSSATransaction::Public(transaction))
        .await
        .context("submit public transaction")?;
    ensure!(
        submitted_hash == expected_hash,
        "public transaction hash must be canonical"
    );
    Ok(expected_hash)
}

async fn submit_and_include<T: Serialize>(
    client: &SequencerClient,
    handle: &sequencer_service::SequencerHandle,
    program_id: [u32; 8],
    account_ids: Vec<AccountId>,
    signers: &[(AccountId, &PrivateKey)],
    instruction: T,
) -> Result<HashType> {
    let hash = submit_public(client, program_id, account_ids, signers, instruction).await?;
    let included = wait_for_inclusion(client, handle, hash).await?;
    ensure!(
        matches!(included, NSSATransaction::Public(_)),
        "included transaction must be public"
    );
    Ok(hash)
}

async fn submit_and_reject<T: Serialize>(
    client: &SequencerClient,
    handle: &sequencer_service::SequencerHandle,
    program_id: [u32; 8],
    account_ids: Vec<AccountId>,
    signers: &[(AccountId, &PrivateKey)],
    instruction: T,
) -> Result<HashType> {
    let hash = submit_public(client, program_id, account_ids, signers, instruction).await?;
    let after_submission = client
        .get_last_block_id()
        .await
        .context("post-submission rejection block id")?;
    wait_for_chain_advance(client, handle, after_submission).await?;
    ensure!(
        client
            .get_transaction(hash)
            .await
            .context("query rejected transaction")?
            .is_none(),
        "state-invalid transaction {hash} must not enter the canonical block store"
    );
    Ok(hash)
}

async fn latest_timestamp(client: &SequencerClient) -> Result<u64> {
    let id = client
        .get_last_block_id()
        .await
        .context("latest timestamp block id")?;
    let block = client
        .get_block(id)
        .await
        .context("latest timestamp block")?
        .context("latest block must exist")?;
    Ok(block.header.timestamp)
}

fn native_ids(program_id: [u32; 8], swap_id: &[u8; 32]) -> (AccountId, AccountId) {
    let metadata = compute_pda(&program_id, &[swap_id]);
    let custody_label = seed_from_str("custody");
    let custody = compute_pda(&program_id, &[&custody_label, swap_id]);
    (metadata, custody)
}

async fn escrow_state(client: &SequencerClient, metadata: AccountId) -> Result<EscrowMetadata> {
    let account = client
        .get_account(metadata)
        .await
        .context("fetch escrow metadata")?;
    EscrowMetadata::try_from_slice(account.data.as_ref()).context("decode escrow metadata")
}

#[derive(Debug, Clone, Copy)]
struct TokenFixture {
    definition: AccountId,
    supply: AccountId,
    depositor_ata: AccountId,
    claimant_ata: AccountId,
    amount: u128,
    total_supply: u128,
}

fn keyed_account(byte: u8) -> Result<(AccountId, PrivateKey)> {
    let key = PrivateKey::try_new([byte; 32]).context("construct deterministic token key")?;
    let id = AccountId::from(&PublicKey::new_from_private_key(&key));
    Ok((id, key))
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

async fn fungible_holding(
    client: &SequencerClient,
    token_program: [u32; 8],
    holding: AccountId,
) -> Result<(AccountId, u128)> {
    let account = client
        .get_account(holding)
        .await
        .context("fetch fungible token holding")?;
    ensure!(
        account.program_owner == token_program,
        "token holding must be token-program owned"
    );
    match TokenHolding::try_from(&account.data).context("decode fungible token holding")? {
        TokenHolding::Fungible {
            definition_id,
            balance,
        } => Ok((definition_id, balance)),
        _ => anyhow::bail!("expected fungible token holding"),
    }
}

#[allow(clippy::too_many_arguments)]
async fn setup_token_fixture(
    client: &SequencerClient,
    handle: &sequencer_service::SequencerHandle,
    token_program: [u32; 8],
    ata_program: [u32; 8],
    depositor: AccountId,
    depositor_key: &PrivateKey,
    claimant: AccountId,
    claimant_key: &PrivateKey,
    definition_key_byte: u8,
    supply_key_byte: u8,
    name: &str,
    amount: u128,
) -> Result<TokenFixture> {
    let (definition, definition_key) = keyed_account(definition_key_byte)?;
    let (supply, supply_key) = keyed_account(supply_key_byte)?;
    let total_supply = 10_000_u128;

    submit_and_include(
        client,
        handle,
        token_program,
        vec![definition, supply],
        &[(definition, &definition_key), (supply, &supply_key)],
        token_core::Instruction::NewFungibleDefinition {
            name: name.to_owned(),
            total_supply,
        },
    )
    .await
    .context("create official fungible definition and supply")?;
    let definition_account = client
        .get_account(definition)
        .await
        .context("fetch token definition")?;
    ensure!(
        definition_account.program_owner == token_program,
        "definition must be token-program owned"
    );
    ensure!(
        matches!(
            TokenDefinition::try_from(&definition_account.data),
            Ok(TokenDefinition::Fungible {
                total_supply: actual,
                ..
            }) if actual == total_supply
        ),
        "definition must decode as the exact fungible supply"
    );

    let depositor_ata = associated_token_account(ata_program, depositor, definition);
    let claimant_ata = associated_token_account(ata_program, claimant, definition);
    for (owner, key, ata) in [
        (depositor, depositor_key, depositor_ata),
        (claimant, claimant_key, claimant_ata),
    ] {
        submit_and_include(
            client,
            handle,
            ata_program,
            vec![owner, definition, ata],
            &[(owner, key)],
            ata_core::Instruction::Create {
                ata_program_id: ata_program,
            },
        )
        .await
        .context("create official actor ATA")?;
        ensure!(
            fungible_holding(client, token_program, ata).await? == (definition, 0),
            "new actor ATA must hold zero of the exact definition"
        );
    }

    submit_and_include(
        client,
        handle,
        token_program,
        vec![supply, depositor_ata],
        &[(supply, &supply_key)],
        token_core::Instruction::Transfer {
            amount_to_transfer: amount,
        },
    )
    .await
    .context("fund depositor ATA from actor-keyed supply")?;
    ensure!(
        fungible_holding(client, token_program, supply).await?
            == (definition, total_supply - amount)
            && fungible_holding(client, token_program, depositor_ata).await?
                == (definition, amount),
        "fixture funding must conserve the exact token supply"
    );

    Ok(TokenFixture {
        definition,
        supply,
        depositor_ata,
        claimant_ata,
        amount,
        total_supply,
    })
}

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
async fn deploys_guest_and_executes_real_actor_lifecycles() -> Result<()> {
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
    wait_for_chain_advance(&client, &handle, genesis)
        .await
        .context("standalone did not produce its mandatory-clock readiness block")?;
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

    let included = wait_for_inclusion(&client, &handle, expected_hash).await?;
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

    let actors = testnet_initial_state::initial_pub_accounts_private_keys();
    let depositor = &actors[0];
    let claimant = &actors[1];
    let native_program = Program::authenticated_transfer_program().id();
    for actor in [depositor, claimant] {
        let funded_actor = client
            .get_account(actor.account_id)
            .await
            .context("fetch funded genesis actor")?;
        ensure!(
            funded_actor.program_owner == native_program && funded_actor.balance > 0,
            "genesis actor must already be funded and authenticated-transfer owned"
        );
    }

    let preimage = [42; 32];
    let secret_digest: [u8; 32] = Sha256::digest(preimage).into();
    let claim_swap_id = [51; 32];
    let claim_amount = 700_u128;
    let claim_refund_at = latest_timestamp(&client).await? + 600_000;
    let (claim_metadata, claim_custody) = native_ids(program.id(), &claim_swap_id);
    let depositor_before_claim_swap = client.get_account(depositor.account_id).await?.balance;
    let claimant_before_claim_swap = client.get_account(claimant.account_id).await?.balance;

    submit_and_include(
        &client,
        &handle,
        program.id(),
        vec![
            claim_metadata,
            claim_custody,
            depositor.account_id,
            claimant.account_id,
        ],
        &[(depositor.account_id, &depositor.pub_sign_key)],
        EscrowInstruction::InitializeNative {
            swap_id: claim_swap_id,
            terms_hash: [61; 32],
            secret_digest,
            amount: claim_amount,
            refund_at: claim_refund_at,
            authenticated_transfer_program: native_program,
        },
    )
    .await
    .context("initialize native swap as depositor")?;
    ensure!(
        escrow_state(&client, claim_metadata).await?.status == EscrowStatus::Empty,
        "initialized swap must be empty"
    );
    let initialized_custody = client.get_account(claim_custody).await?;
    ensure!(
        initialized_custody.program_owner == native_program && initialized_custody.balance == 0,
        "chained native initialization must claim an empty authenticated-transfer custody"
    );

    submit_and_include(
        &client,
        &handle,
        program.id(),
        vec![claim_metadata, claim_custody, depositor.account_id],
        &[(depositor.account_id, &depositor.pub_sign_key)],
        EscrowInstruction::FundNative {
            swap_id: claim_swap_id,
        },
    )
    .await
    .context("fund native swap as depositor")?;
    ensure!(
        escrow_state(&client, claim_metadata).await?.status == EscrowStatus::Funded,
        "funded swap must record funded status"
    );
    ensure!(
        client.get_account(claim_custody).await?.balance == claim_amount,
        "chained native funding must move the exact amount into custody"
    );

    let claimant_nonce_before_wrong_preimage = client.get_account(claimant.account_id).await?.nonce;
    submit_and_reject(
        &client,
        &handle,
        program.id(),
        vec![claim_metadata, claim_custody, claimant.account_id],
        &[(claimant.account_id, &claimant.pub_sign_key)],
        EscrowInstruction::ClaimNative {
            swap_id: claim_swap_id,
            preimage: [99; 32],
        },
    )
    .await
    .context("reject claimant's wrong preimage")?;
    ensure!(
        client.get_account(claimant.account_id).await?.nonce
            == claimant_nonce_before_wrong_preimage
            && escrow_state(&client, claim_metadata).await?.status == EscrowStatus::Funded
            && client.get_account(claim_custody).await?.balance == claim_amount,
        "wrong preimage must leave signer nonce and escrow state unchanged"
    );

    let depositor_nonce_before_wrong_role = client.get_account(depositor.account_id).await?.nonce;
    submit_and_reject(
        &client,
        &handle,
        program.id(),
        vec![claim_metadata, claim_custody, depositor.account_id],
        &[(depositor.account_id, &depositor.pub_sign_key)],
        EscrowInstruction::ClaimNative {
            swap_id: claim_swap_id,
            preimage,
        },
    )
    .await
    .context("reject depositor attempting the claimant role")?;
    ensure!(
        client.get_account(depositor.account_id).await?.nonce == depositor_nonce_before_wrong_role
            && escrow_state(&client, claim_metadata).await?.status == EscrowStatus::Funded
            && client.get_account(claim_custody).await?.balance == claim_amount,
        "wrong-role claim must leave signer nonce and escrow state unchanged"
    );

    submit_and_include(
        &client,
        &handle,
        program.id(),
        vec![claim_metadata, claim_custody, claimant.account_id],
        &[(claimant.account_id, &claimant.pub_sign_key)],
        EscrowInstruction::ClaimNative {
            swap_id: claim_swap_id,
            preimage,
        },
    )
    .await
    .context("claim native swap as claimant")?;
    ensure!(
        escrow_state(&client, claim_metadata).await?.status == EscrowStatus::Claimed,
        "valid preimage must terminally claim the swap"
    );
    ensure!(
        client.get_account(claim_custody).await?.balance == 0
            && client.get_account(depositor.account_id).await?.balance
                == depositor_before_claim_swap - claim_amount
            && client.get_account(claimant.account_id).await?.balance
                == claimant_before_claim_swap + claim_amount,
        "claim must transfer the exact custody amount between the real actor balances"
    );

    let refund_swap_id = [52; 32];
    let refund_amount = 900_u128;
    let refund_at = latest_timestamp(&client).await? + 60_000;
    let (refund_metadata, refund_custody) = native_ids(program.id(), &refund_swap_id);
    let depositor_before_refund_swap = client.get_account(depositor.account_id).await?.balance;

    submit_and_include(
        &client,
        &handle,
        program.id(),
        vec![
            refund_metadata,
            refund_custody,
            depositor.account_id,
            claimant.account_id,
        ],
        &[(depositor.account_id, &depositor.pub_sign_key)],
        EscrowInstruction::InitializeNative {
            swap_id: refund_swap_id,
            terms_hash: [62; 32],
            secret_digest,
            amount: refund_amount,
            refund_at,
            authenticated_transfer_program: native_program,
        },
    )
    .await?;
    submit_and_include(
        &client,
        &handle,
        program.id(),
        vec![refund_metadata, refund_custody, depositor.account_id],
        &[(depositor.account_id, &depositor.pub_sign_key)],
        EscrowInstruction::FundNative {
            swap_id: refund_swap_id,
        },
    )
    .await?;

    submit_and_reject(
        &client,
        &handle,
        program.id(),
        vec![refund_metadata, refund_custody, depositor.account_id],
        &[],
        EscrowInstruction::RefundNative {
            swap_id: refund_swap_id,
        },
    )
    .await
    .context("reject permissionless refund before chain deadline")?;
    ensure!(
        latest_timestamp(&client).await? < refund_at
            && escrow_state(&client, refund_metadata).await?.status == EscrowStatus::Funded
            && client.get_account(refund_custody).await?.balance == refund_amount,
        "refund must be rejected before the canonical deadline and leave escrow funded"
    );

    tokio::time::timeout(Duration::from_secs(90), async {
        loop {
            if latest_timestamp(&client).await? >= refund_at {
                break Ok::<_, anyhow::Error>(());
            }
            ensure!(
                handle.is_healthy(),
                "standalone sequencer stopped before refund deadline"
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .context("canonical clock did not reach refund deadline")??;
    submit_and_include(
        &client,
        &handle,
        program.id(),
        vec![refund_metadata, refund_custody, depositor.account_id],
        &[],
        EscrowInstruction::RefundNative {
            swap_id: refund_swap_id,
        },
    )
    .await
    .context("execute permissionless refund after chain deadline")?;
    ensure!(
        escrow_state(&client, refund_metadata).await?.status == EscrowStatus::Refunded
            && client.get_account(refund_custody).await?.balance == 0
            && client.get_account(depositor.account_id).await?.balance
                == depositor_before_refund_swap,
        "permissionless refund must return the exact amount only to the bound depositor"
    );

    let token_program = Program::token().id();
    let ata_program = Program::ata().id();
    let claim_token_fixture = setup_token_fixture(
        &client,
        &handle,
        token_program,
        ata_program,
        depositor.account_id,
        &depositor.pub_sign_key,
        claimant.account_id,
        &claimant.pub_sign_key,
        21,
        22,
        "ZEC swap token A",
        1_200,
    )
    .await
    .context("set up claim token definition and actor ATAs")?;
    let refund_token_fixture = setup_token_fixture(
        &client,
        &handle,
        token_program,
        ata_program,
        depositor.account_id,
        &depositor.pub_sign_key,
        claimant.account_id,
        &claimant.pub_sign_key,
        23,
        24,
        "ZEC swap token B",
        1_400,
    )
    .await
    .context("set up refund token definition and actor ATAs")?;
    ensure!(
        claim_token_fixture.definition != refund_token_fixture.definition
            && claim_token_fixture.depositor_ata != refund_token_fixture.depositor_ata
            && claim_token_fixture.claimant_ata != refund_token_fixture.claimant_ata,
        "independent definitions must derive independent actor ATAs"
    );

    let token_claim_swap_id = [91; 32];
    let token_claim_refund_at = latest_timestamp(&client).await? + 600_000;
    let token_claim_metadata = compute_pda(&program.id(), &[&token_claim_swap_id]);
    let token_claim_custody = associated_token_account(
        ata_program,
        token_claim_metadata,
        claim_token_fixture.definition,
    );
    submit_and_include(
        &client,
        &handle,
        program.id(),
        vec![
            token_claim_metadata,
            depositor.account_id,
            claimant.account_id,
            claim_token_fixture.definition,
        ],
        &[(depositor.account_id, &depositor.pub_sign_key)],
        EscrowInstruction::InitializeToken {
            swap_id: token_claim_swap_id,
            terms_hash: [101; 32],
            secret_digest,
            amount: claim_token_fixture.amount,
            refund_at: token_claim_refund_at,
            ata_program,
        },
    )
    .await
    .context("initialize token claim swap as depositor owner")?;
    let token_claim_initialized = escrow_state(&client, token_claim_metadata).await?;
    ensure!(
        token_claim_initialized.status == EscrowStatus::Empty
            && token_claim_initialized.asset_program == token_program
            && token_claim_initialized.custody_program == ata_program
            && token_claim_initialized.asset_definition
                == claim_token_fixture.definition.into_value()
            && token_claim_initialized.custody == token_claim_custody,
        "token metadata must bind the exact definition, programs, and custody ATA"
    );

    submit_and_include(
        &client,
        &handle,
        program.id(),
        vec![
            token_claim_metadata,
            claim_token_fixture.definition,
            token_claim_custody,
        ],
        &[],
        EscrowInstruction::CreateTokenCustody {
            swap_id: token_claim_swap_id,
        },
    )
    .await
    .context("permissionlessly create token claim custody ATA")?;
    ensure!(
        fungible_holding(&client, token_program, token_claim_custody).await?
            == (claim_token_fixture.definition, 0),
        "created custody must be the zero-balance official metadata ATA"
    );

    submit_and_include(
        &client,
        &handle,
        program.id(),
        vec![
            token_claim_metadata,
            depositor.account_id,
            claim_token_fixture.depositor_ata,
            token_claim_custody,
        ],
        &[(depositor.account_id, &depositor.pub_sign_key)],
        EscrowInstruction::FundToken {
            swap_id: token_claim_swap_id,
        },
    )
    .await
    .context("fund token claim swap as depositor owner")?;
    ensure!(
        escrow_state(&client, token_claim_metadata).await?.status == EscrowStatus::Funded
            && fungible_holding(&client, token_program, claim_token_fixture.depositor_ata).await?
                == (claim_token_fixture.definition, 0)
            && fungible_holding(&client, token_program, token_claim_custody).await?
                == (claim_token_fixture.definition, claim_token_fixture.amount,),
        "ATA funding must move only the exact amount into metadata custody"
    );

    let claimant_nonce_before_wrong_token_preimage =
        client.get_account(claimant.account_id).await?.nonce;
    submit_and_reject(
        &client,
        &handle,
        program.id(),
        vec![
            token_claim_metadata,
            token_claim_custody,
            claimant.account_id,
            claim_token_fixture.claimant_ata,
        ],
        &[(claimant.account_id, &claimant.pub_sign_key)],
        EscrowInstruction::ClaimToken {
            swap_id: token_claim_swap_id,
            preimage: [102; 32],
        },
    )
    .await
    .context("reject token claimant's wrong preimage")?;
    ensure!(
        client.get_account(claimant.account_id).await?.nonce
            == claimant_nonce_before_wrong_token_preimage
            && escrow_state(&client, token_claim_metadata).await?.status == EscrowStatus::Funded
            && fungible_holding(&client, token_program, token_claim_custody).await?
                == (claim_token_fixture.definition, claim_token_fixture.amount,),
        "wrong token preimage must preserve claimant nonce and custody"
    );

    let depositor_nonce_before_wrong_token_role =
        client.get_account(depositor.account_id).await?.nonce;
    submit_and_reject(
        &client,
        &handle,
        program.id(),
        vec![
            token_claim_metadata,
            token_claim_custody,
            depositor.account_id,
            claim_token_fixture.claimant_ata,
        ],
        &[(depositor.account_id, &depositor.pub_sign_key)],
        EscrowInstruction::ClaimToken {
            swap_id: token_claim_swap_id,
            preimage,
        },
    )
    .await
    .context("reject depositor attempting token claimant role")?;
    ensure!(
        client.get_account(depositor.account_id).await?.nonce
            == depositor_nonce_before_wrong_token_role
            && escrow_state(&client, token_claim_metadata).await?.status == EscrowStatus::Funded,
        "wrong token role must preserve depositor nonce and escrow state"
    );

    let claimant_nonce_before_definition_substitution =
        client.get_account(claimant.account_id).await?.nonce;
    submit_and_reject(
        &client,
        &handle,
        program.id(),
        vec![
            token_claim_metadata,
            token_claim_custody,
            claimant.account_id,
            refund_token_fixture.claimant_ata,
        ],
        &[(claimant.account_id, &claimant.pub_sign_key)],
        EscrowInstruction::ClaimToken {
            swap_id: token_claim_swap_id,
            preimage,
        },
    )
    .await
    .context("reject claimant ATA from the other definition")?;
    ensure!(
        client.get_account(claimant.account_id).await?.nonce
            == claimant_nonce_before_definition_substitution
            && fungible_holding(&client, token_program, refund_token_fixture.claimant_ata).await?
                == (refund_token_fixture.definition, 0)
            && fungible_holding(&client, token_program, token_claim_custody).await?
                == (claim_token_fixture.definition, claim_token_fixture.amount,),
        "cross-definition ATA substitution must preserve nonce and both holdings"
    );

    submit_and_include(
        &client,
        &handle,
        program.id(),
        vec![
            token_claim_metadata,
            token_claim_custody,
            claimant.account_id,
            claim_token_fixture.claimant_ata,
        ],
        &[(claimant.account_id, &claimant.pub_sign_key)],
        EscrowInstruction::ClaimToken {
            swap_id: token_claim_swap_id,
            preimage,
        },
    )
    .await
    .context("claim token swap as bound claimant owner")?;
    ensure!(
        escrow_state(&client, token_claim_metadata).await?.status == EscrowStatus::Claimed
            && fungible_holding(&client, token_program, token_claim_custody).await?
                == (claim_token_fixture.definition, 0)
            && fungible_holding(&client, token_program, claim_token_fixture.claimant_ata).await?
                == (claim_token_fixture.definition, claim_token_fixture.amount,)
            && fungible_holding(&client, token_program, claim_token_fixture.supply).await?
                == (
                    claim_token_fixture.definition,
                    claim_token_fixture.total_supply - claim_token_fixture.amount,
                ),
        "token claim must move exact custody to the correct definition-bound claimant ATA"
    );

    let token_refund_swap_id = [92; 32];
    let token_refund_at = latest_timestamp(&client).await? + 60_000;
    let token_refund_metadata = compute_pda(&program.id(), &[&token_refund_swap_id]);
    let token_refund_custody = associated_token_account(
        ata_program,
        token_refund_metadata,
        refund_token_fixture.definition,
    );
    submit_and_include(
        &client,
        &handle,
        program.id(),
        vec![
            token_refund_metadata,
            depositor.account_id,
            claimant.account_id,
            refund_token_fixture.definition,
        ],
        &[(depositor.account_id, &depositor.pub_sign_key)],
        EscrowInstruction::InitializeToken {
            swap_id: token_refund_swap_id,
            terms_hash: [103; 32],
            secret_digest,
            amount: refund_token_fixture.amount,
            refund_at: token_refund_at,
            ata_program,
        },
    )
    .await
    .context("initialize token refund swap as depositor owner")?;
    submit_and_include(
        &client,
        &handle,
        program.id(),
        vec![
            token_refund_metadata,
            refund_token_fixture.definition,
            token_refund_custody,
        ],
        &[],
        EscrowInstruction::CreateTokenCustody {
            swap_id: token_refund_swap_id,
        },
    )
    .await
    .context("permissionlessly create token refund custody ATA")?;
    submit_and_include(
        &client,
        &handle,
        program.id(),
        vec![
            token_refund_metadata,
            depositor.account_id,
            refund_token_fixture.depositor_ata,
            token_refund_custody,
        ],
        &[(depositor.account_id, &depositor.pub_sign_key)],
        EscrowInstruction::FundToken {
            swap_id: token_refund_swap_id,
        },
    )
    .await
    .context("fund token refund swap as depositor owner")?;
    ensure!(
        escrow_state(&client, token_refund_metadata).await?.status == EscrowStatus::Funded
            && fungible_holding(&client, token_program, token_refund_custody).await?
                == (refund_token_fixture.definition, refund_token_fixture.amount,),
        "token refund scenario must be funded in its independent custody ATA"
    );

    submit_and_reject(
        &client,
        &handle,
        program.id(),
        vec![
            token_refund_metadata,
            token_refund_custody,
            refund_token_fixture.depositor_ata,
        ],
        &[],
        EscrowInstruction::RefundToken {
            swap_id: token_refund_swap_id,
        },
    )
    .await
    .context("reject permissionless token refund before chain deadline")?;
    ensure!(
        latest_timestamp(&client).await? < token_refund_at
            && escrow_state(&client, token_refund_metadata).await?.status == EscrowStatus::Funded
            && fungible_holding(&client, token_program, token_refund_custody).await?
                == (refund_token_fixture.definition, refund_token_fixture.amount,),
        "early token refund must preserve the independent funded custody"
    );

    tokio::time::timeout(Duration::from_secs(90), async {
        loop {
            if latest_timestamp(&client).await? >= token_refund_at {
                break Ok::<_, anyhow::Error>(());
            }
            ensure!(
                handle.is_healthy(),
                "standalone sequencer stopped before token refund deadline"
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .context("canonical clock did not reach token refund deadline")??;
    submit_and_reject(
        &client,
        &handle,
        program.id(),
        vec![
            token_refund_metadata,
            token_refund_custody,
            claim_token_fixture.depositor_ata,
        ],
        &[],
        EscrowInstruction::RefundToken {
            swap_id: token_refund_swap_id,
        },
    )
    .await
    .context("reject token refund destination from the other definition")?;
    ensure!(
        fungible_holding(&client, token_program, claim_token_fixture.depositor_ata).await?
            == (claim_token_fixture.definition, 0)
            && fungible_holding(&client, token_program, token_refund_custody).await?
                == (refund_token_fixture.definition, refund_token_fixture.amount,),
        "wrong refund ATA must preserve both definition-bound holdings"
    );

    submit_and_include(
        &client,
        &handle,
        program.id(),
        vec![
            token_refund_metadata,
            token_refund_custody,
            refund_token_fixture.depositor_ata,
        ],
        &[],
        EscrowInstruction::RefundToken {
            swap_id: token_refund_swap_id,
        },
    )
    .await
    .context("permissionlessly refund to the bound depositor ATA")?;
    ensure!(
        escrow_state(&client, token_refund_metadata).await?.status == EscrowStatus::Refunded
            && fungible_holding(&client, token_program, token_refund_custody).await?
                == (refund_token_fixture.definition, 0)
            && fungible_holding(&client, token_program, refund_token_fixture.depositor_ata).await?
                == (refund_token_fixture.definition, refund_token_fixture.amount,)
            && fungible_holding(&client, token_program, refund_token_fixture.supply).await?
                == (
                    refund_token_fixture.definition,
                    refund_token_fixture.total_supply - refund_token_fixture.amount,
                ),
        "permissionless token refund must restore only the exact bound depositor ATA"
    );

    println!(
        "proved LEZ {LEZ_PIN} deployment plus native and two-definition token actor lifecycles: program_id={:?} tx={} block={after:?}",
        program.id(),
        expected_hash
    );
    drop(handle);
    drop(home);
    Ok(())
}
