//! Secret-free inspection of durable Maker actor scheduler records.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use clap::Parser as _;
use lez_swap_store::{
    MakerActorKindV1, MakerActorProcessRecordV1, MakerActorScheduleState, SqliteSwapStore,
};
use serde::Serialize;

#[derive(clap::Parser)]
#[command(about = "Inspect secret-free durable Maker actor scheduler records")]
struct Arguments {
    /// Existing owner-private Maker application database.
    #[arg(long)]
    database: PathBuf,
}

#[derive(Serialize)]
struct ChildIdentity {
    pid: u32,
    start_ticks: u64,
}

#[derive(Serialize)]
struct ActorRecord {
    schema_version: u16,
    swap_id: Box<str>,
    actor_kind: &'static str,
    config_path: Box<str>,
    config_sha256: Box<str>,
    actor_program_path: Box<str>,
    actor_program_sha256: Box<str>,
    state_db_path: Box<str>,
    schedule_state: &'static str,
    lease_generation: u64,
    attempt_count: u64,
    child_identity: Option<ChildIdentity>,
    child_identity_absent: bool,
}

fn main() -> Result<()> {
    let arguments = Arguments::parse();
    let store = SqliteSwapStore::open(&arguments.database).context("open Maker database")?;
    let records = store
        .list_maker_actor_processes()
        .context("list Maker actor records")?
        .iter()
        .map(project)
        .collect::<Result<Vec<_>>>()?;
    println!("{}", serde_json::to_string(&records)?);
    Ok(())
}

fn project(record: &MakerActorProcessRecordV1) -> Result<ActorRecord> {
    let manifest = record.manifest();
    let child_identity = record
        .child_identity()
        .map(|(pid, start_ticks)| ChildIdentity { pid, start_ticks });
    Ok(ActorRecord {
        schema_version: 1,
        swap_id: record.swap_id().as_str().into(),
        actor_kind: match manifest.kind() {
            MakerActorKindV1::Bitcoin => "bitcoin",
            MakerActorKindV1::Monero => "monero",
            MakerActorKindV1::Zcash => "zcash",
        },
        config_path: exact_path(manifest.config_path())?,
        config_sha256: hex::encode(manifest.config_sha256()).into(),
        actor_program_path: exact_path(manifest.program_path())?,
        actor_program_sha256: hex::encode(manifest.program_sha256()).into(),
        state_db_path: exact_path(manifest.state_database_path())?,
        schedule_state: match record.schedule_state() {
            MakerActorScheduleState::Queued => "queued",
            MakerActorScheduleState::Leased => "leased",
            MakerActorScheduleState::Backoff => "backoff",
            MakerActorScheduleState::Terminal => "terminal",
            MakerActorScheduleState::Failed => "failed",
        },
        lease_generation: record.lease_generation(),
        attempt_count: record.attempt_count(),
        child_identity_absent: child_identity.is_none(),
        child_identity,
    })
}

fn exact_path(path: &Path) -> Result<Box<str>> {
    path.to_str()
        .context("actor manifest path is not UTF-8")
        .map(Into::into)
}
