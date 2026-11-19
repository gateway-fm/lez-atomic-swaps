#![forbid(unsafe_code)]
//! TEST-ONLY: one checked-escrow `claim_native` with a caller-chosen preimage.
//!
//! The devnet scenario `zec-maker-early-claim` (deploy/scripts/node-e2e.py)
//! runs this inside the Maker's container, with the Maker's claimant key, to
//! show that in ZEC `TakerSellsLez` the Maker cannot claim the Taker's locked
//! LEZ with what it holds (ADR 0216, threat TM-0030). The Node and its sidecar
//! never build a claim whose preimage does not hash to the signed digest
//! (`NativeEscrowPlanner::prepare_revealing_claim` refuses it before signing),
//! so they cannot make this attempt; this tool builds the exact `ClaimNative`
//! message the sidecar would, signs it with the claimant key, submits it to
//! the sequencer and reports what the escrow did with it.
//!
//! It holds no durable state and reserves no nonce. It is not part of any
//! role package: the devnet Maker image carries it beside the package.
//!
//! The JSON evidence on stdout names the escrow state, custody balance and
//! claimant balance before and after, the sequencer's answer to the
//! submission, and whether the transaction was included. Exit status: 0 when
//! the escrow kept the custody (still `Funded`, custody balance unchanged),
//! 3 when the claim paid out, 1 on any other failure.

use std::{
    fs,
    os::unix::fs::{MetadataExt as _, PermissionsExt as _},
    path::{Path, PathBuf},
    str::FromStr as _,
    time::{Duration, Instant},
};

use anyhow::{Context as _, Result, bail, ensure};
use borsh::BorshDeserialize as _;
use clap::Parser;
use common::{HashType, transaction::LeeTransaction};
use jsonrpsee::core::ClientError;
use lez_bridge_protocol::Hex32;
use lez_v0_2_sidecar::{
    ZecEscrowInstruction, compute_custody_pda, compute_metadata_pda, program_id_from_hex,
    validate_loopback_http_endpoint,
};
use lez_zec_escrow_v02::{ClaimAuthority, EscrowMetadata, EscrowStatus};
use nssa::{
    Account, AccountId, PrivateKey, PublicKey, PublicTransaction,
    public_transaction::{Message, WitnessSet},
};
use sequencer_service_rpc::{RpcClient as _, SequencerClient, SequencerClientBuilder};
use serde::Serialize;
use sha2::{Digest as _, Sha256};
use zeroize::Zeroizing;

const EVIDENCE_SCHEMA: &str = "lez_v02_escrow_claim_probe_v1";
const POLL_INTERVAL: Duration = Duration::from_millis(500);
const NODE_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const EXIT_CLAIM_PAID: i32 = 3;

/// Submit one `claim_native` with an arbitrary preimage (TEST-ONLY).
#[derive(Debug, Parser)]
#[command(version, about)]
struct Arguments {
    /// Literal-loopback sequencer HTTP URL and port.
    #[arg(long)]
    sequencer_url: String,
    /// Deployed checked escrow program ID as 64 lowercase hex characters.
    #[arg(long)]
    escrow_program_id: String,
    /// The escrow's on-chain swap ID (the metadata PDA seed), 64 hex characters.
    #[arg(long)]
    swap_id: String,
    /// Owner-only file holding the escrow claimant's lowercase-hex LEZ private key.
    #[arg(long)]
    private_key_file: PathBuf,
    /// The preimage to present, 64 hex characters. Not a secret here: the
    /// scenario passes values that must not open the escrow.
    #[arg(long)]
    preimage_hex: String,
    /// How long to look for the transaction in the sequencer's blocks.
    #[arg(long, default_value_t = 30)]
    observe_seconds: u64,
}

#[derive(Debug, Serialize)]
#[serde(deny_unknown_fields)]
struct EscrowSnapshot {
    sequencer_tip: u64,
    tip_timestamp_ms: u64,
    status: String,
    custody_balance: u128,
    claimant_balance: u128,
    claimant_nonce: u128,
}

#[derive(Debug, Serialize)]
#[serde(deny_unknown_fields)]
struct SubmissionEvidence {
    /// `accepted`: the sequencer returned the transaction's hash;
    /// `rejected`: it answered with a JSON-RPC error;
    /// `transport_error`: no JSON-RPC answer.
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
    claimant_account: Hex32,
    signer_is_claimant: bool,
    stored_secret_digest: Hex32,
    presented_preimage: Hex32,
    presented_preimage_sha256: Hex32,
    presented_preimage_matches_digest: bool,
    amount: u128,
    refund_at_ms: u64,
    submitted_before_refund_at: bool,
    transaction_id: Hex32,
    submission: SubmissionEvidence,
    included: bool,
    included_in_block: Option<u64>,
    observed_seconds: u64,
    before: EscrowSnapshot,
    after: EscrowSnapshot,
    escrow_kept_custody: bool,
}

struct Escrow {
    metadata: EscrowMetadata,
    claimant: Account,
    snapshot: EscrowSnapshot,
}

#[tokio::main]
async fn main() {
    match execute(Arguments::parse()).await {
        Ok(evidence) => {
            let kept = evidence.escrow_kept_custody;
            match serde_json::to_string(&evidence) {
                Ok(json) => println!("{json}"),
                Err(error) => {
                    eprintln!("escrow claim probe evidence encoding failed: {error}");
                    std::process::exit(1);
                }
            }
            if !kept {
                std::process::exit(EXIT_CLAIM_PAID);
            }
        }
        Err(error) => {
            eprintln!("escrow claim probe failed: {error:#}");
            std::process::exit(1);
        }
    }
}

