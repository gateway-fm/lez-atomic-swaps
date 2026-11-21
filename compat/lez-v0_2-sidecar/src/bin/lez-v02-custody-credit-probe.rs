#![forbid(unsafe_code)]
//! TEST-ONLY: a stranger credits a funded escrow's custody account (TM-0032).
//!
//! LEZ lets anyone raise the balance of any initialised account, so a third
//! party holding LEZ can send units to the custody account
//! `PDA(escrow, ["custody", swap_id])` of a funded swap with a plain
//! authenticated-transfer `Transfer` it signs alone. The devnet scenarios
//! `btc-custody-credit`, `zec-custody-credit`, `xmr-custody-credit` and their
//! `-refund` variants (deploy/scripts/node-e2e.py) run this inside the Maker's
//! container with the key of a funded account that is neither party's, while
//! the swap runs, and then check that the swap still settles and that the
//! branch's recipient receives the credit with the lot.
//!
//! The probe finds the escrow either by its on-chain swap ID (`--swap-id`) or
//! by following the sequencer's new blocks for an escrow transaction that
//! names `--depositor` as its depositor (`InitializeNative*` and `FundNative`
//! carry `[metadata, custody, depositor, ...]`), and waits until that escrow
//! is `Funded`. It then sends the transfer, looks for it in the sequencer's
//! blocks and reads the custody again.
//!
//! It holds no durable state. It is not part of any role package: the devnet
//! Maker image carries it beside the package, as it carries
//! `lez-v02-escrow-claim-probe`.
//!
//! The JSON evidence on stdout names the escrow, the custody and sender
//! balances before and after, the sequencer's answer and the including block.
//! Exit status: 0 when the credit landed (the custody grew by exactly the
//! amount and the escrow is still `Funded`), 3 when it did not, 1 on any other
//! failure.

use std::{
    collections::BTreeSet,
    fs,
    os::unix::fs::{MetadataExt as _, PermissionsExt as _},
    path::{Path, PathBuf},
    str::FromStr as _,
    time::{Duration, Instant},
};

use anyhow::{Context as _, Result, bail, ensure};
use authenticated_transfer_core::Instruction as AuthenticatedTransferInstruction;
use borsh::BorshDeserialize as _;
use clap::Parser;
use common::{HashType, transaction::LeeTransaction};
use jsonrpsee::core::ClientError;
use lez_bridge_protocol::Hex32;
use lez_v0_2_sidecar::{
    compute_custody_pda, compute_metadata_pda, program_id_from_hex, validate_loopback_http_endpoint,
};
use lez_zec_escrow_v02::{EscrowMetadata, EscrowStatus};
use nssa::{
    Account, AccountId, PrivateKey, PublicKey, PublicTransaction,
    public_transaction::{Message, WitnessSet},
};
use sequencer_service_rpc::{RpcClient as _, SequencerClient, SequencerClientBuilder};
use serde::Serialize;
use zeroize::Zeroizing;

const EVIDENCE_SCHEMA: &str = "lez_v02_custody_credit_probe_v1";
const POLL_INTERVAL: Duration = Duration::from_millis(500);
const NODE_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const EXIT_CREDIT_NOT_LANDED: i32 = 3;

/// Credit a funded escrow's custody account from a stranger's key (TEST-ONLY).
#[derive(Debug, Parser)]
#[command(version, about)]
struct Arguments {
    /// Literal-loopback sequencer HTTP URL and port.
    #[arg(long)]
    sequencer_url: String,
    /// Deployed checked escrow program ID as 64 lowercase hex characters.
    #[arg(long)]
    escrow_program_id: String,
    /// The escrow's on-chain swap ID (the metadata PDA seed), 64 hex
    /// characters. Without it the escrow is found by `--depositor`.
    #[arg(long)]
    swap_id: Option<String>,
    /// The escrow depositor's LEZ account (base58), used to find the escrow
    /// in the blocks made after the probe started.
    #[arg(long)]
    depositor: Option<String>,
    /// Owner-only file holding the stranger's lowercase-hex LEZ private key.
    #[arg(long)]
    private_key_file: PathBuf,
    /// Units to send to the custody account.
    #[arg(long, default_value_t = 1)]
    amount: u128,
    /// How long to wait for the escrow to be `Funded`.
    #[arg(long, default_value_t = 1800)]
    wait_funded_seconds: u64,
    /// How long to look for the transfer in the sequencer's blocks.
    #[arg(long, default_value_t = 30)]
    observe_seconds: u64,
}

