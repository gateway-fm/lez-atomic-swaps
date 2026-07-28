//! Durable scheduling metadata for opaque maker-owned actor processes.

use std::path::{Component, Path, PathBuf};

use lez_swap_core::{Pair, SwapCoordinator, SwapId};
use rusqlite::{Connection, OptionalExtension as _, Row, TransactionBehavior, params};
use thiserror::Error;

use crate::{SqliteSwapStore, StoreError};

const MANIFEST_VERSION: i64 = 1;
const MAX_PATH_BYTES: usize = 4_096;
const MAX_FAILURE_CLASS_BYTES: usize = 64;
const MAX_DUE_LIMIT: usize = 128;

/// Pair adapter executable used by one maker process record.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MakerActorKindV1 {
    /// One-shot Bitcoin reference actor.
    Bitcoin,
    /// One-shot Zcash reference actor.
    Zcash,
}

impl MakerActorKindV1 {
    const fn name(self) -> &'static str {
        match self {
            Self::Bitcoin => "bitcoin",
            Self::Zcash => "zcash",
        }
    }

    const fn pair(self) -> Pair {
        match self {
            Self::Bitcoin => Pair::Bitcoin,
            Self::Zcash => Pair::Zcash,
        }
    }

    fn parse(value: &str) -> Result<Self, MakerActorProcessError> {
        match value {
            "bitcoin" => Ok(Self::Bitcoin),
            "zcash" => Ok(Self::Zcash),
            _ => Err(MakerActorProcessError::CorruptRecord),
        }
    }
}

/// Durable process scheduler state; protocol phase remains in the actor DB.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MakerActorScheduleState {
    /// Ready when `next_attempt_at` is reached.
    Queued,
    /// Owned by one generation-fenced daemon worker.
    Leased,
    /// Retryable failure waiting for `next_attempt_at`.
    Backoff,
    /// Actor reported an absorbing protocol state.
    Terminal,
    /// Operator action is required before another attempt.
    Failed,
}

impl MakerActorScheduleState {
    const fn name(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Leased => "leased",
            Self::Backoff => "backoff",
            Self::Terminal => "terminal",
            Self::Failed => "failed",
        }
    }

    fn parse(value: &str) -> Result<Self, MakerActorProcessError> {
        match value {
            "queued" => Ok(Self::Queued),
            "leased" => Ok(Self::Leased),
            "backoff" => Ok(Self::Backoff),
            "terminal" => Ok(Self::Terminal),
            "failed" => Ok(Self::Failed),
            _ => Err(MakerActorProcessError::CorruptRecord),
        }
    }
}

/// Immutable, secret-free binding between one accepted swap and one actor.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MakerActorManifestV1 {
    swap_id: SwapId,
    kind: MakerActorKindV1,
    config_path: PathBuf,
    config_sha256: [u8; 32],
    program_path: PathBuf,
    program_sha256: [u8; 32],
    state_database_path: PathBuf,
}

impl MakerActorManifestV1 {
    /// Constructs one normalized absolute immutable actor binding.
    ///
    /// # Errors
    ///
    /// Rejects relative, non-normalized, empty, oversized, or lexically aliased paths.
    pub fn new(
        swap_id: SwapId,
        kind: MakerActorKindV1,
        config_path: PathBuf,
        config_sha256: [u8; 32],
        program_path: PathBuf,
        program_sha256: [u8; 32],
        state_database_path: PathBuf,
    ) -> Result<Self, MakerActorProcessError> {
        validate_path(&config_path)?;
        validate_path(&program_path)?;
        validate_path(&state_database_path)?;
        if config_path == program_path
            || config_path == state_database_path
            || program_path == state_database_path
        {
            return Err(MakerActorProcessError::InvalidManifest);
        }
        Ok(Self {
            swap_id,
            kind,
            config_path,
            config_sha256,
            program_path,
            program_sha256,
            state_database_path,
        })
    }

    /// Stable application swap identity.
    #[must_use]
    pub const fn swap_id(&self) -> &SwapId {
        &self.swap_id
    }

    /// Exact pair actor kind.
    #[must_use]
    pub const fn kind(&self) -> MakerActorKindV1 {
        self.kind
    }

