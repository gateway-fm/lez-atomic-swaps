//! Durable scheduling metadata for opaque maker-owned actor processes.

use std::{
    ffi::OsString,
    fs::{self, File},
    io::{Read as _, Seek as _, Write as _},
    os::fd::AsRawFd as _,
    os::unix::fs::{MetadataExt as _, PermissionsExt as _},
    path::{Component, Path, PathBuf},
    process::Command,
};

use command_fds::{CommandFdExt as _, FdMapping};
use lez_swap_core::{Pair, SwapCoordinator, SwapId};
use rusqlite::{Connection, OptionalExtension as _, Row, TransactionBehavior, params};
use rustix::fs::{
    CWD, FlockOperation, MemfdFlags, Mode, OFlags, ResolveFlags, SealFlags, fcntl_add_seals,
    fcntl_get_seals, flock, memfd_create, openat2,
};
use sha2::{Digest as _, Sha256};
use thiserror::Error;
use zeroize::Zeroizing;

use crate::{SqliteSwapStore, StoreError};

const MANIFEST_VERSION: i64 = 1;
const MAX_PATH_BYTES: usize = 4_096;
const MAX_FAILURE_CLASS_BYTES: usize = 64;
const MAX_DUE_LIMIT: usize = 128;
const MAX_ACTOR_CONFIG_BYTES: u64 = 4 * 1_024 * 1_024;
const MAX_ACTOR_PROGRAM_BYTES: u64 = 512 * 1_024 * 1_024;
/// Fixed child descriptor containing the sealed, verified actor config bytes.
pub const MAKER_ACTOR_CONFIG_FD: i32 = 196;
/// Fixed child descriptor used as the exact actor executable.
pub const MAKER_ACTOR_PROGRAM_FD: i32 = 197;
/// Fixed child descriptor retaining the per-swap kernel lock.
pub const MAKER_ACTOR_LOCK_FD: i32 = 198;

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

/// Non-cloneable proof that this process owns one exact per-swap kernel lock.
pub struct MakerActorHeldLock {
    swap_id: SwapId,
    state_database_path: PathBuf,
    lock_path: PathBuf,
    file: File,
    device: u64,
    inode: u64,
}

impl std::fmt::Debug for MakerActorHeldLock {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MakerActorHeldLock")
            .field("swap_id", &self.swap_id)
            .finish_non_exhaustive()
    }
}

impl MakerActorHeldLock {
    /// Securely creates or opens and non-blockingly locks one deterministic file.
    ///
    /// # Errors
    ///
    /// Rejects an unsafe state parent/lock inode or a still-live lock owner.
    pub fn acquire(record: &MakerActorProcessRecordV1) -> Result<Self, MakerActorProcessError> {
        let state_database_path = record.manifest.state_database_path().to_path_buf();
        let parent = state_database_path
            .parent()
            .ok_or(MakerActorProcessError::UnsafeLock)?;
        validate_lock_root(parent)?;
        let lock_path = lock_file_path(&state_database_path);
        let file = openat2(
            CWD,
            &lock_path,
            OFlags::RDWR | OFlags::CREATE | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::RUSR | Mode::WUSR,
            ResolveFlags::NO_SYMLINKS,
        )
        .map(File::from)
        .map_err(|_| MakerActorProcessError::UnsafeLock)?;
        validate_lock_file(&file, &lock_path)?;
        flock(&file, FlockOperation::NonBlockingLockExclusive)
            .map_err(|_| MakerActorProcessError::LockUnavailable)?;
        validate_lock_root(parent)?;
        validate_lock_file(&file, &lock_path)?;
        let metadata = file
            .metadata()
            .map_err(|_| MakerActorProcessError::UnsafeLock)?;
        Ok(Self {
            swap_id: record.swap_id().clone(),
            state_database_path,
            lock_path,
            file,
            device: metadata.dev(),
            inode: metadata.ino(),
        })
    }

    /// Makes this lock survive `exec` in only the child spawned by `command`.
    ///
    /// The parent descriptor remains close-on-exec. The child clears that flag
    /// after `fork` and before `exec`, avoiding a cross-thread descriptor leak.
    ///
    /// # Errors
    ///
    /// Fails when the held descriptor cannot be duplicated for the command.
    pub fn inherit_into(&self, command: &mut Command) -> Result<(), MakerActorProcessError> {
        command
            .fd_mappings(vec![self.fd_mapping()?])
            .map_err(|_| MakerActorProcessError::LockInheritance)?;
        Ok(())
    }