#[derive(Debug, Serialize)]
#[serde(deny_unknown_fields)]
struct Snapshot {
    sequencer_tip: u64,
    status: String,
    custody_balance: u128,
    sender_balance: u128,
}

#[derive(Debug, Serialize)]
#[serde(deny_unknown_fields)]
struct SubmissionEvidence {
    /// `accepted`, `rejected` (a JSON-RPC error) or `transport_error`.
    decision: &'static str,
    error_code: Option<i32>,
    error_message: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(deny_unknown_fields)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "each flag is one separately reported fact of the evidence record"
)]
struct ProbeEvidence {
    schema: &'static str,
    test_only: bool,
    escrow_program_id: Hex32,
    swap_id: Hex32,
    metadata_account: Hex32,
    custody_account: Hex32,
    depositor_account: Hex32,
    claimant_account: Hex32,
    sender_account: Hex32,
    sender_is_party: bool,
    escrow_amount: u128,
    credit_amount: u128,
    found_by: &'static str,
    found_in_block: Option<u64>,
    transaction_id: Hex32,
    submission: SubmissionEvidence,
    included: bool,
    included_in_block: Option<u64>,
    observed_seconds: u64,
    before: Snapshot,
    after: Snapshot,
    credit_landed: bool,
}

/// The escrow accounts an escrow-program transaction names when its third
/// account is `depositor`: `(metadata, custody)`.
fn escrow_candidate(
    message: &Message,
    escrow_program: [u32; 8],
    depositor: AccountId,
) -> Option<(AccountId, AccountId)> {
    (message.program_id == escrow_program
        && message.account_ids.len() >= 3
        && message.account_ids[2] == depositor)
        .then(|| (message.account_ids[0], message.account_ids[1]))
}

/// Whether `metadata` (read from `metadata_id`) is the native escrow whose
/// custody is `custody_id` under `escrow_program`.
fn is_native_escrow(
    metadata: &EscrowMetadata,
    escrow_program: [u32; 8],
    metadata_id: AccountId,
    custody_id: AccountId,
) -> bool {
    metadata.asset_definition == [0; 32]
        && metadata.asset_program == metadata.custody_program
        && metadata.custody == custody_id
        && compute_custody_pda(&escrow_program, &metadata.swap_id) == custody_id
        && compute_metadata_pda(&escrow_program, &metadata.swap_id) == metadata_id
}

/// The credit landed: the custody grew by exactly `amount` and the escrow did
/// not move.
fn credit_landed(
    before_custody: u128,
    after_custody: u128,
    amount: u128,
    before_status: EscrowStatus,
    after_status: EscrowStatus,
) -> bool {
    before_custody.checked_add(amount) == Some(after_custody)
        && before_status == EscrowStatus::Funded
        && after_status == EscrowStatus::Funded
}

#[tokio::main]
async fn main() {
    match execute(Arguments::parse()).await {
        Ok(evidence) => {
            let landed = evidence.credit_landed;
            match serde_json::to_string(&evidence) {
                Ok(json) => println!("{json}"),
                Err(error) => {
                    eprintln!("custody credit probe evidence encoding failed: {error}");
                    std::process::exit(1);
                }
            }
            if !landed {
                std::process::exit(EXIT_CREDIT_NOT_LANDED);
            }
        }
        Err(error) => {
            eprintln!("custody credit probe failed: {error:#}");
            std::process::exit(1);
        }
    }
}

struct Found {
    metadata_id: AccountId,
    custody_id: AccountId,
    metadata: EscrowMetadata,
    by: &'static str,
    block: Option<u64>,
}