    /// Owner-private immutable actor config path.
    #[must_use]
    pub fn config_path(&self) -> &Path {
        &self.config_path
    }

    /// Exact config content identity.
    #[must_use]
    pub const fn config_sha256(&self) -> [u8; 32] {
        self.config_sha256
    }

    /// Pinned one-shot actor executable path.
    #[must_use]
    pub fn program_path(&self) -> &Path {
        &self.program_path
    }

    /// Exact executable content identity.
    #[must_use]
    pub const fn program_sha256(&self) -> [u8; 32] {
        self.program_sha256
    }

    /// Role-local authoritative actor database path.
    #[must_use]
    pub fn state_database_path(&self) -> &Path {
        &self.state_database_path
    }
}

/// Random process-coordinator identity used in fenced lease updates.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MakerActorLeaseOwner([u8; 16]);

impl MakerActorLeaseOwner {
    /// Constructs a nonzero owner identity.
    ///
    /// # Errors
    ///
    /// Rejects the reserved all-zero identity.
    pub fn new(value: [u8; 16]) -> Result<Self, MakerActorProcessError> {
        if value == [0; 16] {
            Err(MakerActorProcessError::InvalidLeaseOwner)
        } else {
            Ok(Self(value))
        }
    }

    const fn bytes(self) -> [u8; 16] {
        self.0
    }
}

/// Secret-free durable scheduler record.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MakerActorProcessRecordV1 {
    manifest: MakerActorManifestV1,
    schedule_state: MakerActorScheduleState,
    next_attempt_at: u64,
    lease_generation: u64,
    lease_owner: Option<MakerActorLeaseOwner>,
    leased_at: Option<u64>,
    child_identity: Option<(u32, u64)>,
    attempt_count: u64,
    last_failure_class: Option<Box<str>>,
    created_at: u64,
    updated_at: u64,
}

impl MakerActorProcessRecordV1 {
    /// Stable application swap ID.
    #[must_use]
    pub const fn swap_id(&self) -> &SwapId {
        self.manifest.swap_id()
    }

    /// Immutable actor manifest.
    #[must_use]
    pub const fn manifest(&self) -> &MakerActorManifestV1 {
        &self.manifest
    }

    /// Current process scheduling state.
    #[must_use]
    pub const fn schedule_state(&self) -> MakerActorScheduleState {
        self.schedule_state
    }

    /// Number of attempted worker generations.
    #[must_use]
    pub const fn attempt_count(&self) -> u64 {
        self.attempt_count
    }

    /// Current generation fence.
    #[must_use]
    pub const fn lease_generation(&self) -> u64 {
        self.lease_generation
    }

    /// Exact child PID/start-ticks diagnostic binding, when spawned.
    #[must_use]
    pub const fn child_identity(&self) -> Option<(u32, u64)> {
        self.child_identity
    }
}

/// Claimed scheduler row, fenced by owner and monotonically increasing generation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MakerActorLeaseV1 {
    record: MakerActorProcessRecordV1,
    owner: MakerActorLeaseOwner,
    generation: u64,
}

impl MakerActorLeaseV1 {
    /// Claimed durable record.
    #[must_use]
    pub const fn record(&self) -> &MakerActorProcessRecordV1 {
        &self.record
    }

    /// Monotonic fencing generation.
    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    /// Returns a deliberately forged value for fence-negative tests.
    #[doc(hidden)]
    #[must_use]
    pub fn with_owner(&self, owner: MakerActorLeaseOwner) -> Self {
        Self {
            record: self.record.clone(),
            owner,
            generation: self.generation,
        }
    }
}

/// Resolution of one bounded actor-process attempt.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MakerActorAttemptResolution {
    /// Immediately or eventually queue another exact attempt.
    Requeue {
        /// Half-open due timestamp.
        not_before: u64,
    },
    /// Retry after a classified dependency/process failure.
    Backoff {
        /// Half-open due timestamp.
        not_before: u64,
        /// Stable payload-free failure class.
        failure_class: Box<str>,
    },
    /// Actor reported an absorbing state.
    Terminal,
    /// Disable automatic retry pending operator action.
    Failed {
        /// Stable payload-free failure class.
        failure_class: Box<str>,
    },
}

