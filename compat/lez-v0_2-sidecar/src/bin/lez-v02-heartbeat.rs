//! lez-v02-heartbeat — the devnet finality heartbeat (Part C). Every
//! `--interval-seconds` it sends one authenticated transfer of one LEZ unit from
//! its own funded account to a second one (`lez_v0_2_sidecar::heartbeat`),
//! then samples the sequencer tip and the indexer's finalized height and
//! prints one line. When the finalized height has not moved for
//! `--stall-seconds` while the tip has, it declares a stall: one loud line
//! (repeated every five minutes while it lasts, one line when it clears) and
//! `stalled: true` in `--status-file`, which the devnet's runners read instead
//! of waiting out a vault sweep on a dead chain. Devnet tooling: it never
//! touches a swap's accounts and ships in the lez-tools image.

use std::{
    fs,
    os::unix::fs::{MetadataExt as _, PermissionsExt as _},
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context as _, Result, bail, ensure};
use clap::Parser;
use lez_bridge_protocol::Hex32;
use lez_v0_2_sidecar::{
    FinalizedIndexerApi as _, OfficialIndexerRpc, OfficialNodeRpc,
    heartbeat::prepare_unit_transfer, program_id_from_hex,
};
use nssa::{AccountId, PrivateKey, PublicKey};
use serde::Serialize;
use zeroize::Zeroizing;

const STATUS_SCHEMA_VERSION: u16 = 1;
const STALL_REPEAT: Duration = Duration::from_mins(5);
/// The inclusion wait is shorter than any interval, so ticks never pile up; a
/// transaction still pending when it ends is reported as such and the next
/// tick starts from a fresh nonce.
const INCLUSION_WAIT: Duration = Duration::from_secs(15);
const INCLUSION_POLL: Duration = Duration::from_millis(500);

/// Keep a quiet LEZ devnet chain finalizing with one tiny transaction per tick, and say when it stops.
#[derive(Debug, Parser)]
struct Arguments {
    /// Literal-loopback sequencer HTTP endpoint.
    #[arg(long)]
    sequencer_url: String,
    /// Literal-loopback indexer HTTP endpoint.
    #[arg(long)]
    indexer_url: String,
    /// Owner-only file with the sender's LEZ private key (64 hex characters).
    #[arg(long)]
    private_key_file: PathBuf,
    /// The recipient account id (64 hex characters); a funded devnet account, never a role's.
    #[arg(long)]
    recipient_account_id: String,
    /// The authenticated transfer program id (64 hex characters, from market-bootstrap.env).
    #[arg(long)]
    authenticated_transfer_program_id: String,
    /// Seconds between ticks.
    #[arg(long, default_value_t = 20)]
    interval_seconds: u64,
    /// Finalized height flat for this long while the tip advances: a stall.
    #[arg(long, default_value_t = 120)]
    stall_seconds: u64,
    /// Where the status JSON is rewritten every tick (its directory must exist).
    #[arg(long)]
    status_file: PathBuf,
}

/// What the runners read.
#[derive(Debug, Serialize)]
struct Status {
    schema_version: u16,
    tick: u64,
    at_unix: u64,
    tip: Option<u64>,
    finalized: Option<u64>,
    finality_moved_at_unix: u64,
    finality_flat_seconds: u64,
    stalled: bool,
    stalled_since_unix: Option<u64>,
    last_transaction_id: Option<String>,
    last_transaction_included: Option<bool>,
    /// Submit to inclusion, when it was included within the wait.
    last_inclusion_ms: Option<u64>,
    sender_balance: Option<u128>,
}

/// What one tick's transaction came to.
#[derive(Debug, Default)]
struct Sent {
    transaction_id: Option<String>,
    included: Option<bool>,
    inclusion_ms: Option<u64>,
    sender_balance: Option<u128>,
}