#[allow(
    clippy::too_many_lines,
    reason = "the probe keeps the search, the before read, the transfer, the inclusion poll and the after read in one linear flow"
)]
async fn execute(arguments: Arguments) -> Result<ProbeEvidence> {
    validate_loopback_http_endpoint(&arguments.sequencer_url)
        .context("the sequencer URL must be literal-loopback HTTP with a port")?;
    ensure!(arguments.amount > 0, "the credit amount must be nonzero");
    let program_hex = parse_nonzero_hex(&arguments.escrow_program_id, "escrow program ID")?;
    let program_id = program_id_from_hex(program_hex);
    let swap_id = arguments
        .swap_id
        .as_deref()
        .map(|value| parse_nonzero_hex(value, "swap ID"))
        .transpose()?;
    let depositor = arguments
        .depositor
        .as_deref()
        .map(|value| AccountId::from_str(value).context("invalid depositor account (base58)"))
        .transpose()?;
    ensure!(
        swap_id.is_some() || depositor.is_some(),
        "name the escrow by --swap-id or --depositor"
    );
    let sender_key = read_private_key(&arguments.private_key_file)?;
    let sender = AccountId::from(&PublicKey::new_from_private_key(&sender_key));
    let client = SequencerClientBuilder::default()
        .request_timeout(NODE_REQUEST_TIMEOUT)
        .max_concurrent_requests(1)
        .build(&arguments.sequencer_url)
        .context("could not build the sequencer client")?;

    let found = find_funded_escrow(
        &client,
        program_id,
        swap_id,
        depositor,
        Duration::from_secs(arguments.wait_funded_seconds),
    )
    .await?;
    let metadata = &found.metadata;
    if let Some(depositor) = depositor {
        ensure!(
            metadata.depositor == depositor,
            "the escrow's depositor is not --depositor"
        );
    }
    let sender_is_party = sender == metadata.depositor || sender == metadata.claimant;
    ensure!(
        !sender_is_party,
        "the key is a party of the swap: the probe tests a stranger's credit"
    );

    let before_sender = account(&client, sender).await?;
    ensure!(
        before_sender.program_owner == metadata.asset_program,
        "the stranger's account is not owned by the escrow's asset program"
    );
    ensure!(
        before_sender.balance >= arguments.amount,
        "the stranger's account holds {} units, less than the credit",
        before_sender.balance
    );
    let before = snapshot(&client, found.metadata_id, found.custody_id, sender).await?;
    ensure!(
        before.1 == EscrowStatus::Funded,
        "the escrow left Funded before the credit"
    );

    let message = Message::try_new(
        metadata.asset_program,
        vec![sender, found.custody_id],
        vec![before_sender.nonce],
        AuthenticatedTransferInstruction::Transfer {
            amount: arguments.amount,
        },
    )
    .map_err(|_| anyhow::anyhow!("could not encode the authenticated-transfer message"))?;
    let witness = WitnessSet::for_message(&message, &[&sender_key]);
    let transaction = PublicTransaction::new(message, witness);
    let transaction_id = transaction.hash();
    let submission = match client
        .send_transaction(LeeTransaction::Public(transaction))
        .await
    {
        Ok(hash) if hash == HashType(transaction_id) => SubmissionEvidence {
            decision: "accepted",
            error_code: None,
            error_message: None,
        },
        Ok(hash) => bail!(
            "the sequencer acknowledged another hash ({}) than the transfer's",
            hex::encode(hash.0)
        ),
        Err(ClientError::Call(object)) => SubmissionEvidence {
            decision: "rejected",
            error_code: Some(object.code()),
            error_message: Some(object.message().to_owned()),
        },
        Err(error) => SubmissionEvidence {
            decision: "transport_error",
            error_code: None,
            error_message: Some(error.to_string()),
        },
    };

    let started = Instant::now();
    let window = Duration::from_secs(arguments.observe_seconds);
    let mut included_in_block = None;
    loop {
        if let Some((_, block)) = client
            .get_transaction(HashType(transaction_id))
            .await
            .context("could not query the sequencer for the transfer")?
        {
            included_in_block = Some(block);
            break;
        }
        if started.elapsed() >= window {
            break;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }

    let after = snapshot(&client, found.metadata_id, found.custody_id, sender).await?;
    let landed = credit_landed(
        before.0.custody_balance,
        after.0.custody_balance,
        arguments.amount,
        before.1,
        after.1,
    );
    Ok(ProbeEvidence {
        schema: EVIDENCE_SCHEMA,
        test_only: true,
        escrow_program_id: program_hex,
        swap_id: Hex32::from_bytes(metadata.swap_id),
        metadata_account: Hex32::from_bytes(found.metadata_id.into_value()),
        custody_account: Hex32::from_bytes(found.custody_id.into_value()),
        depositor_account: Hex32::from_bytes(metadata.depositor.into_value()),
        claimant_account: Hex32::from_bytes(metadata.claimant.into_value()),
        sender_account: Hex32::from_bytes(sender.into_value()),
        sender_is_party,
        escrow_amount: metadata.amount,
        credit_amount: arguments.amount,
        found_by: found.by,
        found_in_block: found.block,
        transaction_id: Hex32::from_bytes(transaction_id),
        submission,
        included: included_in_block.is_some(),
        included_in_block,
        observed_seconds: started.elapsed().as_secs(),
        before: before.0,
        after: after.0,
        credit_landed: landed,
    })
}

/// The escrow named by `swap_id`, or the one an escrow transaction made after
/// the probe started names `depositor` of, once it is `Funded`.
async fn find_funded_escrow(
    client: &SequencerClient,
    program_id: [u32; 8],
    swap_id: Option<Hex32>,
    depositor: Option<AccountId>,
    wait: Duration,
) -> Result<Found> {
    let started = Instant::now();
    let mut candidates = BTreeSet::new();
    let mut next_block = client
        .get_last_block_id()
        .await
        .context("could not read the sequencer tip")?;
    if let Some(swap_id) = swap_id {
        candidates.insert((
            compute_metadata_pda(&program_id, swap_id.as_bytes()),
            compute_custody_pda(&program_id, swap_id.as_bytes()),
            None,
        ));
    }
    loop {
        if swap_id.is_none()
            && let Some(depositor) = depositor
        {
            let tip = client
                .get_last_block_id()
                .await
                .context("could not read the sequencer tip")?;
            while next_block <= tip {
                if let Some(block) = client
                    .get_block(next_block)
                    .await
                    .context("could not read a block")?
                {
                    for transaction in &block.body.transactions {
                        if let LeeTransaction::Public(public) = transaction
                            && let Some((metadata_id, custody_id)) =
                                escrow_candidate(public.message(), program_id, depositor)
                        {
                            candidates.insert((metadata_id, custody_id, Some(next_block)));
                        }
                    }
                }
                next_block += 1;
            }
        }
        let mut funded = Vec::new();
        for &(metadata_id, custody_id, block) in &candidates {
            let account = account(client, metadata_id).await?;
            if account.program_owner != program_id {
                continue;
            }
            let Ok(metadata) = EscrowMetadata::try_from_slice(account.data.as_ref()) else {
                continue;
            };
            if is_native_escrow(&metadata, program_id, metadata_id, custody_id)
                && metadata.status == EscrowStatus::Funded
            {
                funded.push(Found {
                    metadata_id,
                    custody_id,
                    metadata,
                    by: if swap_id.is_some() {
                        "swap_id"
                    } else {
                        "depositor"
                    },
                    block,
                });
            }
        }
        funded.dedup_by_key(|found| found.metadata_id);
        match funded.len() {
            0 => {}
            1 => return Ok(funded.remove(0)),
            more => bail!("{more} funded escrows name the depositor: name the swap with --swap-id"),
        }
        if started.elapsed() >= wait {
            bail!(
                "no funded native escrow {} within {} s",
                if swap_id.is_some() {
                    "for the swap ID"
                } else {
                    "names the depositor"
                },
                wait.as_secs()
            );
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

async fn snapshot(
    client: &SequencerClient,
    metadata_id: AccountId,
    custody_id: AccountId,
    sender: AccountId,
) -> Result<(Snapshot, EscrowStatus)> {
    let tip = client
        .get_last_block_id()
        .await
        .context("could not read the sequencer tip")?;
    let metadata =
        EscrowMetadata::try_from_slice(account(client, metadata_id).await?.data.as_ref())
            .context("the escrow metadata account is malformed")?;
    let custody = account(client, custody_id).await?;
    let sender = account(client, sender).await?;
    Ok((
        Snapshot {
            sequencer_tip: tip,
            status: format!("{:?}", metadata.status),
            custody_balance: custody.balance,
            sender_balance: sender.balance,
        },
        metadata.status,
    ))
}

async fn account(client: &SequencerClient, account_id: AccountId) -> Result<Account> {
    client
        .get_account(account_id)
        .await
        .context("could not read an account")
}

fn parse_nonzero_hex(value: &str, name: &str) -> Result<Hex32> {
    let value = Hex32::from_hex(value).with_context(|| format!("invalid {name}"))?;
    ensure!(value.as_bytes() != &[0; 32], "{name} must be nonzero");
    Ok(value)
}

fn read_private_key(path: &Path) -> Result<PrivateKey> {
    let metadata = fs::symlink_metadata(path).context("private key file unavailable")?;
    ensure!(metadata.is_file(), "private key must be a regular file");
    ensure!(
        metadata.permissions().mode().trailing_zeros() >= 6,
        "private key file must not be accessible by group or others"
    );
    ensure!(metadata.nlink() == 1, "private key file must have one link");
    let encoded = Zeroizing::new(fs::read(path).context("read private key file")?);
    ensure!(encoded.len() <= 128, "private key file is too large");
    let text = std::str::from_utf8(encoded.as_slice())
        .context("private key file must be UTF-8")?
        .trim();
    ensure!(
        text.len() == 64,
        "private key file must contain exactly 64 hex characters"
    );
    PrivateKey::from_str(text).context("private key file contains an invalid key")
}

#[cfg(test)]
mod tests {
    use super::*;

    const PROGRAM: [u32; 8] = [7; 8];

    fn id(byte: u8) -> AccountId {
        AccountId::new([byte; 32])
    }

    fn message(program: [u32; 8], accounts: Vec<AccountId>) -> Message {
        Message::try_new(
            program,
            accounts,
            vec![],
            AuthenticatedTransferInstruction::Transfer { amount: 1 },
        )
        .expect("encodable test message")
    }

    #[test]
    fn an_escrow_transaction_naming_the_depositor_third_is_a_candidate() {
        let depositor = id(3);
        assert_eq!(
            escrow_candidate(
                &message(PROGRAM, vec![id(1), id(2), depositor]),
                PROGRAM,
                depositor
            ),
            Some((id(1), id(2)))
        );
        assert_eq!(
            escrow_candidate(
                &message(PROGRAM, vec![id(1), id(2), depositor, id(4)]),
                PROGRAM,
                depositor
            ),
            Some((id(1), id(2)))
        );
        assert_eq!(
            escrow_candidate(
                &message([8; 8], vec![id(1), id(2), depositor]),
                PROGRAM,
                depositor
            ),
            None,
            "another program's transaction"
        );
        assert_eq!(
            escrow_candidate(
                &message(PROGRAM, vec![id(1), depositor, id(2)]),
                PROGRAM,
                depositor
            ),
            None,
            "the depositor in another position (a claim's claimant, a refund's depositor)"
        );
        assert_eq!(
            escrow_candidate(&message(PROGRAM, vec![id(1), id(2)]), PROGRAM, depositor),
            None
        );
    }

    #[test]
    fn only_the_exact_pda_pair_of_a_native_escrow_is_accepted() {
        let swap_id = [9; 32];
        let metadata_id = compute_metadata_pda(&PROGRAM, &swap_id);
        let custody_id = compute_custody_pda(&PROGRAM, &swap_id);
        let mut metadata = EscrowMetadata {
            version: 2,
            swap_id,
            terms_hash: [1; 32],
            claim_authority: lez_zec_escrow_v02::ClaimAuthority::Sha256Preimage {
                secret_digest: [2; 32],
            },
            depositor: id(3),
            depositor_asset: id(3),
            claimant: id(4),
            claimant_asset: id(4),
            custody: custody_id,
            asset_program: [5; 8],
            custody_program: [5; 8],
            asset_definition: [0; 32],
            amount: 75,
            refund_at: 1_000,
            status: EscrowStatus::Funded,
        };
        assert!(is_native_escrow(
            &metadata,
            PROGRAM,
            metadata_id,
            custody_id
        ));
        assert!(!is_native_escrow(
            &metadata,
            [6; 8],
            metadata_id,
            custody_id
        ));
        assert!(!is_native_escrow(&metadata, PROGRAM, id(1), custody_id));
        metadata.asset_definition = [1; 32];
        assert!(
            !is_native_escrow(&metadata, PROGRAM, metadata_id, custody_id),
            "a token escrow"
        );
    }

    #[test]
    fn the_credit_landed_only_when_custody_grew_by_the_amount_and_the_escrow_stayed_funded() {
        let funded = EscrowStatus::Funded;
        assert!(credit_landed(75, 76, 1, funded, funded));
        assert!(!credit_landed(75, 75, 1, funded, funded));
        assert!(!credit_landed(75, 77, 1, funded, funded));
        assert!(!credit_landed(75, 0, 1, funded, EscrowStatus::Claimed));
        assert!(!credit_landed(u128::MAX, 0, 1, funded, funded));
    }
}