/// Insert/replay result for one immutable actor manifest.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MakerActorRegistrationCommit {
    was_replay: bool,
}

impl MakerActorRegistrationCommit {
    /// Whether the exact immutable row was already present.
    #[must_use]
    pub const fn was_replay(self) -> bool {
        self.was_replay
    }
}

/// Stable process-scheduling failure.
#[derive(Debug, Error)]
pub enum MakerActorProcessError {
    /// Manifest paths or immutable fields are invalid.
    #[error("maker actor manifest is invalid")]
    InvalidManifest,
    /// The all-zero process owner is reserved.
    #[error("maker actor lease owner is invalid")]
    InvalidLeaseOwner,
    /// Referenced application swap is absent.
    #[error("maker actor application swap does not exist")]
    MissingSwap,
    /// Actor kind disagrees with the application aggregate pair.
    #[error("maker actor pair does not match application swap")]
    PairMismatch,
    /// Existing immutable registration differs.
    #[error("maker actor registration conflicts with durable state")]
    RegistrationConflict,
    /// Lease owner or generation is stale.
    #[error("maker actor lease conflicts with durable owner")]
    LeaseConflict,
    /// Timestamp, child identity, limit, or failure class is invalid.
    #[error("maker actor scheduling input is invalid")]
    InvalidSchedulingInput,
    /// Durable columns violate the process-record contract.
    #[error("maker actor scheduling record is corrupt")]
    CorruptRecord,
    /// Underlying swap store failed.
    #[error("maker actor scheduling store failed")]
    Store(#[from] StoreError),
    /// Scheduler SQL operation failed.
    #[error("maker actor scheduling operation failed")]
    Sqlite(#[from] rusqlite::Error),
}

impl SqliteSwapStore {
    /// Inserts one immutable maker actor or exact-replays its registration.
    ///
    /// # Errors
    ///
    /// Fails for an absent or mismatched swap, changed manifest, or store error.
    pub fn register_maker_actor(
        &mut self,
        manifest: &MakerActorManifestV1,
        now: u64,
    ) -> Result<MakerActorRegistrationCommit, MakerActorProcessError> {
        let now = time_to_sql(now);
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let swap_json = transaction
            .query_row(
                "SELECT state_json FROM swaps WHERE id = ?1",
                [manifest.swap_id().as_str()],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .ok_or(MakerActorProcessError::MissingSwap)?;
        let swap: SwapCoordinator = serde_json::from_str(&swap_json).map_err(StoreError::from)?;
        if swap.pair() != manifest.kind().pair() {
            return Err(MakerActorProcessError::PairMismatch);
        }
        let existing = load_record(&transaction, manifest.swap_id())?;
        if let Some(existing) = existing {
            if existing.manifest == *manifest {
                transaction.commit()?;
                return Ok(MakerActorRegistrationCommit { was_replay: true });
            }
            return Err(MakerActorProcessError::RegistrationConflict);
        }
        transaction.execute(
            "INSERT INTO maker_actor_processes (
                swap_id, actor_kind, manifest_version, manifest_path, manifest_sha256,
                actor_program_path, actor_program_sha256, state_db_path,
                desired_state, schedule_state, next_attempt_at, lease_generation,
                attempt_count, created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8,
                       'running', 'queued', ?9, 0, 0, ?9, ?9)",
            params![
                manifest.swap_id().as_str(),
                manifest.kind().name(),
                MANIFEST_VERSION,
                path_string(manifest.config_path())?,
                manifest.config_sha256().as_slice(),
                path_string(manifest.program_path())?,
                manifest.program_sha256().as_slice(),
                path_string(manifest.state_database_path())?,
                now,
            ],
        )?;
        transaction.commit()?;
        Ok(MakerActorRegistrationCommit { was_replay: false })
    }

    /// Lists due, unleased maker actors in stable order.
    ///
    /// # Errors
    ///
    /// Fails for an invalid limit or corrupt store state.
    pub fn list_due_maker_actor_ids(
        &self,
        now: u64,
        limit: usize,
    ) -> Result<Vec<SwapId>, MakerActorProcessError> {
        if !(1..=MAX_DUE_LIMIT).contains(&limit) {
            return Err(MakerActorProcessError::InvalidSchedulingInput);
        }
        let limit =
            i64::try_from(limit).map_err(|_| MakerActorProcessError::InvalidSchedulingInput)?;
        let mut statement = self.connection.prepare(
            "SELECT swap_id FROM maker_actor_processes
             WHERE desired_state = 'running'
               AND schedule_state IN ('queued', 'backoff')
               AND next_attempt_at <= ?1
             ORDER BY next_attempt_at, swap_id LIMIT ?2",
        )?;
        let values = statement
            .query_map(params![time_to_sql(now), limit], |row| {
                row.get::<_, String>(0)
            })?
            .collect::<Result<Vec<_>, _>>()?;
        values
            .into_iter()
            .map(|value| SwapId::new(value).map_err(|_| MakerActorProcessError::CorruptRecord))
            .collect()
    }

    /// Claims one due actor with an owner-and-generation fenced lease.
    ///
    /// # Errors
    ///
    /// Fails when the transaction or resulting durable record is invalid.
    pub fn claim_maker_actor(
        &mut self,
        swap_id: &SwapId,
        owner: MakerActorLeaseOwner,
        now: u64,
    ) -> Result<Option<MakerActorLeaseV1>, MakerActorProcessError> {
        let now = time_to_sql(now);
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let changed = transaction.execute(
            "UPDATE maker_actor_processes SET
                 schedule_state = 'leased', lease_generation = lease_generation + 1,
                 lease_owner = ?1, leased_at = ?2, child_pid = NULL,
                 child_start_ticks = NULL, attempt_count = attempt_count + 1,
                 updated_at = ?2
             WHERE swap_id = ?3 AND desired_state = 'running'
               AND schedule_state IN ('queued', 'backoff') AND next_attempt_at <= ?2",
            params![owner.bytes().as_slice(), now, swap_id.as_str()],
        )?;
        if changed == 0 {
            transaction.commit()?;
            return Ok(None);
        }
        let record =
            load_record(&transaction, swap_id)?.ok_or(MakerActorProcessError::CorruptRecord)?;
        let generation = record.lease_generation;
        if record.lease_owner != Some(owner)
            || record.schedule_state != MakerActorScheduleState::Leased
        {
            return Err(MakerActorProcessError::CorruptRecord);
        }
        transaction.commit()?;
        Ok(Some(MakerActorLeaseV1 {
            record,
            owner,
            generation,
        }))
    }

    /// Fences one spawned child identity into its active lease.
    ///
    /// # Errors
    ///
    /// Fails for an invalid child identity, stale lease, or store error.
    pub fn record_maker_actor_child(
        &mut self,
        lease: &MakerActorLeaseV1,
        pid: u32,
        start_ticks: u64,
    ) -> Result<(), MakerActorProcessError> {
        if pid == 0 || start_ticks == 0 {
            return Err(MakerActorProcessError::InvalidSchedulingInput);
        }
        let changed = self.connection.execute(
            "UPDATE maker_actor_processes SET child_pid = ?1, child_start_ticks = ?2
             WHERE swap_id = ?3 AND schedule_state = 'leased'
               AND lease_owner = ?4 AND lease_generation = ?5
               AND child_pid IS NULL AND child_start_ticks IS NULL",
            params![
                i64::from(pid),
                time_to_sql(start_ticks),
                lease.record.swap_id().as_str(),
                lease.owner.bytes().as_slice(),
                generation_to_sql(lease.generation)?,
            ],
        )?;
        if changed == 1 {
            Ok(())
        } else {
            Err(MakerActorProcessError::LeaseConflict)
        }
    }

    /// Resolves one attempt only for the exact durable lease generation.
    ///
    /// # Errors
    ///
    /// Fails for invalid resolution data, a stale lease, or store error.
    pub fn resolve_maker_actor_attempt(
        &mut self,
        lease: &MakerActorLeaseV1,
        resolution: MakerActorAttemptResolution,
        now: u64,
    ) -> Result<(), MakerActorProcessError> {
        let (state, next, failure) = resolution_fields(resolution)?;
        let changed = self.connection.execute(
            "UPDATE maker_actor_processes SET
                 schedule_state = ?1, next_attempt_at = ?2, lease_owner = NULL,
                 leased_at = NULL, child_pid = NULL, child_start_ticks = NULL,
                 last_failure_class = ?3, updated_at = ?4
             WHERE swap_id = ?5 AND schedule_state = 'leased'
               AND lease_owner = ?6 AND lease_generation = ?7",
            params![
                state.name(),
                time_to_sql(next),
                failure.as_deref(),
                time_to_sql(now),
                lease.record.swap_id().as_str(),
                lease.owner.bytes().as_slice(),
                generation_to_sql(lease.generation)?,
            ],
        )?;
        if changed == 1 {
            Ok(())
        } else {
            Err(MakerActorProcessError::LeaseConflict)
        }
    }

    /// Lists every process record in stable swap-ID order.
    ///
    /// # Errors
    ///
    /// Fails when a durable row is unavailable, malformed, or unsupported.
    pub fn list_maker_actor_processes(
        &self,
    ) -> Result<Vec<MakerActorProcessRecordV1>, MakerActorProcessError> {
        list_records(&self.connection, None)
    }

    /// Lists generation-fenced leases for crash recovery; no lease is expired by time.
    ///
    /// # Errors
    ///
    /// Fails when a leased row lacks a valid owner or cannot be read.
    pub fn list_leased_maker_actors(
        &self,
    ) -> Result<Vec<MakerActorLeaseV1>, MakerActorProcessError> {
        list_records(&self.connection, Some(MakerActorScheduleState::Leased))?
            .into_iter()
            .map(|record| {
                let owner = record
                    .lease_owner
                    .ok_or(MakerActorProcessError::CorruptRecord)?;
                let generation = record.lease_generation;
                Ok(MakerActorLeaseV1 {
                    record,
                    owner,
                    generation,
                })
            })
            .collect()
    }
}

fn resolution_fields(
    resolution: MakerActorAttemptResolution,
) -> Result<(MakerActorScheduleState, u64, Option<Box<str>>), MakerActorProcessError> {
    match resolution {
        MakerActorAttemptResolution::Requeue { not_before } => {
            Ok((MakerActorScheduleState::Queued, not_before, None))
        }
        MakerActorAttemptResolution::Backoff {
            not_before,
            failure_class,
        } => {
            validate_failure_class(&failure_class)?;
            Ok((
                MakerActorScheduleState::Backoff,
                not_before,
                Some(failure_class),
            ))
        }
        MakerActorAttemptResolution::Terminal => Ok((MakerActorScheduleState::Terminal, 0, None)),
        MakerActorAttemptResolution::Failed { failure_class } => {
            validate_failure_class(&failure_class)?;
            Ok((MakerActorScheduleState::Failed, 0, Some(failure_class)))
        }
    }
}

fn validate_failure_class(value: &str) -> Result<(), MakerActorProcessError> {
    if value.is_empty()
        || value.len() > MAX_FAILURE_CLASS_BYTES
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
    {
        Err(MakerActorProcessError::InvalidSchedulingInput)
    } else {
        Ok(())
    }
}

fn validate_path(path: &Path) -> Result<(), MakerActorProcessError> {
    let Some(raw) = path.to_str() else {
        return Err(MakerActorProcessError::InvalidManifest);
    };
    if !path.is_absolute() || raw.len() > MAX_PATH_BYTES || raw.is_empty() {
        return Err(MakerActorProcessError::InvalidManifest);
    }
    if path
        .components()
        .any(|component| !matches!(component, Component::RootDir | Component::Normal(_)))
    {
        return Err(MakerActorProcessError::InvalidManifest);
    }
    Ok(())
}

fn path_string(path: &Path) -> Result<&str, MakerActorProcessError> {
    path.to_str().ok_or(MakerActorProcessError::InvalidManifest)
}

fn time_to_sql(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

fn generation_to_sql(value: u64) -> Result<i64, MakerActorProcessError> {
    i64::try_from(value).map_err(|_| MakerActorProcessError::CorruptRecord)
}

fn decode_u64(value: i64) -> Result<u64, MakerActorProcessError> {
    u64::try_from(value).map_err(|_| MakerActorProcessError::CorruptRecord)
}

fn decode_owner(
    value: Option<Vec<u8>>,
) -> Result<Option<MakerActorLeaseOwner>, MakerActorProcessError> {
    value
        .map(|bytes| {
            let bytes: [u8; 16] = bytes
                .try_into()
                .map_err(|_| MakerActorProcessError::CorruptRecord)?;
            MakerActorLeaseOwner::new(bytes).map_err(|_| MakerActorProcessError::CorruptRecord)
        })
        .transpose()
}

type RawRecord = (
    String,
    String,
    i64,
    String,
    Vec<u8>,
    String,
    Vec<u8>,
    String,
    String,
    i64,
    i64,
    Option<Vec<u8>>,
    Option<i64>,
    Option<i64>,
    Option<i64>,
    i64,
    Option<String>,
    i64,
    i64,
);

fn read_raw_record(row: &Row<'_>) -> rusqlite::Result<RawRecord> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
        row.get(7)?,
        row.get(8)?,
        row.get(9)?,
        row.get(10)?,
        row.get(11)?,
        row.get(12)?,
        row.get(13)?,
        row.get(14)?,
        row.get(15)?,
        row.get(16)?,
        row.get(17)?,
        row.get(18)?,
    ))
}

fn decode_record(raw: RawRecord) -> Result<MakerActorProcessRecordV1, MakerActorProcessError> {
    let (
        swap_id,
        kind,
        manifest_version,
        config_path,
        config_hash,
        program_path,
        program_hash,
        state_path,
        schedule,
        next,
        generation,
        owner,
        leased_at,
        child_pid,
        child_ticks,
        attempts,
        failure,
        created,
        updated,
    ) = raw;
    if manifest_version != MANIFEST_VERSION {
        return Err(MakerActorProcessError::CorruptRecord);
    }
    let config_hash: [u8; 32] = config_hash
        .try_into()
        .map_err(|_| MakerActorProcessError::CorruptRecord)?;
    let program_hash: [u8; 32] = program_hash
        .try_into()
        .map_err(|_| MakerActorProcessError::CorruptRecord)?;
    let manifest = MakerActorManifestV1::new(
        SwapId::new(swap_id).map_err(|_| MakerActorProcessError::CorruptRecord)?,
        MakerActorKindV1::parse(&kind)?,
        PathBuf::from(config_path),
        config_hash,
        PathBuf::from(program_path),
        program_hash,
        PathBuf::from(state_path),
    )?;
    let schedule_state = MakerActorScheduleState::parse(&schedule)?;
    let lease_owner = decode_owner(owner)?;
    let leased_at = leased_at.map(decode_u64).transpose()?;
    let child_identity = match (child_pid, child_ticks) {
        (None, None) => None,
        (Some(pid), Some(ticks)) => Some((
            u32::try_from(pid).map_err(|_| MakerActorProcessError::CorruptRecord)?,
            decode_u64(ticks)?,
        )),
        _ => return Err(MakerActorProcessError::CorruptRecord),
    };
    if (schedule_state == MakerActorScheduleState::Leased)
        != (lease_owner.is_some() && leased_at.is_some())
        || (schedule_state != MakerActorScheduleState::Leased && child_identity.is_some())
    {
        return Err(MakerActorProcessError::CorruptRecord);
    }
    Ok(MakerActorProcessRecordV1 {
        manifest,
        schedule_state,
        next_attempt_at: decode_u64(next)?,
        lease_generation: decode_u64(generation)?,
        lease_owner,
        leased_at,
        child_identity,
        attempt_count: decode_u64(attempts)?,
        last_failure_class: failure.map(String::into_boxed_str),
        created_at: decode_u64(created)?,
        updated_at: decode_u64(updated)?,
    })
}

const RECORD_COLUMNS: &str = "swap_id, actor_kind, manifest_version, manifest_path,
    manifest_sha256, actor_program_path, actor_program_sha256, state_db_path,
    schedule_state, next_attempt_at, lease_generation, lease_owner, leased_at,
    child_pid, child_start_ticks, attempt_count, last_failure_class, created_at, updated_at";

fn load_record(
    connection: &Connection,
    swap_id: &SwapId,
) -> Result<Option<MakerActorProcessRecordV1>, MakerActorProcessError> {
    let sql = format!("SELECT {RECORD_COLUMNS} FROM maker_actor_processes WHERE swap_id = ?1");
    connection
        .query_row(&sql, [swap_id.as_str()], read_raw_record)
        .optional()?
        .map(decode_record)
        .transpose()
}

fn list_records(
    connection: &Connection,
    state: Option<MakerActorScheduleState>,
) -> Result<Vec<MakerActorProcessRecordV1>, MakerActorProcessError> {
    let (sql, parameter) = match state {
        Some(state) => (
            format!(
                "SELECT {RECORD_COLUMNS} FROM maker_actor_processes WHERE schedule_state = ?1 ORDER BY swap_id"
            ),
            Some(state.name()),
        ),
        None => (
            format!("SELECT {RECORD_COLUMNS} FROM maker_actor_processes ORDER BY swap_id"),
            None,
        ),
    };
    let mut statement = connection.prepare(&sql)?;
    let rows = match parameter {
        Some(value) => statement
            .query_map([value], read_raw_record)?
            .collect::<Result<Vec<_>, _>>()?,
        None => statement
            .query_map([], read_raw_record)?
            .collect::<Result<Vec<_>, _>>()?,
    };
    rows.into_iter().map(decode_record).collect()
}

pub(super) fn migrate(transaction: &rusqlite::Transaction<'_>) -> Result<(), StoreError> {
    transaction.execute_batch(
        "CREATE TABLE IF NOT EXISTS maker_actor_processes (
            swap_id              TEXT PRIMARY KEY NOT NULL REFERENCES swaps(id) ON DELETE CASCADE,
            actor_kind           TEXT NOT NULL CHECK (actor_kind IN ('bitcoin', 'zcash')),
            manifest_version     INTEGER NOT NULL CHECK (manifest_version = 1),
            manifest_path        TEXT NOT NULL UNIQUE,
            manifest_sha256      BLOB NOT NULL CHECK (length(manifest_sha256) = 32),
            actor_program_path   TEXT NOT NULL,
            actor_program_sha256 BLOB NOT NULL CHECK (length(actor_program_sha256) = 32),
            state_db_path        TEXT NOT NULL UNIQUE,
            desired_state        TEXT NOT NULL CHECK (desired_state IN ('running', 'stopped')),
            schedule_state       TEXT NOT NULL CHECK (
                schedule_state IN ('queued', 'leased', 'backoff', 'terminal', 'failed')
            ),
            next_attempt_at      INTEGER NOT NULL CHECK (next_attempt_at >= 0),
            lease_generation     INTEGER NOT NULL DEFAULT 0 CHECK (lease_generation >= 0),
            lease_owner          BLOB CHECK (lease_owner IS NULL OR length(lease_owner) = 16),
            leased_at            INTEGER CHECK (leased_at IS NULL OR leased_at >= 0),
            child_pid            INTEGER CHECK (child_pid IS NULL OR child_pid > 0),
            child_start_ticks    INTEGER CHECK (child_start_ticks IS NULL OR child_start_ticks > 0),
            attempt_count        INTEGER NOT NULL DEFAULT 0 CHECK (attempt_count >= 0),
            last_failure_class   TEXT,
            created_at           INTEGER NOT NULL CHECK (created_at >= 0),
            updated_at           INTEGER NOT NULL CHECK (updated_at >= created_at),
            CHECK ((child_pid IS NULL) = (child_start_ticks IS NULL)),
            CHECK (
                (schedule_state = 'leased' AND lease_owner IS NOT NULL AND leased_at IS NOT NULL)
                OR (schedule_state != 'leased' AND lease_owner IS NULL AND leased_at IS NULL
                    AND child_pid IS NULL AND child_start_ticks IS NULL)
            )
        ) STRICT;
        CREATE INDEX IF NOT EXISTS maker_actor_processes_due
            ON maker_actor_processes (desired_state, schedule_state, next_attempt_at, swap_id);",
    )?;
    Ok(())
}
