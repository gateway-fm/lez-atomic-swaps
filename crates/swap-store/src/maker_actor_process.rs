//! Durable scheduling metadata for opaque maker-owned actor processes.

use std::{
    collections::BTreeSet,
    ffi::OsString,
    fs::{self, File},
    io::{Read as _, Seek as _, Write as _},
    os::fd::AsRawFd as _,
    os::unix::fs::{MetadataExt as _, PermissionsExt as _},
    path::{Component, Path, PathBuf},
    process::Command,
};

use command_fds::{CommandFdExt as _, FdMapping};
use lez_bridge_protocol::RequestId;
use lez_swap_core::{Pair, SwapCoordinator, SwapId};
use rusqlite::{Connection, OptionalExtension as _, Row, Transaction, TransactionBehavior, params};
use rustix::fs::{
    FlockOperation, MemfdFlags, Mode, OFlags, SealFlags, fcntl_add_seals, fcntl_get_seals, flock,
    memfd_create,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use thiserror::Error;
use zeroize::Zeroizing;

use crate::{SqliteSwapStore, StoreError, is_owner_private_regular_file, open_no_symlinks};

const MANIFEST_VERSION: i64 = 1;
const MAX_PATH_BYTES: usize = 4_096;
const MAX_FAILURE_CLASS_BYTES: usize = 64;
const MAX_PROGRESS_LABEL_BYTES: usize = 64;
const MAX_DUE_LIMIT: usize = 128;
const MAX_ACTOR_CONFIG_BYTES: u64 = 4 * 1_024 * 1_024;
const MAX_ACTOR_PROGRAM_BYTES: u64 = 512 * 1_024 * 1_024;
/// Standard-input descriptor used only to transfer a safe owned duplicate of
/// the supervisor lock into a nested semantic Maker effect process.
pub const MAKER_ACTOR_LOCK_TRANSFER_FD: i32 = 0;
/// Fixed child descriptor containing the sealed, verified actor config bytes.
pub const MAKER_ACTOR_CONFIG_FD: i32 = 196;
/// Fixed child descriptor used as the exact actor executable.
pub const MAKER_ACTOR_PROGRAM_FD: i32 = 197;
/// Fixed child descriptor retaining the per-swap kernel lock.
pub const MAKER_ACTOR_LOCK_FD: i32 = 198;
/// Fixed child descriptor for one generic hash-pinned sealed executable.
pub const PINNED_EXECUTABLE_FD: i32 = MAKER_ACTOR_PROGRAM_FD;
/// Fixed child descriptor retaining a pinned executable workflow lock.
pub const PINNED_EXECUTABLE_WORKFLOW_LOCK_FD: i32 = 199;
/// Lowest child descriptor available to a pinned executable input plan.
pub const PINNED_EXECUTABLE_INPUT_FD_MIN: i32 = 200;
/// Highest child descriptor accepted from a bounded pinned input plan.
pub const PINNED_EXECUTABLE_INPUT_FD_MAX: i32 = 1_023;
const MAX_PINNED_EXECUTABLE_INPUTS: usize = 64;

/// Pair adapter executable used by one maker process record.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum MakerActorKindV1 {
    /// One-shot Bitcoin reference actor.
    Bitcoin,
    /// One-shot Monero reference actor.
    Monero,
    /// One-shot Zcash reference actor.
    Zcash,
}

impl MakerActorKindV1 {
    const fn name(self) -> &'static str {
        match self {
            Self::Bitcoin => "bitcoin",
            Self::Monero => "monero",
            Self::Zcash => "zcash",
        }
    }

    const fn pair(self) -> Pair {
        match self {
            Self::Bitcoin => Pair::Bitcoin,
            Self::Monero => Pair::Monero,
            Self::Zcash => Pair::Zcash,
        }
    }

    pub(crate) fn parse(value: &str) -> Result<Self, MakerActorProcessError> {
        match value {
            "bitcoin" => Ok(Self::Bitcoin),
            "monero" => Ok(Self::Monero),
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
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
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

    /// Generates a nonzero owner identity from the operating system CSPRNG.
    ///
    /// The reserved all-zero value is discarded and regenerated. Callers should
    /// create one identity per coordinator process lifetime and reuse it for all
    /// leases owned by that process.
    ///
    /// # Errors
    ///
    /// Returns [`MakerActorProcessError::LeaseOwnerEntropy`] when the operating
    /// system random source is unavailable.
    pub fn random() -> Result<Self, MakerActorProcessError> {
        loop {
            let mut value = [0_u8; 16];
            getrandom::fill(&mut value).map_err(|_| MakerActorProcessError::LeaseOwnerEntropy)?;
            if value != [0; 16] {
                return Ok(Self(value));
            }
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
        Self::acquire_for(record.swap_id(), record.manifest.state_database_path())
    }

    /// Securely creates or opens and non-blockingly locks one role-state path.
    ///
    /// This is the same per-swap kernel authority used by the Maker scheduler,
    /// exposed for a synchronous role-fixed CLI that has no scheduler record.
    ///
    /// # Errors
    ///
    /// Rejects an unsafe state parent/lock inode or a still-live lock owner.
    pub fn acquire_for(
        swap_id: &SwapId,
        state_database_path: &Path,
    ) -> Result<Self, MakerActorProcessError> {
        let state_database_path = state_database_path.to_path_buf();
        let parent = state_database_path
            .parent()
            .ok_or(MakerActorProcessError::UnsafeLock)?;
        validate_lock_root(parent)?;
        let lock_path = lock_file_path(&state_database_path);
        let file = open_no_symlinks(
            &lock_path,
            OFlags::RDWR | OFlags::CREATE,
            Mode::RUSR | Mode::WUSR,
        )
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
            swap_id: swap_id.clone(),
            state_database_path,
            lock_path,
            file,
            device: metadata.dev(),
            inode: metadata.ino(),
        })
    }

    /// Accepts an owned duplicate of the supervisor lock transferred on stdin.
    ///
    /// The caller obtains the `File` through safe standard-input descriptor
    /// ownership. This method revalidates the deterministic named inode and
    /// asserts the exclusive `flock` on that same open-file description before
    /// returning authority that can be passed to a nested semantic worker.
    ///
    /// # Errors
    ///
    /// Rejects unsafe lock paths or inodes and a descriptor that cannot hold
    /// the exact exclusive lock.
    pub fn accept_transferred_for(
        swap_id: &SwapId,
        state_database_path: &Path,
        file: File,
    ) -> Result<Self, MakerActorProcessError> {
        let state_database_path = state_database_path.to_path_buf();
        let parent = state_database_path
            .parent()
            .ok_or(MakerActorProcessError::UnsafeLock)?;
        validate_lock_root(parent)?;
        let lock_path = lock_file_path(&state_database_path);
        validate_lock_file(&file, &lock_path)?;
        flock(&file, FlockOperation::NonBlockingLockExclusive)
            .map_err(|_| MakerActorProcessError::LockUnavailable)?;
        validate_lock_root(parent)?;
        validate_lock_file(&file, &lock_path)?;
        let metadata = file
            .metadata()
            .map_err(|_| MakerActorProcessError::UnsafeLock)?;
        Ok(Self {
            swap_id: swap_id.clone(),
            state_database_path,
            lock_path,
            file,
            device: metadata.dev(),
            inode: metadata.ino(),
        })
    }

    /// Revalidates that this live lock guards one exact swap/state path.
    ///
    /// # Errors
    ///
    /// Rejects a changed lock identity or a lock acquired for another swap or
    /// state path.
    pub fn validate_for_state(
        &self,
        swap_id: &SwapId,
        state_database_path: &Path,
    ) -> Result<(), MakerActorProcessError> {
        self.validate_identity()?;
        if &self.swap_id != swap_id || self.state_database_path != state_database_path {
            return Err(MakerActorProcessError::LockMismatch);
        }
        Ok(())
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
        self.fd_mapping_to(MAKER_ACTOR_LOCK_FD)
    }

    fn fd_mapping_to(&self, child_fd: i32) -> Result<FdMapping, MakerActorProcessError> {
        if !matches!(
            child_fd,
            MAKER_ACTOR_LOCK_TRANSFER_FD | MAKER_ACTOR_LOCK_FD | PINNED_EXECUTABLE_WORKFLOW_LOCK_FD
        ) {
            return Err(MakerActorProcessError::InvalidDescriptorMapping);
        }
        let descriptor = self
            .file
            .try_clone()
            .map_err(|_| MakerActorProcessError::LockInheritance)?;
        Ok(FdMapping {
            parent_fd: descriptor.into(),
            child_fd,
        })
    }

    fn validate_identity(&self) -> Result<(), MakerActorProcessError> {
        let parent = self
            .lock_path
            .parent()
            .ok_or(MakerActorProcessError::UnsafeLock)?;
        validate_lock_root(parent)?;
        validate_lock_file(&self.file, &self.lock_path)?;
        let metadata = self
            .file
            .metadata()
            .map_err(|_| MakerActorProcessError::UnsafeLock)?;
        if metadata.dev() != self.device || metadata.ino() != self.inode {
            return Err(MakerActorProcessError::LockMismatch);
        }
        validate_lock_root(parent)?;
        validate_lock_file(&self.file, &self.lock_path)
    }

    fn aliases(&self, other: &Self) -> bool {
        self.device == other.device && self.inode == other.inode
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

/// Immutable sealed snapshot of one securely opened hash-pinned executable.
#[must_use]
pub struct PinnedExecutable {
    program: File,
}

/// Owned, non-cloneable descriptors to install beside one pinned executable.
///
/// The plan contains no argument or environment values. Each descriptor is
/// moved into one complete child mapping and is dropped on any validation
/// failure.
#[must_use]
pub struct PinnedChildFdPlan {
    descriptors: Vec<(File, i32)>,
}

impl std::fmt::Debug for PinnedChildFdPlan {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PinnedChildFdPlan")
            .field("descriptor_count", &self.descriptors.len())
            .finish_non_exhaustive()
    }
}

impl PinnedChildFdPlan {
    /// Validates one bounded set of owned auxiliary child descriptors.
    ///
    /// # Errors
    ///
    /// Rejects an empty or oversized plan, a descriptor outside 200..=1023,
    /// duplicate child targets, or aliased source descriptors.
    pub fn new(descriptors: Vec<(File, i32)>) -> Result<Self, MakerActorProcessError> {
        if descriptors.is_empty() || descriptors.len() > MAX_PINNED_EXECUTABLE_INPUTS {
            return Err(MakerActorProcessError::InvalidDescriptorMapping);
        }
        let mut child_fds = BTreeSet::new();
        let mut identities = BTreeSet::new();
        for (descriptor, child_fd) in &descriptors {
            let metadata = descriptor
                .metadata()
                .map_err(|_| MakerActorProcessError::ArtifactPreparation)?;
            if !(PINNED_EXECUTABLE_INPUT_FD_MIN..=PINNED_EXECUTABLE_INPUT_FD_MAX).contains(child_fd)
                || !child_fds.insert(*child_fd)
                || !identities.insert((metadata.dev(), metadata.ino()))
            {
                return Err(MakerActorProcessError::InvalidDescriptorMapping);
            }
        }
        Ok(Self { descriptors })
    }
}

impl std::fmt::Debug for PinnedExecutable {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PinnedExecutable")
            .finish_non_exhaustive()
    }
}