struct Clockwork {
    tick: u64,
    last_finalized: Option<u64>,
    finality_moved_at: Instant,
    finality_moved_at_unix: u64,
    tip_when_finality_moved: Option<u64>,
    stalled_since: Option<(Instant, u64)>,
    last_stall_line: Option<Instant>,
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

#[tokio::main]
async fn main() -> Result<()> {
    let arguments = Arguments::parse();
    ensure!(
        (20..=600).contains(&arguments.interval_seconds),
        "interval must be 20..=600 seconds (the inclusion wait is 15 s)"
    );
    ensure!(
        (30..=3600).contains(&arguments.stall_seconds),
        "stall threshold must be 30..=3600 seconds"
    );
    let signer = read_private_key(&arguments.private_key_file)?;
    let sender = AccountId::from(&PublicKey::new_from_private_key(&signer));
    let recipient = AccountId::new(
        *Hex32::from_hex(&arguments.recipient_account_id)
            .context("invalid recipient account id")?
            .as_bytes(),
    );
    ensure!(
        sender != recipient,
        "the recipient must differ from the sender"
    );
    let program = program_id_from_hex(
        Hex32::from_hex(&arguments.authenticated_transfer_program_id)
            .context("invalid authenticated transfer program id")?,
    );
    let node = OfficialNodeRpc::connect_local(&arguments.sequencer_url)
        .context("invalid sequencer endpoint")?;
    let indexer = OfficialIndexerRpc::connect_local(&arguments.indexer_url)
        .context("invalid indexer endpoint")?;
    eprintln!(
        "heartbeat: one unit from {} to {} every {} s; a stall is finality flat for {} s while the tip moves",
        hex::encode(sender.into_value()),
        hex::encode(recipient.into_value()),
        arguments.interval_seconds,
        arguments.stall_seconds
    );
    let interval = Duration::from_secs(arguments.interval_seconds);
    let mut clockwork = Clockwork {
        tick: 0,
        last_finalized: None,
        finality_moved_at: Instant::now(),
        finality_moved_at_unix: unix_now(),
        tip_when_finality_moved: None,
        stalled_since: None,
        last_stall_line: None,
    };
    loop {
        tick(
            &arguments,
            &node,
            &indexer,
            &signer,
            program,
            sender,
            recipient,
            &mut clockwork,
        )
        .await;
        tokio::time::sleep(interval).await;
    }
}

/// One tick: the transaction, the two heights, the stall detector, the line
/// and the status file.
#[allow(clippy::too_many_arguments)]
async fn tick(
    arguments: &Arguments,
    node: &OfficialNodeRpc,
    indexer: &OfficialIndexerRpc,
    signer: &PrivateKey,
    program: [u32; 8],
    sender: AccountId,
    recipient: AccountId,
    clockwork: &mut Clockwork,
) {
    let stall_after = Duration::from_secs(arguments.stall_seconds);
    clockwork.tick += 1;
    let sent = send_one(node, signer, program, sender, recipient).await;
    let tip = node.sequencer_tip().await.ok();
    let finalized = indexer.last_finalized_block_id().await.ok().flatten();
    let now = Instant::now();
    if finalized.is_some() && finalized != clockwork.last_finalized {
        clockwork.last_finalized = finalized;
        clockwork.finality_moved_at = now;
        clockwork.finality_moved_at_unix = unix_now();
        clockwork.tip_when_finality_moved = tip;
        if let Some((since, _)) = clockwork.stalled_since.take() {
            eprintln!(
                "heartbeat: finality moves again at {} after {} s",
                finalized.unwrap_or_default(),
                since.elapsed().as_secs()
            );
            clockwork.last_stall_line = None;
        }
    }
    let flat = now.duration_since(clockwork.finality_moved_at);
    let tip_advanced = match (tip, clockwork.tip_when_finality_moved) {
        (Some(now_tip), Some(then_tip)) => now_tip > then_tip,
        _ => false,
    };
    if flat >= stall_after && tip_advanced {
        if clockwork.stalled_since.is_none() {
            clockwork.stalled_since = Some((now, unix_now()));
        }
        let repeat = clockwork
            .last_stall_line
            .is_none_or(|last| now.duration_since(last) >= STALL_REPEAT);
        if repeat {
            eprintln!(
                "heartbeat: STALL finalized {} flat {} s, tip {} (was {} when finality last moved)",
                clockwork.last_finalized.unwrap_or_default(),
                flat.as_secs(),
                tip.unwrap_or_default(),
                clockwork.tip_when_finality_moved.unwrap_or_default()
            );
            clockwork.last_stall_line = Some(now);
        }
    }
    eprintln!(
        "heartbeat: tick {} tx {} ({}) tip {} finalized {} (flat {} s){}",
        clockwork.tick,
        sent.transaction_id.as_deref().unwrap_or("-"),
        match (sent.included, sent.inclusion_ms) {
            (Some(true), Some(ms)) => format!("included in {ms} ms"),
            (Some(true), None) => "included".to_owned(),
            (Some(false), _) => format!("not included within {} s", INCLUSION_WAIT.as_secs()),
            (None, _) => "not sent".to_owned(),
        },
        tip.map_or_else(|| "-".to_owned(), |t| t.to_string()),
        finalized.map_or_else(|| "-".to_owned(), |f| f.to_string()),
        flat.as_secs(),
        if clockwork.stalled_since.is_some() {
            " STALLED"
        } else {
            ""
        }
    );
    let status = Status {
        schema_version: STATUS_SCHEMA_VERSION,
        tick: clockwork.tick,
        at_unix: unix_now(),
        tip,
        finalized,
        finality_moved_at_unix: clockwork.finality_moved_at_unix,
        finality_flat_seconds: flat.as_secs(),
        stalled: clockwork.stalled_since.is_some(),
        stalled_since_unix: clockwork.stalled_since.map(|(_, unix)| unix),
        last_transaction_id: sent.transaction_id,
        last_transaction_included: sent.included,
        last_inclusion_ms: sent.inclusion_ms,
        sender_balance: sent.sender_balance,
    };
    if let Err(error) = write_status(&arguments.status_file, &status) {
        eprintln!("heartbeat: status file not written: {error:#}");
    }
}

/// One tick's transaction: the sender's nonce and balance from the sequencer,
/// one signed unit transfer, one submission and a bounded wait for inclusion
/// (submit → included is the tick's latency). Nothing retries inside a tick:
/// a rejected submit or a nonce the chain has moved past is one line, and the
/// next tick reads a fresh nonce.
async fn send_one(
    node: &OfficialNodeRpc,
    signer: &PrivateKey,
    program: [u32; 8],
    sender: AccountId,
    recipient: AccountId,
) -> Sent {
    let account = match node.native_account(sender).await {
        Ok(account) => account,
        Err(error) => {
            eprintln!("heartbeat: sender account unavailable: {error}");
            return Sent::default();
        }
    };
    let balance = account.balance;
    let mut sent = Sent {
        sender_balance: Some(balance),
        ..Sent::default()
    };
    if balance == 0 {
        eprintln!("heartbeat: the sender holds no LEZ; sampling without a transaction");
        return sent;
    }
    let prepared = match prepare_unit_transfer(
        signer,
        program,
        sender,
        recipient,
        u128::from(account.nonce),
    ) {
        Ok(prepared) => prepared,
        Err(error) => {
            eprintln!("heartbeat: transfer could not be prepared: {error}");
            return sent;
        }
    };
    sent.transaction_id = Some(hex::encode(prepared.transaction_id.as_bytes()));
    let submitted_at = Instant::now();
    if let Err(error) = node.submit_prepared_transaction(&prepared).await {
        eprintln!(
            "heartbeat: submit refused (nonce {}): {error}; a fresh nonce next tick",
            account.nonce
        );
        sent.included = Some(false);
        return sent;
    }
    sent.included = Some(false);
    while submitted_at.elapsed() < INCLUSION_WAIT {
        match node.prepared_transaction_is_included(&prepared).await {
            Ok(true) => {
                sent.included = Some(true);
                sent.inclusion_ms =
                    Some(u64::try_from(submitted_at.elapsed().as_millis()).unwrap_or(u64::MAX));
                break;
            }
            Ok(false) => {}
            Err(error) => {
                eprintln!("heartbeat: inclusion read failed: {error}");
                break;
            }
        }
        tokio::time::sleep(INCLUSION_POLL).await;
    }
    sent
}

fn write_status(path: &Path, status: &Status) -> Result<()> {
    let encoded = serde_json::to_vec_pretty(status).context("encode status")?;
    let temporary = path.with_extension("json.tmp");
    fs::write(&temporary, encoded).context("write status")?;
    fs::set_permissions(&temporary, fs::Permissions::from_mode(0o644)).context("status mode")?;
    fs::rename(&temporary, path).context("publish status")?;
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
    let mut bytes = Zeroizing::new([0_u8; 32]);
    hex::decode_to_slice(encoded, bytes.as_mut_slice()).context("private key is not hex")?;
    PrivateKey::try_new(*bytes).map_err(|_| anyhow::anyhow!("private key is not valid"))
}