    fn fd_mapping(&self) -> Result<FdMapping, MakerActorProcessError> {
        let descriptor = self
            .file
            .try_clone()
            .map_err(|_| MakerActorProcessError::LockInheritance)?;
        Ok(FdMapping {
            parent_fd: descriptor.into(),
            child_fd: MAKER_ACTOR_LOCK_FD,
        })
    }

    fn validate_for(
        &self,
        record: &MakerActorProcessRecordV1,
    ) -> Result<(), MakerActorProcessError> {
        validate_lock_file(&self.file, &self.lock_path)?;
        let metadata = self
            .file
            .metadata()
            .map_err(|_| MakerActorProcessError::UnsafeLock)?;
        if &self.swap_id != record.swap_id()
            || self.state_database_path != record.manifest.state_database_path()
            || metadata.dev() != self.device
            || metadata.ino() != self.inode
        {
            return Err(MakerActorProcessError::LockMismatch);
        }
        Ok(())
    }
}

/// Non-cloneable, sealed snapshot of one actor's exact deployment artifacts.
pub struct MakerActorArtifacts {
    record: MakerActorProcessRecordV1,
    config: File,
    program: File,
    state: MakerActorStateBinding,
}

enum MakerActorStateBinding {
    Missing,
    Existing(File),
}

impl std::fmt::Debug for MakerActorArtifacts {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MakerActorArtifacts")
            .field("swap_id", self.record.swap_id())
            .finish_non_exhaustive()
    }
}

impl MakerActorArtifacts {
    /// Secure-opens, hashes, and seals one immutable config/program pair.
    ///
    /// The role-state database is bound as either one exact private inode or an
    /// exact absent path under its owner-private parent. Config and program
    /// bytes are copied into write-sealed anonymous files so later path or
    /// in-place replacement cannot change what the child reads or executes.
    ///
    /// # Errors
    ///
    /// Rejects unsafe metadata, content drift, hash mismatch, or an unsafe
    /// state-database location.
    pub fn open(record: &MakerActorProcessRecordV1) -> Result<Self, MakerActorProcessError> {
        let manifest = record.manifest();
        let config_bytes = read_verified_artifact(
            manifest.config_path(),
            MakerActorArtifactKind::Config,
            MAX_ACTOR_CONFIG_BYTES,
            manifest.config_sha256(),
        )?;
        let program_bytes = read_verified_artifact(
            manifest.program_path(),
            MakerActorArtifactKind::Program,
            MAX_ACTOR_PROGRAM_BYTES,
            manifest.program_sha256(),
        )?;
        let state = bind_actor_state(manifest.state_database_path())?;
        let config = sealed_artifact("lez-maker-actor-config", &config_bytes, 0o600)?;
        let program = sealed_artifact("lez-maker-actor-program", &program_bytes, 0o700)?;
        Ok(Self {
            record: record.clone(),
            config,
            program,
            state,
        })
    }