impl PinnedExecutable {
    /// Secure-opens, hashes, and seals one executable for later exact execution.
    ///
    /// # Errors
    ///
    /// Rejects an unsafe parent, type, owner, mode, link count, size, identity,
    /// or SHA-256 mismatch. The returned command never reopens the named path.
    pub fn open(path: &Path, expected_sha256: [u8; 32]) -> Result<Self, MakerActorProcessError> {
        let bytes = read_verified_artifact(
            path,
            MakerActorArtifactKind::Program,
            MAX_ACTOR_PROGRAM_BYTES,
            expected_sha256,
        )?;
        Ok(Self {
            program: sealed_artifact("lez-pinned-executable", &bytes, 0o700)?,
        })
    }

    /// Consumes the sealed snapshot into an exact descriptor-addressed command.
    ///
    /// # Errors
    ///
    /// Fails if the child descriptor mapping cannot be installed.
    pub fn into_command(self) -> Result<Command, MakerActorProcessError> {
        let mut command = Command::new(format!("/proc/self/fd/{PINNED_EXECUTABLE_FD}"));
        command
            .fd_mappings(vec![FdMapping {
                parent_fd: self.program.into(),
                child_fd: PINNED_EXECUTABLE_FD,
            }])
            .map_err(|_| MakerActorProcessError::ArtifactPreparation)?;
        Ok(command)
    }

    /// Consumes the sealed executable into a command holding two exact locks.
    ///
    /// The actor/state lock is inherited as child FD 198 and the distinct
    /// workflow lock as child FD 199. The executable and both locks are
    /// installed together so no later descriptor-mapping call can replace an
    /// earlier mapping.
    ///
    /// # Errors
    ///
    /// Rejects changed or aliased locks, descriptor collisions, and mapping
    /// failures before a child can be spawned.
    pub fn into_command_with_locks(
        self,
        actor_lock: &MakerActorHeldLock,
        workflow_lock: &MakerActorHeldLock,
    ) -> Result<Command, MakerActorProcessError> {
        actor_lock.validate_identity()?;
        workflow_lock.validate_identity()?;
        if actor_lock.swap_id != workflow_lock.swap_id
            || actor_lock.aliases(workflow_lock)
            || PINNED_EXECUTABLE_FD == MAKER_ACTOR_LOCK_FD
            || PINNED_EXECUTABLE_FD == PINNED_EXECUTABLE_WORKFLOW_LOCK_FD
            || MAKER_ACTOR_LOCK_FD == PINNED_EXECUTABLE_WORKFLOW_LOCK_FD
        {
            return Err(MakerActorProcessError::InvalidDescriptorMapping);
        }
        let actor_mapping = actor_lock.fd_mapping_to(MAKER_ACTOR_LOCK_FD)?;
        let workflow_mapping = workflow_lock.fd_mapping_to(PINNED_EXECUTABLE_WORKFLOW_LOCK_FD)?;
        let mut command = Command::new(format!("/proc/self/fd/{PINNED_EXECUTABLE_FD}"));
        command
            .fd_mappings(vec![
                FdMapping {
                    parent_fd: self.program.into(),
                    child_fd: PINNED_EXECUTABLE_FD,
                },
                actor_mapping,
                workflow_mapping,
            ])
            .map_err(|_| MakerActorProcessError::ArtifactPreparation)?;
        Ok(command)
    }