#[allow(
    clippy::too_many_lines,
    reason = "the probe keeps the before read, the claim, the inclusion poll and the after read in one linear flow"
)]
async fn execute(arguments: Arguments) -> Result<ProbeEvidence> {
    validate_loopback_http_endpoint(&arguments.sequencer_url)
        .context("the sequencer URL must be literal-loopback HTTP with a port")?;
    let program_hex = parse_nonzero_hex(&arguments.escrow_program_id, "escrow program ID")?;
    let swap_id = parse_nonzero_hex(&arguments.swap_id, "swap ID")?;
    let preimage = Hex32::from_hex(&arguments.preimage_hex).context("invalid preimage")?;
    let signer_key = read_private_key(&arguments.private_key_file)?;
    let signer = AccountId::from(&PublicKey::new_from_private_key(&signer_key));
    let program_id = program_id_from_hex(program_hex);
    let metadata_id = compute_metadata_pda(&program_id, swap_id.as_bytes());
    let custody_id = compute_custody_pda(&program_id, swap_id.as_bytes());
    let client = SequencerClientBuilder::default()
        .request_timeout(NODE_REQUEST_TIMEOUT)
        .max_concurrent_requests(1)
        .build(&arguments.sequencer_url)
        .context("could not build the sequencer client")?;

    let before = read_escrow(&client, program_id, metadata_id, custody_id, signer).await?;
    let metadata = &before.metadata;
    ensure!(
        metadata.swap_id == *swap_id.as_bytes(),
        "the escrow metadata names another swap ID"
    );
    ensure!(
        metadata.status == EscrowStatus::Funded,
        "the escrow is {:?}, not Funded: there is nothing to claim",
        metadata.status
    );
    let ClaimAuthority::Sha256Preimage { secret_digest } = metadata.claim_authority else {
        bail!("the escrow is not a SHA-256 preimage escrow");
    };
    let signer_is_claimant = metadata.claimant == signer;
    ensure!(
        signer_is_claimant,
        "the key is not the escrow's claimant: the probe tests the claimant's own claim"
    );
    let presented_sha256: [u8; 32] = Sha256::digest(preimage.as_bytes()).into();

    let message = Message::try_new(
        program_id,
        vec![metadata_id, custody_id, signer],
        vec![before.claimant.nonce],
        ZecEscrowInstruction::ClaimNative {
            swap_id: *swap_id.as_bytes(),
            preimage: *preimage.as_bytes(),
        },
    )
    .map_err(|_| anyhow::anyhow!("could not encode the ClaimNative message"))?;
    let witness = WitnessSet::for_message(&message, &[&signer_key]);
    let transaction = PublicTransaction::new(message, witness);
    let transaction_id = transaction.hash();
    let submitted_at_ms = before.snapshot.tip_timestamp_ms;

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
            "the sequencer acknowledged another hash ({}) than the claim's",
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
            .context("could not query the sequencer for the claim")?
        {
            included_in_block = Some(block);
            break;
        }
        if started.elapsed() >= window {
            break;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }

    let after = read_escrow(&client, program_id, metadata_id, custody_id, signer).await?;
    let escrow_kept_custody = after.metadata.status == EscrowStatus::Funded
        && after.snapshot.custody_balance == before.snapshot.custody_balance
        && after.snapshot.custody_balance == metadata.amount;

    Ok(ProbeEvidence {
        schema: EVIDENCE_SCHEMA,
        test_only: true,
        escrow_program_id: program_hex,
        swap_id,
        metadata_account: Hex32::from_bytes(metadata_id.into_value()),
        custody_account: Hex32::from_bytes(custody_id.into_value()),
        claimant_account: Hex32::from_bytes(metadata.claimant.into_value()),
        signer_is_claimant,
        stored_secret_digest: Hex32::from_bytes(secret_digest),
        presented_preimage: preimage,
        presented_preimage_sha256: Hex32::from_bytes(presented_sha256),
        presented_preimage_matches_digest: presented_sha256 == secret_digest,
        amount: metadata.amount,
        refund_at_ms: metadata.refund_at,
        submitted_before_refund_at: submitted_at_ms < metadata.refund_at,
        transaction_id: Hex32::from_bytes(transaction_id),
        submission,
        included: included_in_block.is_some(),
        included_in_block,
        observed_seconds: started.elapsed().as_secs(),
        before: before.snapshot,
        after: after.snapshot,
        escrow_kept_custody,
    })
}

async fn read_escrow(
    client: &SequencerClient,
    program_id: [u32; 8],
    metadata_id: AccountId,
    custody_id: AccountId,
    claimant_id: AccountId,
) -> Result<Escrow> {
    let tip = client
        .get_last_block_id()
        .await
        .context("could not read the sequencer tip")?;
    let tip_block = client
        .get_block(tip)
        .await
        .context("could not read the tip block")?
        .context("the sequencer has no tip block")?;
    let metadata_account = account(client, metadata_id).await?;
    ensure!(
        metadata_account.program_owner == program_id,
        "the metadata account is not owned by the escrow program (no escrow for this swap ID)"
    );
    let metadata = EscrowMetadata::try_from_slice(metadata_account.data.as_ref())
        .context("the escrow metadata account is malformed")?;
    let custody = account(client, custody_id).await?;
    let claimant = account(client, claimant_id).await?;
    let snapshot = EscrowSnapshot {
        sequencer_tip: tip,
        tip_timestamp_ms: tip_block.header.timestamp,
        status: format!("{:?}", metadata.status),
        custody_balance: custody.balance,
        claimant_balance: claimant.balance,
        claimant_nonce: u128::from(claimant.nonce),
    };
    Ok(Escrow {
        metadata,
        claimant,
        snapshot,
    })
}

async fn account(client: &SequencerClient, account_id: AccountId) -> Result<Account> {
    client
        .get_account(account_id)
        .await
        .context("could not read an escrow account")
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