    /// Consumes the sealed snapshot into one exact child command.
    ///
    /// The executable is addressed only through child FD 197. The verified
    /// config is child FD 196 and the per-swap lock is child FD 198. Callers
    /// add the actor command and `--config-fd 196` arguments before spawning.
    ///
    /// # Errors
    ///
    /// Rejects a changed state location, mismatched lock, or descriptor setup
    /// failure.
    pub fn into_command(
        self,
        held_lock: &MakerActorHeldLock,
    ) -> Result<Command, MakerActorProcessError> {
        held_lock.validate_for(&self.record)?;
        validate_actor_state(self.record.manifest().state_database_path(), &self.state)?;
        let lock_mapping = held_lock.fd_mapping()?;
        let mut command = Command::new(format!("/proc/self/fd/{MAKER_ACTOR_PROGRAM_FD}"));
        command
            .fd_mappings(vec![
                FdMapping {
                    parent_fd: self.program.into(),
                    child_fd: MAKER_ACTOR_PROGRAM_FD,
                },
                FdMapping {
                    parent_fd: self.config.into(),
                    child_fd: MAKER_ACTOR_CONFIG_FD,
                },
                lock_mapping,
            ])
            .map_err(|_| MakerActorProcessError::ArtifactPreparation)?;
        Ok(command)
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
    /// Lock root or deterministic lock inode is unsafe.
    #[error("maker actor process lock is unsafe")]
    UnsafeLock,
    /// Another process or inherited actor child still owns the lock.
    #[error("maker actor process lock is unavailable")]
    LockUnavailable,
    /// Held lock belongs to a different swap.
    #[error("maker actor process lock does not match lease")]
    LockMismatch,
    /// Held descriptor could not be prepared for one exact child.
    #[error("maker actor process lock inheritance failed")]
    LockInheritance,
    /// A config, program, or state-database filesystem binding is unsafe.
    #[error("maker actor deployment artifact is unsafe")]
    UnsafeArtifact,
    /// Config or program bytes differ from the immutable manifest digest.
    #[error("maker actor deployment artifact hash does not match")]
    ArtifactHashMismatch,
    /// Verified artifacts could not be sealed or mapped into one child.
    #[error("maker actor deployment artifacts could not be prepared")]
    ArtifactPreparation,
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

    /// Atomically transfers one abandoned lease while holding its exact kernel lock.
    ///
    /// The non-cloneable `held_lock` can exist only after the old parent and
    /// every child that inherited its descriptor have exited. Time is never an
    /// admission signal, and the row never becomes publicly queued/unleased.
    ///
    /// # Errors
    ///
    /// Fails for a cross-swap lock, stale lease, or durable-store error.
    pub fn recover_abandoned_maker_actor(
        &mut self,
        lease: &MakerActorLeaseV1,
        held_lock: &MakerActorHeldLock,
        new_owner: MakerActorLeaseOwner,
        now: u64,
    ) -> Result<MakerActorLeaseV1, MakerActorProcessError> {
        held_lock.validate_for(&lease.record)?;
        let generation = lease
            .generation
            .checked_add(1)
            .ok_or(MakerActorProcessError::CorruptRecord)?;
        let now = time_to_sql(now);
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let changed = transaction.execute(
            "UPDATE maker_actor_processes SET
                 lease_generation = ?1, lease_owner = ?2, leased_at = ?3,
                 child_pid = NULL,
                 child_start_ticks = NULL, last_failure_class = 'coordinator_restarted',
                 attempt_count = attempt_count + 1, updated_at = ?3
             WHERE swap_id = ?4 AND schedule_state = 'leased'
               AND lease_owner = ?5 AND lease_generation = ?6",
            params![
                generation_to_sql(generation)?,
                new_owner.bytes().as_slice(),
                now,
                lease.record.swap_id().as_str(),
                lease.owner.bytes().as_slice(),
                generation_to_sql(lease.generation)?,
            ],
        )?;
        if changed != 1 {
            return Err(MakerActorProcessError::LeaseConflict);
        }
        let record = load_record(&transaction, lease.record.swap_id())?
            .ok_or(MakerActorProcessError::CorruptRecord)?;
        if record.schedule_state != MakerActorScheduleState::Leased
            || record.lease_owner != Some(new_owner)
            || record.lease_generation != generation
        {
            return Err(MakerActorProcessError::CorruptRecord);
        }
        transaction.commit()?;
        Ok(MakerActorLeaseV1 {
            record,
            owner: new_owner,
            generation,
        })
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

#[derive(Clone, Copy, Eq, PartialEq)]
enum MakerActorArtifactKind {
    Config,
    Program,
    State,
}

fn read_verified_artifact(
    path: &Path,
    kind: MakerActorArtifactKind,
    maximum_bytes: u64,
    expected_sha256: [u8; 32],
) -> Result<Zeroizing<Vec<u8>>, MakerActorProcessError> {
    validate_artifact_parent(path, kind)?;
    let mut file = openat2(
        CWD,
        path,
        OFlags::RDONLY | OFlags::NONBLOCK | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
        ResolveFlags::NO_SYMLINKS,
    )
    .map(File::from)
    .map_err(|_| MakerActorProcessError::UnsafeArtifact)?;
    validate_named_artifact(&file, path, kind, Some(maximum_bytes))?;
    let mut bytes = Zeroizing::new(Vec::new());
    std::io::Read::by_ref(&mut file)
        .take(maximum_bytes.saturating_add(1))
        .read_to_end(bytes.as_mut())
        .map_err(|_| MakerActorProcessError::UnsafeArtifact)?;
    if bytes.is_empty() || bytes.len() as u64 > maximum_bytes {
        return Err(MakerActorProcessError::UnsafeArtifact);
    }
    validate_artifact_parent(path, kind)?;
    validate_named_artifact(&file, path, kind, Some(maximum_bytes))?;
    let actual_sha256: [u8; 32] = Sha256::digest(bytes.as_slice()).into();
    if actual_sha256 != expected_sha256 {
        return Err(MakerActorProcessError::ArtifactHashMismatch);
    }
    Ok(bytes)
}

fn validate_artifact_parent(
    path: &Path,
    kind: MakerActorArtifactKind,
) -> Result<(), MakerActorProcessError> {
    let parent = path
        .parent()
        .ok_or(MakerActorProcessError::UnsafeArtifact)?;
    match kind {
        MakerActorArtifactKind::Config | MakerActorArtifactKind::State => {
            validate_lock_root(parent).map_err(|_| MakerActorProcessError::UnsafeArtifact)
        }
        MakerActorArtifactKind::Program => validate_program_parent(parent),
    }
}

fn validate_program_parent(parent: &Path) -> Result<(), MakerActorProcessError> {
    if !parent.is_absolute() {
        return Err(MakerActorProcessError::UnsafeArtifact);
    }
    let before =
        fs::symlink_metadata(parent).map_err(|_| MakerActorProcessError::UnsafeArtifact)?;
    let effective_uid = rustix::process::geteuid().as_raw();
    if !before.file_type().is_dir()
        || (before.uid() != 0 && before.uid() != effective_uid)
        || before.permissions().mode() & 0o022 != 0
        || fs::canonicalize(parent).map_err(|_| MakerActorProcessError::UnsafeArtifact)? != parent
    {
        return Err(MakerActorProcessError::UnsafeArtifact);
    }
    let after = fs::symlink_metadata(parent).map_err(|_| MakerActorProcessError::UnsafeArtifact)?;
    if !same_artifact(&before, &after) {
        return Err(MakerActorProcessError::UnsafeArtifact);
    }
    Ok(())
}

fn validate_named_artifact(
    file: &File,
    path: &Path,
    kind: MakerActorArtifactKind,
    maximum_bytes: Option<u64>,
) -> Result<(), MakerActorProcessError> {
    let opened = file
        .metadata()
        .map_err(|_| MakerActorProcessError::UnsafeArtifact)?;
    let named = fs::symlink_metadata(path).map_err(|_| MakerActorProcessError::UnsafeArtifact)?;
    let mode = opened.permissions().mode() & 0o7777;
    let effective_uid = rustix::process::geteuid().as_raw();
    let valid_kind = match kind {
        MakerActorArtifactKind::Config | MakerActorArtifactKind::State => {
            opened.uid() == effective_uid && mode == 0o600
        }
        MakerActorArtifactKind::Program => {
            let owner_is_trusted = opened.uid() == 0 || opened.uid() == effective_uid;
            let executable = if opened.uid() == effective_uid {
                mode & 0o100 != 0
            } else {
                mode & 0o001 != 0
            };
            owner_is_trusted && mode & 0o022 == 0 && executable
        }
    };
    if !opened.file_type().is_file()
        || !named.file_type().is_file()
        || opened.nlink() != 1
        || !valid_kind
        || maximum_bytes.is_some_and(|maximum| opened.len() == 0 || opened.len() > maximum)
        || !same_artifact(&opened, &named)
    {
        return Err(MakerActorProcessError::UnsafeArtifact);
    }
    Ok(())
}

fn bind_actor_state(path: &Path) -> Result<MakerActorStateBinding, MakerActorProcessError> {
    validate_artifact_parent(path, MakerActorArtifactKind::State)?;
    match openat2(
        CWD,
        path,
        OFlags::RDWR | OFlags::NONBLOCK | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
        ResolveFlags::NO_SYMLINKS,
    ) {
        Ok(descriptor) => {
            let file = File::from(descriptor);
            validate_named_artifact(&file, path, MakerActorArtifactKind::State, None)?;
            validate_artifact_parent(path, MakerActorArtifactKind::State)?;
            Ok(MakerActorStateBinding::Existing(file))
        }
        Err(error) if error == rustix::io::Errno::NOENT => {
            match fs::symlink_metadata(path) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Ok(_) | Err(_) => return Err(MakerActorProcessError::UnsafeArtifact),
            }
            validate_artifact_parent(path, MakerActorArtifactKind::State)?;
            Ok(MakerActorStateBinding::Missing)
        }
        Err(_) => Err(MakerActorProcessError::UnsafeArtifact),
    }
}

fn validate_actor_state(
    path: &Path,
    binding: &MakerActorStateBinding,
) -> Result<(), MakerActorProcessError> {
    validate_artifact_parent(path, MakerActorArtifactKind::State)?;
    match binding {
        MakerActorStateBinding::Existing(file) => {
            validate_named_artifact(file, path, MakerActorArtifactKind::State, None)?;
        }
        MakerActorStateBinding::Missing => match fs::symlink_metadata(path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Ok(_) | Err(_) => return Err(MakerActorProcessError::UnsafeArtifact),
        },
    }
    validate_artifact_parent(path, MakerActorArtifactKind::State)
}

fn sealed_artifact(name: &str, bytes: &[u8], mode: u32) -> Result<File, MakerActorProcessError> {
    let descriptor = memfd_create(name, MemfdFlags::CLOEXEC | MemfdFlags::ALLOW_SEALING)
        .map_err(|_| MakerActorProcessError::ArtifactPreparation)?;
    let mut writer = File::from(descriptor);
    writer
        .write_all(bytes)
        .and_then(|()| writer.flush())
        .map_err(|_| MakerActorProcessError::ArtifactPreparation)?;
    writer
        .set_permissions(fs::Permissions::from_mode(mode))
        .map_err(|_| MakerActorProcessError::ArtifactPreparation)?;
    let seals = SealFlags::SEAL | SealFlags::SHRINK | SealFlags::GROW | SealFlags::WRITE;
    fcntl_add_seals(&writer, seals).map_err(|_| MakerActorProcessError::ArtifactPreparation)?;
    if !fcntl_get_seals(&writer)
        .map_err(|_| MakerActorProcessError::ArtifactPreparation)?
        .contains(seals)
    {
        return Err(MakerActorProcessError::ArtifactPreparation);
    }
    writer
        .seek(std::io::SeekFrom::Start(0))
        .map_err(|_| MakerActorProcessError::ArtifactPreparation)?;
    let descriptor_path = format!("/proc/self/fd/{}", writer.as_raw_fd());
    let reader =
        File::open(descriptor_path).map_err(|_| MakerActorProcessError::ArtifactPreparation)?;
    let metadata = reader
        .metadata()
        .map_err(|_| MakerActorProcessError::ArtifactPreparation)?;
    if metadata.len() != bytes.len() as u64 || metadata.permissions().mode() & 0o7777 != mode {
        return Err(MakerActorProcessError::ArtifactPreparation);
    }
    drop(writer);
    Ok(reader)
}

fn same_artifact(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    left.dev() == right.dev()
        && left.ino() == right.ino()
        && left.uid() == right.uid()
        && left.mode() == right.mode()
        && left.nlink() == right.nlink()
        && left.len() == right.len()
        && left.mtime() == right.mtime()
        && left.mtime_nsec() == right.mtime_nsec()
        && left.ctime() == right.ctime()
        && left.ctime_nsec() == right.ctime_nsec()
}

fn lock_file_path(state_database_path: &Path) -> PathBuf {
    let mut value = OsString::from(state_database_path.as_os_str());
    value.push(".maker-actor.lock");
    PathBuf::from(value)
}

fn validate_lock_root(root: &Path) -> Result<(), MakerActorProcessError> {
    if !root.is_absolute() {
        return Err(MakerActorProcessError::UnsafeLock);
    }
    let before = fs::symlink_metadata(root).map_err(|_| MakerActorProcessError::UnsafeLock)?;
    let effective_uid = rustix::process::geteuid().as_raw();
    if !before.file_type().is_dir()
        || before.uid() != effective_uid
        || before.permissions().mode() & 0o7777 != 0o700
        || fs::canonicalize(root).map_err(|_| MakerActorProcessError::UnsafeLock)? != root
    {
        return Err(MakerActorProcessError::UnsafeLock);
    }
    let after = fs::symlink_metadata(root).map_err(|_| MakerActorProcessError::UnsafeLock)?;
    if !same_inode(&before, &after) {
        return Err(MakerActorProcessError::UnsafeLock);
    }
    Ok(())
}

fn validate_lock_file(file: &File, path: &Path) -> Result<(), MakerActorProcessError> {
    let opened = file
        .metadata()
        .map_err(|_| MakerActorProcessError::UnsafeLock)?;
    let named = fs::symlink_metadata(path).map_err(|_| MakerActorProcessError::UnsafeLock)?;
    if !opened.file_type().is_file()
        || !named.file_type().is_file()
        || opened.uid() != rustix::process::geteuid().as_raw()
        || opened.permissions().mode() & 0o7777 != 0o600
        || opened.nlink() != 1
        || !same_inode(&opened, &named)
    {
        return Err(MakerActorProcessError::UnsafeLock);
    }
    Ok(())
}

fn same_inode(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    left.dev() == right.dev()
        && left.ino() == right.ino()
        && left.uid() == right.uid()
        && left.mode() == right.mode()
        && left.nlink() == right.nlink()
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