    /// Consumes one executable, two locks, and auxiliary inputs into one map.
    ///
    /// This is the only composition boundary for descriptor-addressed effect
    /// children. Program FD 197, actor lock FD 198, workflow lock FD 199, and
    /// every plan descriptor are installed by one `fd_mappings` call so a
    /// later call cannot replace an earlier custody mapping.
    ///
    /// # Errors
    ///
    /// Rejects changed, crossed, or aliased locks; aliased program/input
    /// descriptors; invalid child targets; and mapping failures before spawn.
    pub fn into_command_with_locks_and_fd_plan(
        self,
        actor_lock: &MakerActorHeldLock,
        workflow_lock: &MakerActorHeldLock,
        plan: PinnedChildFdPlan,
    ) -> Result<Command, MakerActorProcessError> {
        actor_lock.validate_identity()?;
        workflow_lock.validate_identity()?;
        if actor_lock.swap_id != workflow_lock.swap_id || actor_lock.aliases(workflow_lock) {
            return Err(MakerActorProcessError::InvalidDescriptorMapping);
        }

        let program_metadata = self
            .program
            .metadata()
            .map_err(|_| MakerActorProcessError::ArtifactPreparation)?;
        let mut identities = BTreeSet::from([
            (program_metadata.dev(), program_metadata.ino()),
            (actor_lock.device, actor_lock.inode),
            (workflow_lock.device, workflow_lock.inode),
        ]);
        if identities.len() != 3 {
            return Err(MakerActorProcessError::InvalidDescriptorMapping);
        }
        for (descriptor, _) in &plan.descriptors {
            let metadata = descriptor
                .metadata()
                .map_err(|_| MakerActorProcessError::ArtifactPreparation)?;
            if !identities.insert((metadata.dev(), metadata.ino())) {
                return Err(MakerActorProcessError::InvalidDescriptorMapping);
            }
        }

        let actor_mapping = actor_lock.fd_mapping_to(MAKER_ACTOR_LOCK_FD)?;
        let workflow_mapping = workflow_lock.fd_mapping_to(PINNED_EXECUTABLE_WORKFLOW_LOCK_FD)?;
        let mut mappings = Vec::with_capacity(3 + plan.descriptors.len());
        mappings.push(FdMapping {
            parent_fd: self.program.into(),
            child_fd: PINNED_EXECUTABLE_FD,
        });
        mappings.push(actor_mapping);
        mappings.push(workflow_mapping);
        mappings.extend(
            plan.descriptors
                .into_iter()
                .map(|(descriptor, child_fd)| FdMapping {
                    parent_fd: descriptor.into(),
                    child_fd,
                }),
        );
        let mut command = Command::new(format!("/proc/self/fd/{PINNED_EXECUTABLE_FD}"));
        command
            .fd_mappings(mappings)
            .map_err(|_| MakerActorProcessError::ArtifactPreparation)?;
        Ok(command)
    }
}

/// Validates one exact actor executable against the scheduler artifact policy.
///
/// # Errors
///
/// Rejects an unsafe parent, type, owner, mode, link count, size, identity, or
/// SHA-256 mismatch. This lets a daemon fail before accepting work.
pub fn validate_maker_actor_program(
    path: &Path,
    expected_sha256: [u8; 32],
) -> Result<(), MakerActorProcessError> {
    read_verified_artifact(
        path,
        MakerActorArtifactKind::Program,
        MAX_ACTOR_PROGRAM_BYTES,
        expected_sha256,
    )
    .map(drop)
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
        Self::open_validated(record, |_| Ok(()))
    }

    /// Secure-opens one deployment and validates the exact config snapshot.
    ///
    /// The callback receives the same hash-verified bytes that are subsequently
    /// copied into sealed child FD 196. It must perform only pair-specific,
    /// secret-free semantic validation and return `Err(())` on mismatch.
    ///
    /// # Errors
    ///
    /// Returns [`MakerActorProcessError::ArtifactSemanticMismatch`] when the
    /// exact config bytes do not match their pair-specific manifest semantics,
    /// in addition to the failures documented by [`Self::open`].
    pub fn open_validated(
        record: &MakerActorProcessRecordV1,
        validate_config: impl FnOnce(&[u8]) -> Result<(), ()>,
    ) -> Result<Self, MakerActorProcessError> {
        let manifest = record.manifest();
        let config_bytes = read_verified_artifact(
            manifest.config_path(),
            MakerActorArtifactKind::Config,
            MAX_ACTOR_CONFIG_BYTES,
            manifest.config_sha256(),
        )?;
        validate_config(config_bytes.as_slice())
            .map_err(|()| MakerActorProcessError::ArtifactSemanticMismatch)?;
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

    /// Consumes the sealed snapshot into an effect child command that receives
    /// the same actor lock on FD 198 and as a safe standard-input transfer.
    ///
    /// The role actor may safely clone stdin into an owned `File`, validate it,
    /// and pass that same open-file-description lock to a nested semantic worker.
    ///
    /// # Errors
    ///
    /// Rejects a changed state location, mismatched lock, or descriptor setup
    /// failure.
    pub fn into_effect_command(
        self,
        held_lock: &MakerActorHeldLock,
    ) -> Result<Command, MakerActorProcessError> {
        held_lock.validate_for(&self.record)?;
        validate_actor_state(self.record.manifest().state_database_path(), &self.state)?;
        let lock_mapping = held_lock.fd_mapping()?;
        let transfer_mapping = held_lock.fd_mapping_to(MAKER_ACTOR_LOCK_TRANSFER_FD)?;
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
                transfer_mapping,
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
    /// The explicit leased action reached its intended absorbing state.
    ManualActionCompleted,
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

/// Explicit owner-requested effect class for one maker actor.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MakerActorManualAction {
    /// Execute only the pair actor's ordered claim state machine.
    Claim,
    /// Execute only the pair actor's ordered timeout-recovery state machine.
    Refund,
}

impl MakerActorManualAction {
    const fn name(self) -> &'static str {
        match self {
            Self::Claim => "claim",
            Self::Refund => "refund",
        }
    }

    fn parse(value: &str) -> Result<Self, MakerActorProcessError> {
        match value {
            "claim" => Ok(Self::Claim),
            "refund" => Ok(Self::Refund),
            _ => Err(MakerActorProcessError::CorruptRecord),
        }
    }
}

/// Durable lifecycle of one replay-safe manual action request.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MakerActorManualActionState {
    /// Waiting for the next exact actor lease.
    Queued,
    /// Attached to one owner-and-generation fenced actor lease.
    Leased,
    /// The explicit action reached its intended terminal state.
    Completed,
    /// The action failed closed and requires a new operator decision.
    Failed,
}

impl MakerActorManualActionState {
    const fn name(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Leased => "leased",
            Self::Completed => "completed",
            Self::Failed => "failed",
        }
    }

    fn parse(value: &str) -> Result<Self, MakerActorProcessError> {
        match value {
            "queued" => Ok(Self::Queued),
            "leased" => Ok(Self::Leased),
            "completed" => Ok(Self::Completed),
            "failed" => Ok(Self::Failed),
            _ => Err(MakerActorProcessError::CorruptRecord),
        }
    }
}

/// Secret-free durable snapshot of the latest manual action for one actor.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MakerActorManualActionSnapshot {
    request_id: RequestId,
    swap_id: SwapId,
    action: MakerActorManualAction,
    state: MakerActorManualActionState,
    requested_after_generation: u64,
    lease_owner: Option<MakerActorLeaseOwner>,
    lease_generation: Option<u64>,
}

impl MakerActorManualActionSnapshot {
    /// Stable idempotency identity supplied by the operator.
    pub const fn request_id(&self) -> &RequestId {
        &self.request_id
    }
    /// Application swap targeted by this action.
    #[must_use]
    pub const fn swap_id(&self) -> &SwapId {
        &self.swap_id
    }
    /// Explicit effect class; never a generic lifecycle drive.
    #[must_use]
    pub const fn action(&self) -> MakerActorManualAction {
        self.action
    }
    /// Current durable request state.
    #[must_use]
    pub const fn state(&self) -> MakerActorManualActionState {
        self.state
    }
    /// Actor generation observed when the owner queued this request.
    #[must_use]
    pub const fn requested_after_generation(&self) -> u64 {
        self.requested_after_generation
    }
    /// Exact execution generation while leased.
    #[must_use]
    pub const fn lease_generation(&self) -> Option<u64> {
        self.lease_generation
    }
}

/// Immutable enqueue result for exact request replay.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MakerActorManualActionCommit {
    requested_after_generation: u64,
    was_replay: bool,
}

impl MakerActorManualActionCommit {
    /// Generation against which the original request was admitted.
    #[must_use]
    pub const fn requested_after_generation(self) -> u64 {
        self.requested_after_generation
    }
    /// Whether this exact request and payload were already durable.
    #[must_use]
    pub const fn was_replay(self) -> bool {
        self.was_replay
    }
}

/// Validated secret-free output of one role-fixed actor observation.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum MakerActorProgressObservationV1 {
    /// No durable actor activation exists yet.
    NotActivated,
    /// Durable lifecycle state projected by the actor.
    Active {
        /// Stable pair-protocol phase name.
        phase: Box<str>,
        /// Monotonic role-local actor revision.
        revision: u64,
        /// Stable next-action name selected by the pair actor.
        next_action: Box<str>,
    },
}

impl MakerActorProgressObservationV1 {
    /// Constructs one bounded active observation.
    ///
    /// # Errors
    ///
    /// Rejects labels outside bounded lowercase snake-case ASCII.
    pub fn active(
        phase: impl Into<Box<str>>,
        revision: u64,
        next_action: impl Into<Box<str>>,
    ) -> Result<Self, MakerActorProcessError> {
        let phase = phase.into();
        let next_action = next_action.into();
        if !valid_progress_label(&phase) || !valid_progress_label(&next_action) {
            return Err(MakerActorProcessError::InvalidSchedulingInput);
        }
        Ok(Self::Active {
            phase,
            revision,
            next_action,
        })
    }

    fn validate(&self) -> Result<(), MakerActorProcessError> {
        match self {
            Self::NotActivated => Ok(()),
            Self::Active {
                phase, next_action, ..
            } if valid_progress_label(phase) && valid_progress_label(next_action) => Ok(()),
            Self::Active { .. } => Err(MakerActorProcessError::CorruptRecord),
        }
    }
}

fn valid_progress_label(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_PROGRESS_LABEL_BYTES
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
}

/// Durable secret-free actor progress committed under one process generation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct MakerActorProgressSnapshotV1 {
    swap_id: SwapId,
    actor_kind: MakerActorKindV1,
    source_generation: u64,
    observation: MakerActorProgressObservationV1,
    observed_at: u64,
}

impl MakerActorProgressSnapshotV1 {
    /// Application swap identity.
    #[must_use]
    pub const fn swap_id(&self) -> &SwapId {
        &self.swap_id
    }

    /// Pair actor that produced the observation.
    #[must_use]
    pub const fn actor_kind(&self) -> MakerActorKindV1 {
        self.actor_kind
    }

    /// Exact process generation under which output was validated.
    #[must_use]
    pub const fn source_generation(&self) -> u64 {
        self.source_generation
    }

    /// Validated secret-free lifecycle observation.
    #[must_use]
    pub const fn observation(&self) -> &MakerActorProgressObservationV1 {
        &self.observation
    }

    /// Trusted application time when the observation was committed.
    #[must_use]
    pub const fn observed_at(&self) -> u64 {
        self.observed_at
    }
}

/// One transactionally consistent, secret-free scheduler projection.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MakerActorMonitorSnapshotV1 {
    process: MakerActorProcessRecordV1,
    progress: Option<MakerActorProgressSnapshotV1>,
    manual_action: Option<MakerActorManualActionSnapshot>,
}

impl MakerActorMonitorSnapshotV1 {
    /// Exact durable scheduler record observed in this read transaction.
    #[must_use]
    pub const fn process(&self) -> &MakerActorProcessRecordV1 {
        &self.process
    }

    /// Latest validated actor progress from the same read transaction.
    #[must_use]
    pub const fn progress(&self) -> Option<&MakerActorProgressSnapshotV1> {
        self.progress.as_ref()
    }

    /// Latest owner action from the same read transaction.
    #[must_use]
    pub const fn manual_action(&self) -> Option<&MakerActorManualActionSnapshot> {
        self.manual_action.as_ref()
    }
}

#[derive(Serialize)]
struct StoredManualActionRequest<'a> {
    swap_id: &'a SwapId,
    action: MakerActorManualAction,
    expected_generation: u64,
}

#[derive(Deserialize, Serialize)]
struct StoredManualActionResultV1 {
    schema_version: u16,
    requested_after_generation: u64,
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
    /// The operating system could not provide lease-owner entropy.
    #[error("maker actor lease owner entropy is unavailable")]
    LeaseOwnerEntropy,
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
    /// A request ID was already bound to another application mutation or payload.
    #[error("maker actor manual action request conflicts with durable state")]
    ManualActionRequestConflict,
    /// The operator based a new action on an obsolete process generation.
    #[error("maker actor manual action generation is stale")]
    ManualActionGenerationConflict,
    /// Another action remains queued or leased for this swap.
    #[error("maker actor already has a pending manual action")]
    ManualActionPending,
    /// The actor cannot safely admit a new manual action in its current schedule state.
    #[error("maker actor manual action is unavailable")]
    ManualActionUnavailable,
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
    /// Exact config bytes disagree with pair-specific manifest semantics.
    #[error("maker actor deployment semantics do not match")]
    ArtifactSemanticMismatch,
    /// Verified artifacts could not be sealed or mapped into one child.
    #[error("maker actor deployment artifacts could not be prepared")]
    ArtifactPreparation,
    /// Child descriptor roles collide or refer to the same kernel lock.
    #[error("maker actor child descriptor mapping is invalid")]
    InvalidDescriptorMapping,
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

pub(crate) fn register_maker_actor_in_transaction(
    transaction: &Transaction<'_>,
    manifest: &MakerActorManifestV1,
    now: u64,
) -> Result<MakerActorRegistrationCommit, MakerActorProcessError> {
    validate_manifest_swap(transaction, manifest)?;
    if let Some(existing) = load_record(transaction, manifest.swap_id())? {
        if existing.manifest == *manifest {
            return Ok(MakerActorRegistrationCommit { was_replay: true });
        }
        return Err(MakerActorProcessError::RegistrationConflict);
    }
    let now = time_to_sql(now);
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
    Ok(MakerActorRegistrationCommit { was_replay: false })
}

pub(crate) fn require_exact_maker_actor_in_transaction(
    transaction: &Transaction<'_>,
    manifest: &MakerActorManifestV1,
) -> Result<(), MakerActorProcessError> {
    validate_manifest_swap(transaction, manifest)?;
    let existing = load_record(transaction, manifest.swap_id())?
        .ok_or(MakerActorProcessError::RegistrationConflict)?;
    if existing.manifest != *manifest {
        return Err(MakerActorProcessError::RegistrationConflict);
    }
    Ok(())
}

pub(crate) fn load_maker_actor_manifest_in_transaction(
    transaction: &Transaction<'_>,
    swap_id: &SwapId,
) -> Result<Option<MakerActorManifestV1>, MakerActorProcessError> {
    Ok(load_record(transaction, swap_id)?.map(|record| record.manifest))
}

fn validate_manifest_swap(
    connection: &Connection,
    manifest: &MakerActorManifestV1,
) -> Result<(), MakerActorProcessError> {
    let swap_json = connection
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
    Ok(())
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
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let commit = register_maker_actor_in_transaction(&transaction, manifest, now)?;
        transaction.commit()?;
        Ok(commit)
    }

    /// Atomically queues one explicit action or exact-replays its original admission.
    ///
    /// The expected generation is checked only for a new request. Exact replay
    /// remains valid after later leases or restart. A new action never splices
    /// into an already leased worker and at most one action is open per swap.
    ///
    /// # Errors
    ///
    /// Fails for request-ID reuse, stale generation, an unavailable actor,
    /// another open action, corrupt state, or a durable-store error.
    pub fn queue_maker_actor_manual_action(
        &mut self,
        request_id: &RequestId,
        swap_id: &SwapId,
        action: MakerActorManualAction,
        expected_generation: u64,
        now: u64,
    ) -> Result<MakerActorManualActionCommit, MakerActorProcessError> {
        let request_json = serde_json::to_string(&StoredManualActionRequest {
            swap_id,
            action,
            expected_generation,
        })
        .map_err(StoreError::from)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(commit) = replay_manual_action_request(&transaction, request_id, &request_json)?
        {
            transaction.commit()?;
            return Ok(commit);
        }

        let record = load_record(&transaction, swap_id)?
            .ok_or(MakerActorProcessError::ManualActionUnavailable)?;
        if record.lease_generation != expected_generation {
            return Err(MakerActorProcessError::ManualActionGenerationConflict);
        }
        if record.schedule_state == MakerActorScheduleState::Terminal {
            return Err(MakerActorProcessError::ManualActionUnavailable);
        }
        if load_open_manual_action(&transaction, swap_id)?.is_some() {
            return Err(MakerActorProcessError::ManualActionPending);
        }
        let now = time_to_sql(now);
        transaction.execute(
            "INSERT INTO maker_actor_manual_actions (
                 request_id, swap_id, action, state, requested_after_generation,
                 created_at, updated_at
             ) VALUES (?1, ?2, ?3, 'queued', ?4, ?5, ?5)",
            params![
                request_id.as_str(),
                swap_id.as_str(),
                action.name(),
                generation_to_sql(expected_generation)?,
                now,
            ],
        )?;
        if record.schedule_state != MakerActorScheduleState::Leased {
            let changed = transaction.execute(
                "UPDATE maker_actor_processes SET
                     desired_state = 'running', schedule_state = 'queued',
                     next_attempt_at = ?1, lease_owner = NULL, leased_at = NULL,
                     child_pid = NULL, child_start_ticks = NULL,
                     last_failure_class = NULL, updated_at = ?1
                 WHERE swap_id = ?2 AND schedule_state IN ('queued', 'backoff', 'failed')
                   AND lease_generation = ?3",
                params![
                    now,
                    swap_id.as_str(),
                    generation_to_sql(expected_generation)?,
                ],
            )?;
            if changed != 1 {
                return Err(MakerActorProcessError::ManualActionUnavailable);
            }
        }
        let result_json = serde_json::to_string(&StoredManualActionResultV1 {
            schema_version: 1,
            requested_after_generation: expected_generation,
        })
        .map_err(StoreError::from)?;
        transaction.execute(
            "INSERT INTO maker_application_mutations (
                 request_id, operation, request_payload_version, request_json, result_json
             ) VALUES (?1, 'actor_action_request', 1, ?2, ?3)",
            params![request_id.as_str(), request_json, result_json],
        )?;
        transaction.commit()?;
        Ok(MakerActorManualActionCommit {
            requested_after_generation: expected_generation,
            was_replay: false,
        })
    }

    /// Returns the latest secret-free manual-action snapshot for one actor.
    ///
    /// # Errors
    ///
    /// Fails when the durable row is malformed or unavailable.
    pub fn maker_actor_manual_action(
        &self,
        swap_id: &SwapId,
    ) -> Result<Option<MakerActorManualActionSnapshot>, MakerActorProcessError> {
        load_latest_manual_action(&self.connection, swap_id)
    }

    /// Returns the latest validated secret-free actor progress snapshot.
    ///
    /// This pure `SQLite` read never opens the private actor database, invokes an
    /// actor process, or contacts a chain RPC.
    ///
    /// # Errors
    ///
    /// Fails when the durable row is malformed or unavailable.
    pub fn maker_actor_progress(
        &self,
        swap_id: &SwapId,
    ) -> Result<Option<MakerActorProgressSnapshotV1>, MakerActorProcessError> {
        load_actor_progress(&self.connection, swap_id)
    }

    /// Attaches one queued action to the exact active actor lease.
    ///
    /// Repeating this call with the same owner and generation is an exact replay.
    /// A forged or stale lease cannot observe or take action authority.
    ///
    /// # Errors
    ///
    /// Fails when the actor lease is stale or the action row is corrupt.
    pub fn claim_maker_actor_manual_action(
        &mut self,
        lease: &MakerActorLeaseV1,
    ) -> Result<Option<MakerActorManualActionSnapshot>, MakerActorProcessError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let record = load_record(&transaction, lease.record.swap_id())?
            .ok_or(MakerActorProcessError::LeaseConflict)?;
        if record.schedule_state != MakerActorScheduleState::Leased
            || record.lease_owner != Some(lease.owner)
            || record.lease_generation != lease.generation
        {
            return Err(MakerActorProcessError::LeaseConflict);
        }
        let Some(action) = load_open_manual_action(&transaction, lease.record.swap_id())? else {
            transaction.commit()?;
            return Ok(None);
        };
        match action.state {
            MakerActorManualActionState::Queued => {
                if lease.generation < action.requested_after_generation {
                    return Err(MakerActorProcessError::CorruptRecord);
                }
                if lease.generation == action.requested_after_generation {
                    transaction.commit()?;
                    return Ok(None);
                }
                let changed = transaction.execute(
                    "UPDATE maker_actor_manual_actions SET
                         state = 'leased', lease_owner = ?1, lease_generation = ?2
                     WHERE request_id = ?3 AND state = 'queued'",
                    params![
                        lease.owner.bytes().as_slice(),
                        generation_to_sql(lease.generation)?,
                        action.request_id.as_str(),
                    ],
                )?;
                if changed != 1 {
                    return Err(MakerActorProcessError::LeaseConflict);
                }
            }
            MakerActorManualActionState::Leased
                if action.lease_owner == Some(lease.owner)
                    && action.lease_generation == Some(lease.generation) => {}
            _ => return Err(MakerActorProcessError::LeaseConflict),
        }
        let leased = load_manual_action_by_request(&transaction, &action.request_id)?
            .ok_or(MakerActorProcessError::CorruptRecord)?;
        transaction.commit()?;
        Ok(Some(leased))
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

    /// Clears one reaped child only for its exact durable lease and identity.
    ///
    /// A supervisor may run a bounded offline status subprocess before the
    /// effect-capable subprocess. Clearing by owner, generation, PID, and start
    /// ticks prevents either process from erasing a newer child's diagnostic
    /// identity. The kernel lock remains the execution authority throughout.
    ///
    /// # Errors
    ///
    /// Fails for an invalid child identity, stale lease/child, or store error.
    pub fn clear_maker_actor_child(
        &mut self,
        lease: &MakerActorLeaseV1,
        pid: u32,
        start_ticks: u64,
    ) -> Result<(), MakerActorProcessError> {
        if pid == 0 || start_ticks == 0 {
            return Err(MakerActorProcessError::InvalidSchedulingInput);
        }
        let changed = self.connection.execute(
            "UPDATE maker_actor_processes SET child_pid = NULL, child_start_ticks = NULL
             WHERE swap_id = ?1 AND schedule_state = 'leased'
               AND lease_owner = ?2 AND lease_generation = ?3
               AND child_pid = ?4 AND child_start_ticks = ?5",
            params![
                lease.record.swap_id().as_str(),
                lease.owner.bytes().as_slice(),
                generation_to_sql(lease.generation)?,
                i64::from(pid),
                time_to_sql(start_ticks),
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
        self.resolve_maker_actor_attempt_inner(lease, resolution, None, now)
    }

    /// Resolves one attempt and commits its validated actor progress atomically.
    ///
    /// The process row, any attached manual action, and progress snapshot share
    /// one immediate transaction under the exact owner and generation fence.
    /// A stale worker therefore updates none of them.
    ///
    /// # Errors
    ///
    /// Fails for invalid progress, invalid resolution data, a stale lease, or
    /// a durable-store error.
    pub fn resolve_maker_actor_attempt_with_progress(
        &mut self,
        lease: &MakerActorLeaseV1,
        resolution: MakerActorAttemptResolution,
        progress: &MakerActorProgressObservationV1,
        now: u64,
    ) -> Result<(), MakerActorProcessError> {
        progress
            .validate()
            .map_err(|_| MakerActorProcessError::InvalidSchedulingInput)?;
        self.resolve_maker_actor_attempt_inner(lease, resolution, Some(progress), now)
    }

    fn resolve_maker_actor_attempt_inner(
        &mut self,
        lease: &MakerActorLeaseV1,
        resolution: MakerActorAttemptResolution,
        progress: Option<&MakerActorProgressObservationV1>,
        now: u64,
    ) -> Result<(), MakerActorProcessError> {
        let manual_state = match &resolution {
            MakerActorAttemptResolution::Requeue { .. }
            | MakerActorAttemptResolution::Backoff { .. } => MakerActorManualActionState::Queued,
            MakerActorAttemptResolution::ManualActionCompleted => {
                MakerActorManualActionState::Completed
            }
            MakerActorAttemptResolution::Terminal | MakerActorAttemptResolution::Failed { .. } => {
                MakerActorManualActionState::Failed
            }
        };
        let (state, next, failure) = resolution_fields(resolution)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let changed = transaction.execute(
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
        if changed != 1 {
            return Err(MakerActorProcessError::LeaseConflict);
        }
        let leased_action = load_open_manual_action(&transaction, lease.record.swap_id())?;
        if let Some(action) = leased_action {
            match action.state {
                MakerActorManualActionState::Queued
                    if action.requested_after_generation == lease.generation =>
                {
                    match manual_state {
                        MakerActorManualActionState::Queued => {}
                        MakerActorManualActionState::Failed => {
                            let changed = transaction.execute(
                                "UPDATE maker_actor_manual_actions SET
                                     state = 'failed', updated_at = ?1
                                 WHERE request_id = ?2 AND state = 'queued'",
                                params![time_to_sql(now), action.request_id.as_str()],
                            )?;
                            if changed != 1 {
                                return Err(MakerActorProcessError::LeaseConflict);
                            }
                        }
                        _ => return Err(MakerActorProcessError::LeaseConflict),
                    }
                }
                MakerActorManualActionState::Leased
                    if action.lease_owner == Some(lease.owner)
                        && action.lease_generation == Some(lease.generation) =>
                {
                    let changed = transaction.execute(
                        "UPDATE maker_actor_manual_actions SET
                             state = ?1, lease_owner = NULL, lease_generation = NULL,
                             updated_at = ?2
                         WHERE request_id = ?3 AND state = 'leased'
                           AND lease_owner = ?4 AND lease_generation = ?5",
                        params![
                            manual_state.name(),
                            time_to_sql(now),
                            action.request_id.as_str(),
                            lease.owner.bytes().as_slice(),
                            generation_to_sql(lease.generation)?,
                        ],
                    )?;
                    if changed != 1 {
                        return Err(MakerActorProcessError::LeaseConflict);
                    }
                }
                _ => return Err(MakerActorProcessError::LeaseConflict),
            }
        }
        if let Some(progress) = progress {
            upsert_actor_progress(&transaction, lease, progress, now)?;
        }
        transaction.commit()?;
        Ok(())
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
        if let Some(action) = load_open_manual_action(&transaction, lease.record.swap_id())? {
            match action.state {
                MakerActorManualActionState::Queued => {}
                MakerActorManualActionState::Leased
                    if action.lease_owner == Some(lease.owner)
                        && action.lease_generation == Some(lease.generation) =>
                {
                    let changed = transaction.execute(
                        "UPDATE maker_actor_manual_actions SET
                             lease_owner = ?1, lease_generation = ?2, updated_at = ?3
                         WHERE request_id = ?4 AND state = 'leased'
                           AND lease_owner = ?5 AND lease_generation = ?6",
                        params![
                            new_owner.bytes().as_slice(),
                            generation_to_sql(generation)?,
                            now,
                            action.request_id.as_str(),
                            lease.owner.bytes().as_slice(),
                            generation_to_sql(lease.generation)?,
                        ],
                    )?;
                    if changed != 1 {
                        return Err(MakerActorProcessError::LeaseConflict);
                    }
                }
                _ => return Err(MakerActorProcessError::LeaseConflict),
            }
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

    /// Returns one exact secret-free scheduler record by application swap ID.
    ///
    /// # Errors
    ///
    /// Fails when a durable row is unavailable, malformed, or unsupported.
    pub fn maker_actor_process(
        &self,
        swap_id: &SwapId,
    ) -> Result<Option<MakerActorProcessRecordV1>, MakerActorProcessError> {
        load_record(&self.connection, swap_id)
    }

    /// Reads process, progress, and latest action from one consistent `SQLite` snapshot.
    ///
    /// This read never opens an actor-private database or contacts a chain RPC.
    ///
    /// # Errors
    ///
    /// Fails when any durable row is malformed or the snapshot cannot be committed.
    pub fn maker_actor_monitor_snapshot(
        &mut self,
        swap_id: &SwapId,
    ) -> Result<Option<MakerActorMonitorSnapshotV1>, MakerActorProcessError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Deferred)?;
        let Some(process) = load_record(&transaction, swap_id)? else {
            transaction.commit()?;
            return Ok(None);
        };
        let progress = load_actor_progress(&transaction, swap_id)?;
        let manual_action = load_latest_manual_action(&transaction, swap_id)?;
        if progress
            .as_ref()
            .is_some_and(|snapshot| snapshot.actor_kind() != process.manifest().kind())
            || manual_action
                .as_ref()
                .is_some_and(|snapshot| snapshot.swap_id() != process.swap_id())
        {
            return Err(MakerActorProcessError::CorruptRecord);
        }
        transaction.commit()?;
        Ok(Some(MakerActorMonitorSnapshotV1 {
            process,
            progress,
            manual_action,
        }))
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
        MakerActorAttemptResolution::Terminal
        | MakerActorAttemptResolution::ManualActionCompleted => {
            Ok((MakerActorScheduleState::Terminal, 0, None))
        }
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

fn replay_manual_action_request(
    transaction: &Transaction<'_>,
    request_id: &RequestId,
    request_json: &str,
) -> Result<Option<MakerActorManualActionCommit>, MakerActorProcessError> {
    let prior = transaction
        .query_row(
            "SELECT operation, request_json, result_json
               FROM maker_application_mutations WHERE request_id = ?1",
            [request_id.as_str()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .optional()?;
    let Some((operation, stored_request, stored_result)) = prior else {
        return Ok(None);
    };
    if operation != "actor_action_request" || stored_request != request_json {
        return Err(MakerActorProcessError::ManualActionRequestConflict);
    }
    let result: StoredManualActionResultV1 =
        serde_json::from_str(&stored_result).map_err(StoreError::from)?;
    if result.schema_version != 1
        || load_manual_action_by_request(transaction, request_id)?.is_none()
    {
        return Err(MakerActorProcessError::CorruptRecord);
    }
    Ok(Some(MakerActorManualActionCommit {
        requested_after_generation: result.requested_after_generation,
        was_replay: true,
    }))
}

type RawManualAction = (
    String,
    String,
    String,
    String,
    i64,
    Option<Vec<u8>>,
    Option<i64>,
);

fn read_raw_manual_action(row: &Row<'_>) -> rusqlite::Result<RawManualAction> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
    ))
}

fn decode_manual_action(
    raw: RawManualAction,
) -> Result<MakerActorManualActionSnapshot, MakerActorProcessError> {
    let (request_id, swap_id, action, state, requested_generation, owner, lease_generation) = raw;
    let state = MakerActorManualActionState::parse(&state)?;
    let lease_owner = decode_owner(owner)?;
    let lease_generation = lease_generation.map(decode_u64).transpose()?;
    if (state == MakerActorManualActionState::Leased)
        != (lease_owner.is_some() && lease_generation.is_some())
    {
        return Err(MakerActorProcessError::CorruptRecord);
    }
    Ok(MakerActorManualActionSnapshot {
        request_id: RequestId::new(request_id)
            .map_err(|_| MakerActorProcessError::CorruptRecord)?,
        swap_id: SwapId::new(swap_id).map_err(|_| MakerActorProcessError::CorruptRecord)?,
        action: MakerActorManualAction::parse(&action)?,
        state,
        requested_after_generation: decode_u64(requested_generation)?,
        lease_owner,
        lease_generation,
    })
}

const MANUAL_ACTION_COLUMNS: &str =
    "request_id, swap_id, action, state, requested_after_generation, lease_owner, lease_generation";

fn load_manual_action_by_request(
    connection: &Connection,
    request_id: &RequestId,
) -> Result<Option<MakerActorManualActionSnapshot>, MakerActorProcessError> {
    let sql = format!(
        "SELECT {MANUAL_ACTION_COLUMNS} FROM maker_actor_manual_actions WHERE request_id = ?1"
    );
    connection
        .query_row(&sql, [request_id.as_str()], read_raw_manual_action)
        .optional()?
        .map(decode_manual_action)
        .transpose()
}

fn load_open_manual_action(
    connection: &Connection,
    swap_id: &SwapId,
) -> Result<Option<MakerActorManualActionSnapshot>, MakerActorProcessError> {
    let sql = format!(
        "SELECT {MANUAL_ACTION_COLUMNS} FROM maker_actor_manual_actions
         WHERE swap_id = ?1 AND state IN ('queued', 'leased') ORDER BY sequence DESC LIMIT 1"
    );
    connection
        .query_row(&sql, [swap_id.as_str()], read_raw_manual_action)
        .optional()?
        .map(decode_manual_action)
        .transpose()
}

fn load_latest_manual_action(
    connection: &Connection,
    swap_id: &SwapId,
) -> Result<Option<MakerActorManualActionSnapshot>, MakerActorProcessError> {
    let sql = format!(
        "SELECT {MANUAL_ACTION_COLUMNS} FROM maker_actor_manual_actions
         WHERE swap_id = ?1 ORDER BY sequence DESC LIMIT 1"
    );
    connection
        .query_row(&sql, [swap_id.as_str()], read_raw_manual_action)
        .optional()?
        .map(decode_manual_action)
        .transpose()
}

fn upsert_actor_progress(
    transaction: &Transaction<'_>,
    lease: &MakerActorLeaseV1,
    progress: &MakerActorProgressObservationV1,
    now: u64,
) -> Result<(), MakerActorProcessError> {
    let payload_json = serde_json::to_string(progress).map_err(StoreError::from)?;
    let changed = transaction.execute(
        "INSERT INTO maker_actor_progress (
             swap_id, payload_version, actor_kind, source_generation,
             payload_json, observed_at
         ) VALUES (?1, 1, ?2, ?3, ?4, ?5)
         ON CONFLICT (swap_id) DO UPDATE SET
             actor_kind = excluded.actor_kind,
             source_generation = excluded.source_generation,
             payload_json = excluded.payload_json,
             observed_at = excluded.observed_at
         WHERE maker_actor_progress.source_generation <= excluded.source_generation",
        params![
            lease.record.swap_id().as_str(),
            lease.record.manifest().kind().name(),
            generation_to_sql(lease.generation)?,
            payload_json,
            time_to_sql(now),
        ],
    )?;
    if changed == 1 {
        Ok(())
    } else {
        Err(MakerActorProcessError::LeaseConflict)
    }
}

type RawActorProgress = (String, i64, String, i64, String, i64, String);

fn decode_actor_progress(
    raw: RawActorProgress,
) -> Result<MakerActorProgressSnapshotV1, MakerActorProcessError> {
    let (swap_id, payload_version, actor_kind, generation, payload, observed_at, process_kind) =
        raw;
    if payload_version != 1 || actor_kind != process_kind {
        return Err(MakerActorProcessError::CorruptRecord);
    }
    let observation: MakerActorProgressObservationV1 =
        serde_json::from_str(&payload).map_err(|_| MakerActorProcessError::CorruptRecord)?;
    observation.validate()?;
    Ok(MakerActorProgressSnapshotV1 {
        swap_id: SwapId::new(swap_id).map_err(|_| MakerActorProcessError::CorruptRecord)?,
        actor_kind: MakerActorKindV1::parse(&actor_kind)?,
        source_generation: decode_u64(generation)?,
        observation,
        observed_at: decode_u64(observed_at)?,
    })
}

fn load_actor_progress(
    connection: &Connection,
    swap_id: &SwapId,
) -> Result<Option<MakerActorProgressSnapshotV1>, MakerActorProcessError> {
    connection
        .query_row(
            "SELECT progress.swap_id, progress.payload_version,
                    progress.actor_kind, progress.source_generation,
                    progress.payload_json, progress.observed_at, process.actor_kind
             FROM maker_actor_progress AS progress
             JOIN maker_actor_processes AS process ON process.swap_id = progress.swap_id
             WHERE progress.swap_id = ?1",
            [swap_id.as_str()],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                ))
            },
        )
        .optional()?
        .map(decode_actor_progress)
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
    let mut file = open_no_symlinks(path, OFlags::RDONLY | OFlags::NONBLOCK, Mode::empty())
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
    match open_no_symlinks(path, OFlags::RDWR | OFlags::NONBLOCK, Mode::empty()) {
        Ok(file) => {
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
    if !is_owner_private_regular_file(&opened, 0o600)
        || !named.file_type().is_file()
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
            actor_kind           TEXT NOT NULL CHECK (actor_kind IN ('bitcoin', 'monero', 'zcash')),
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
            ON maker_actor_processes (desired_state, schedule_state, next_attempt_at, swap_id);
        CREATE TABLE IF NOT EXISTS maker_actor_manual_actions (
            sequence                   INTEGER PRIMARY KEY AUTOINCREMENT,
            request_id                 TEXT NOT NULL UNIQUE,
            swap_id                    TEXT NOT NULL REFERENCES maker_actor_processes(swap_id)
                                           ON DELETE CASCADE,
            action                     TEXT NOT NULL CHECK (action IN ('claim', 'refund')),
            state                      TEXT NOT NULL CHECK (
                                           state IN ('queued', 'leased', 'completed', 'failed')
                                       ),
            requested_after_generation INTEGER NOT NULL CHECK (requested_after_generation >= 0),
            lease_owner                BLOB CHECK (
                                           lease_owner IS NULL OR length(lease_owner) = 16
                                       ),
            lease_generation           INTEGER CHECK (
                                           lease_generation IS NULL OR lease_generation > 0
                                       ),
            created_at                 INTEGER NOT NULL CHECK (created_at >= 0),
            updated_at                 INTEGER NOT NULL CHECK (updated_at >= created_at),
            CHECK (
                (state = 'leased' AND lease_owner IS NOT NULL AND lease_generation IS NOT NULL)
                OR (state != 'leased' AND lease_owner IS NULL AND lease_generation IS NULL)
            )
        ) STRICT;
        CREATE UNIQUE INDEX IF NOT EXISTS maker_actor_manual_actions_one_open
            ON maker_actor_manual_actions (swap_id)
            WHERE state IN ('queued', 'leased');
        CREATE TABLE IF NOT EXISTS maker_actor_progress (
            swap_id           TEXT PRIMARY KEY NOT NULL REFERENCES maker_actor_processes(swap_id)
                                  ON DELETE CASCADE,
            payload_version   INTEGER NOT NULL CHECK (payload_version = 1),
            actor_kind        TEXT NOT NULL CHECK (actor_kind IN ('bitcoin', 'monero', 'zcash')),
            source_generation INTEGER NOT NULL CHECK (source_generation > 0),
            payload_json      TEXT NOT NULL,
            observed_at       INTEGER NOT NULL CHECK (observed_at >= 0)
        ) STRICT;",
    )?;
    if !actor_kind_tables_support_monero(transaction)? {
        rebuild_actor_kind_tables_for_monero(transaction)?;
    }
    Ok(())
}

fn actor_kind_tables_support_monero(
    transaction: &rusqlite::Transaction<'_>,
) -> Result<bool, StoreError> {
    for table in ["maker_actor_processes", "maker_actor_progress"] {
        let sql: String = transaction.query_row(
            "SELECT sql FROM sqlite_schema WHERE type = 'table' AND name = ?1",
            rusqlite::params![table],
            |row| row.get(0),
        )?;
        if !sql.contains("'monero'") {
            return Ok(false);
        }
    }
    Ok(true)
}

#[allow(clippy::too_many_lines)] // Explicit column copies make migration preservation auditable.
fn rebuild_actor_kind_tables_for_monero(
    transaction: &rusqlite::Transaction<'_>,
) -> Result<(), StoreError> {
    transaction.execute_batch(
        "DROP INDEX IF EXISTS maker_actor_processes_due;
         DROP INDEX IF EXISTS maker_actor_manual_actions_one_open;
         ALTER TABLE maker_actor_progress RENAME TO maker_actor_progress_before_monero;
         ALTER TABLE maker_actor_manual_actions RENAME TO maker_actor_manual_actions_before_monero;
         ALTER TABLE maker_actor_processes RENAME TO maker_actor_processes_before_monero;

         CREATE TABLE maker_actor_processes (
             swap_id              TEXT PRIMARY KEY NOT NULL REFERENCES swaps(id) ON DELETE CASCADE,
             actor_kind           TEXT NOT NULL CHECK (actor_kind IN ('bitcoin', 'monero', 'zcash')),
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
         CREATE INDEX maker_actor_processes_due
             ON maker_actor_processes (desired_state, schedule_state, next_attempt_at, swap_id);

         CREATE TABLE maker_actor_manual_actions (
             sequence                   INTEGER PRIMARY KEY AUTOINCREMENT,
             request_id                 TEXT NOT NULL UNIQUE,
             swap_id                    TEXT NOT NULL REFERENCES maker_actor_processes(swap_id)
                                            ON DELETE CASCADE,
             action                     TEXT NOT NULL CHECK (action IN ('claim', 'refund')),
             state                      TEXT NOT NULL CHECK (
                                            state IN ('queued', 'leased', 'completed', 'failed')
                                        ),
             requested_after_generation INTEGER NOT NULL CHECK (requested_after_generation >= 0),
             lease_owner                BLOB CHECK (
                                            lease_owner IS NULL OR length(lease_owner) = 16
                                        ),
             lease_generation           INTEGER CHECK (
                                            lease_generation IS NULL OR lease_generation > 0
                                        ),
             created_at                 INTEGER NOT NULL CHECK (created_at >= 0),
             updated_at                 INTEGER NOT NULL CHECK (updated_at >= created_at),
             CHECK (
                 (state = 'leased' AND lease_owner IS NOT NULL AND lease_generation IS NOT NULL)
                 OR (state != 'leased' AND lease_owner IS NULL AND lease_generation IS NULL)
             )
         ) STRICT;
         CREATE UNIQUE INDEX maker_actor_manual_actions_one_open
             ON maker_actor_manual_actions (swap_id)
             WHERE state IN ('queued', 'leased');

         CREATE TABLE maker_actor_progress (
             swap_id           TEXT PRIMARY KEY NOT NULL REFERENCES maker_actor_processes(swap_id)
                                   ON DELETE CASCADE,
             payload_version   INTEGER NOT NULL CHECK (payload_version = 1),
             actor_kind        TEXT NOT NULL CHECK (actor_kind IN ('bitcoin', 'monero', 'zcash')),
             source_generation INTEGER NOT NULL CHECK (source_generation > 0),
             payload_json      TEXT NOT NULL,
             observed_at       INTEGER NOT NULL CHECK (observed_at >= 0)
         ) STRICT;

         INSERT INTO maker_actor_processes (
             swap_id, actor_kind, manifest_version, manifest_path, manifest_sha256,
             actor_program_path, actor_program_sha256, state_db_path, desired_state,
             schedule_state, next_attempt_at, lease_generation, lease_owner, leased_at,
             child_pid, child_start_ticks, attempt_count, last_failure_class, created_at, updated_at
         )
         SELECT
             swap_id, actor_kind, manifest_version, manifest_path, manifest_sha256,
             actor_program_path, actor_program_sha256, state_db_path, desired_state,
             schedule_state, next_attempt_at, lease_generation, lease_owner, leased_at,
             child_pid, child_start_ticks, attempt_count, last_failure_class, created_at, updated_at
         FROM maker_actor_processes_before_monero;

         INSERT INTO maker_actor_manual_actions (
             sequence, request_id, swap_id, action, state, requested_after_generation,
             lease_owner, lease_generation, created_at, updated_at
         )
         SELECT
             sequence, request_id, swap_id, action, state, requested_after_generation,
             lease_owner, lease_generation, created_at, updated_at
         FROM maker_actor_manual_actions_before_monero;

         INSERT INTO maker_actor_progress (
             swap_id, payload_version, actor_kind, source_generation, payload_json, observed_at
         )
         SELECT
             swap_id, payload_version, actor_kind, source_generation, payload_json, observed_at
         FROM maker_actor_progress_before_monero;

         DROP TABLE maker_actor_progress_before_monero;
         DROP TABLE maker_actor_manual_actions_before_monero;
         DROP TABLE maker_actor_processes_before_monero;",
    )?;
    Ok(())
}
