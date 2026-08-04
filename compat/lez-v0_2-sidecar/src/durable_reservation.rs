use std::{
    fmt,
    fs::File,
    io::{Read, Write},
    os::unix::fs::MetadataExt,
    path::{Component, Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use rustix::{
    fs::{
        AtFlags, CWD, Dir, FlockOperation, Mode, OFlags, RenameFlags, ResolveFlags, flock, openat,
        openat2, renameat_with, unlinkat,
    },
    io::Errno,
    process::geteuid,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

const BRIDGE_STATE_LEASE_FILENAME: &str = "bridge-state-lease.v1.lock";
const RESERVATION_SCHEMA_VERSION: u32 = 1;
const MAX_RESERVATION_BYTES: u64 = 8 * 1024 * 1024;
static TEMP_FILE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Fail-closed durable reservation failures with no state path disclosure.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum DurableReservationError {
    /// The actor's state directory is not an owner-only real directory.
    #[error("durable reservation directory is not an owner-only real directory")]
    InsecureDirectory,
    /// The state entry is not one owner-only, regular, non-aliased file.
    #[error("durable reservation is not an owner-only non-aliased regular file")]
    InsecureStateFile,
    /// A crash-left temporary file requires explicit operator recovery.
    #[error("a partial durable reservation requires explicit recovery")]
    PartialReservation,
    /// The durable envelope or payload is malformed or unsupported in place.
    #[error("durable reservation is corrupt")]
    CorruptReservation,
    /// A newer writer owns the state and this binary must not reinterpret it.
    #[error("durable reservation uses a future schema")]
    FutureSchema,
    /// Another process atomically installed the reservation first.
    #[error("durable reservation already exists")]
    AlreadyReserved,
    /// A redacted filesystem operation failed.
    #[error("durable reservation filesystem operation failed")]
    Filesystem,
}

/// Fail-closed errors while taking exclusive ownership of a sidecar state directory.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum StateDirectoryLeaseError {
    /// The directory or fixed lease file is aliased, foreign-owned, or has unsafe permissions.
    #[error("sidecar state directory or lease file is unsafe")]
    UnsafeState,
    /// Another live file description already holds the exclusive lease.
    #[error("sidecar state directory is already owned by another process")]
    AlreadyHeld,
    /// A redacted filesystem or lock operation failed.
    #[error("sidecar state directory lease operation failed")]
    Filesystem,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ReservationKind {
    NativeEscrow,
    XmrNativeEscrowV3,
    XmrNativeClaimAuthorizationV3,
    XmrNativeClaimV3,
    XmrNativeClaimCompletionV3,
    XmrNativeRefundV3,
    XmrNativeRefundCompletionV3,
    XmrNativePunishV3,
    XmrCurrentProfileClockV1,
    WitnessedEscrow,
    NativeClaim,
    WitnessedClaim,
    WitnessedClaimCompletion,
    NativeRefund,
    WitnessedAssetEscrowV2,
    WitnessedAssetClaimV2,
    WitnessedAssetClaimCompletionV2,
    WitnessedAssetRefundV2,
    VaultClaim,
}

impl ReservationKind {
    const fn filename(self) -> &'static str {
        match self {
            Self::NativeEscrow => "native-escrow-reservation.v1.json",
            Self::XmrNativeEscrowV3 => "xmr-native-escrow-reservation.v3.json",
            Self::XmrNativeClaimAuthorizationV3 => {
                "xmr-native-claim-authorization-reservation.v3.json"
            }
            Self::XmrNativeClaimV3 => "xmr-native-claim-reservation.v3.json",
            Self::XmrNativeClaimCompletionV3 => "xmr-native-claim-completion.v3.json",
            Self::XmrNativeRefundV3 => "xmr-native-refund-reservation.v3.json",
            Self::XmrNativeRefundCompletionV3 => "xmr-native-refund-completion.v3.json",
            Self::XmrNativePunishV3 => "xmr-native-punish-reservation.v3.json",
            Self::XmrCurrentProfileClockV1 => "xmr-current-profile-clock.v1.json",
            Self::WitnessedEscrow => "witnessed-escrow-reservation.v1.json",
            Self::NativeClaim => "native-claim-reservation.v1.json",
            Self::WitnessedClaim => "witnessed-claim-reservation.v1.json",
            Self::WitnessedClaimCompletion => "witnessed-claim-completion.v1.json",
            Self::NativeRefund => "native-refund-reservation.v1.json",
            Self::WitnessedAssetEscrowV2 => "witnessed-asset-escrow-reservation.v2.json",
            Self::WitnessedAssetClaimV2 => "witnessed-asset-claim-reservation.v2.json",
            Self::WitnessedAssetClaimCompletionV2 => "witnessed-asset-claim-completion.v2.json",
            Self::WitnessedAssetRefundV2 => "witnessed-asset-refund-reservation.v2.json",
            Self::VaultClaim => "vault-claim-reservation.v1.json",
        }
    }

    fn partial_prefix(self) -> String {
        format!(".{}.partial.", self.filename())
    }
}

#[derive(Debug, Deserialize)]
struct ReservationHeader {
    schema_version: u32,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ReservationEnvelope<Request, Output> {
    schema_version: u32,
    kind: ReservationKind,
    request: Request,
    result: Output,
}

pub(crate) struct SecureStateDirectory {
    descriptor: File,
    path: PathBuf,
    device: u64,
    inode: u64,
}

impl fmt::Debug for SecureStateDirectory {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SecureStateDirectory")
            .field("path", &"[REDACTED]")
            .finish_non_exhaustive()
    }
}

impl SecureStateDirectory {
    pub(crate) fn open(path: &Path) -> Result<Self, DurableReservationError> {
        let path =
            std::path::absolute(path).map_err(|_| DurableReservationError::InsecureDirectory)?;
        if path
            .components()
            .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
        {
            return Err(DurableReservationError::InsecureDirectory);
        }
        validate_trusted_parent_chain(&path)?;
        let descriptor = open_secure_directory(&path)?;
        let metadata = validate_directory(&descriptor)?;
        let directory = Self {
            descriptor,
            path,
            device: metadata.dev(),
            inode: metadata.ino(),
        };
        directory.revalidate()?;
        Ok(directory)
    }

    pub(crate) fn descriptor(&self) -> &File {
        &self.descriptor
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) const fn identity(&self) -> (u64, u64) {
        (self.device, self.inode)
    }

    pub(crate) fn revalidate(&self) -> Result<(), DurableReservationError> {
        validate_trusted_parent_chain(&self.path)?;
        let held = validate_directory(&self.descriptor)?;
        if held.dev() != self.device || held.ino() != self.inode {
            return Err(DurableReservationError::InsecureDirectory);
        }
        let reopened = open_secure_directory(&self.path)?;
        let current = validate_directory(&reopened)?;
        if current.dev() != self.device || current.ino() != self.inode {
            return Err(DurableReservationError::InsecureDirectory);
        }
        Ok(())
    }
}

/// Process-held exclusive ownership of one secure sidecar state directory.
///
/// The fixed empty lock file is opened relative to the already validated
/// directory descriptor. Dropping this value releases the kernel lease while
/// retaining the owner-only file for the next process.
pub struct StateDirectoryLease {
    file: File,
    directory: SecureStateDirectory,
}

impl fmt::Debug for StateDirectoryLease {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("StateDirectoryLease")
            .field("state_directory", &"[REDACTED]")
            .finish_non_exhaustive()
    }
}

impl StateDirectoryLease {
    pub(crate) fn state_path(&self) -> &Path {
        self.directory.path()
    }

    pub(crate) const fn state_identity(&self) -> (u64, u64) {
        self.directory.identity()
    }

    /// Takes a nonblocking exclusive lease on one existing owner-only state directory.
    ///
    /// # Errors
    ///
    /// Rejects unsafe directory paths, unsafe or aliased fixed lock files,
    /// concurrent ownership, and redacted filesystem failures.
    pub fn acquire(path: impl AsRef<Path>) -> Result<Self, StateDirectoryLeaseError> {
        let directory = SecureStateDirectory::open(path.as_ref())
            .map_err(|_| StateDirectoryLeaseError::UnsafeState)?;
        directory
            .revalidate()
            .map_err(|_| StateDirectoryLeaseError::UnsafeState)?;
        let descriptor = openat(
            directory.descriptor(),
            BRIDGE_STATE_LEASE_FILENAME,
            OFlags::RDWR | OFlags::CREATE | OFlags::NONBLOCK | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::RUSR | Mode::WUSR,
        )
        .map_err(|_| StateDirectoryLeaseError::UnsafeState)?;
        let file = File::from(descriptor);
        let held = validate_lease_file(&file)?;
        match flock(&file, FlockOperation::NonBlockingLockExclusive) {
            Ok(()) => {}
            Err(error) if error == Errno::AGAIN || error == Errno::WOULDBLOCK => {
                return Err(StateDirectoryLeaseError::AlreadyHeld);
            }
            Err(_) => return Err(StateDirectoryLeaseError::Filesystem),
        }
        let held_after_lock = validate_lease_file(&file)?;
        if held.dev() != held_after_lock.dev() || held.ino() != held_after_lock.ino() {
            return Err(StateDirectoryLeaseError::UnsafeState);
        }
        let reopened = openat(
            directory.descriptor(),
            BRIDGE_STATE_LEASE_FILENAME,
            OFlags::RDWR | OFlags::NONBLOCK | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map(File::from)
        .map_err(|_| StateDirectoryLeaseError::UnsafeState)?;
        let current = validate_lease_file(&reopened)?;
        if held.dev() != current.dev() || held.ino() != current.ino() {
            return Err(StateDirectoryLeaseError::UnsafeState);
        }
        directory
            .descriptor()
            .sync_all()
            .map_err(|_| StateDirectoryLeaseError::Filesystem)?;
        directory
            .revalidate()
            .map_err(|_| StateDirectoryLeaseError::UnsafeState)?;
        Ok(Self { file, directory })
    }
}

impl Drop for StateDirectoryLease {
    fn drop(&mut self) {
        let _ = flock(&self.file, FlockOperation::Unlock);
        let _ = self.directory.revalidate();
    }
}

fn validate_trusted_parent_chain(path: &Path) -> Result<(), DurableReservationError> {
    let parent = path
        .parent()
        .ok_or(DurableReservationError::InsecureDirectory)?;
    let effective_uid = geteuid().as_raw();
    for ancestor in parent.ancestors() {
        let descriptor = open_secure_directory(ancestor)?;
        let metadata = descriptor
            .metadata()
            .map_err(|_| DurableReservationError::InsecureDirectory)?;
        let mode = metadata.mode();
        let trusted_owner = metadata.uid() == 0 || metadata.uid() == effective_uid;
        let group_or_other_writable = mode & 0o022 != 0;
        let sticky = mode & 0o1000 != 0;
        if !metadata.file_type().is_dir() || !trusted_owner || (group_or_other_writable && !sticky)
        {
            return Err(DurableReservationError::InsecureDirectory);
        }
    }
    Ok(())
}

fn open_secure_directory(path: &Path) -> Result<File, DurableReservationError> {
    openat2(
        CWD,
        path,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
        ResolveFlags::NO_SYMLINKS,
    )
    .map(File::from)
    .map_err(|_| DurableReservationError::InsecureDirectory)
}

pub(crate) struct DurableReservationStore {
    directory: SecureStateDirectory,
}

impl fmt::Debug for DurableReservationStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DurableReservationStore")
            .field("directory", &"[REDACTED]")
            .finish_non_exhaustive()
    }
}

impl DurableReservationStore {
    pub(crate) fn open(path: &Path) -> Result<Self, DurableReservationError> {
        let directory = SecureStateDirectory::open(path)?;
        Ok(Self { directory })
    }

    pub(crate) const fn identity(&self) -> (u64, u64) {
        self.directory.identity()
    }

    pub(crate) fn path(&self) -> &Path {
        self.directory.path()
    }

    pub(crate) fn contains_fixed_file(
        &self,
        filename: &str,
    ) -> Result<bool, DurableReservationError> {
        validate_fixed_filename(filename)?;
        self.directory.revalidate()?;
        let descriptor = match openat(
            self.directory.descriptor(),
            filename,
            OFlags::RDONLY | OFlags::NONBLOCK | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        ) {
            Ok(descriptor) => descriptor,
            Err(Errno::NOENT) => return Ok(false),
            Err(Errno::LOOP) => return Err(DurableReservationError::InsecureStateFile),
            Err(_) => return Err(DurableReservationError::Filesystem),
        };
        validate_state_file(&File::from(descriptor))?;
        self.directory.revalidate()?;
        Ok(true)
    }

    pub(crate) fn create_fixed_file_set(
        &self,
        artifacts: &[(&str, &[u8])],
    ) -> Result<(), DurableReservationError> {
        if artifacts.is_empty() {
            return Err(DurableReservationError::Filesystem);
        }
        self.directory.revalidate()?;
        let entries = Dir::read_from(self.directory.descriptor())
            .map_err(|_| DurableReservationError::Filesystem)?;
        for entry in entries {
            let entry = entry.map_err(|_| DurableReservationError::Filesystem)?;
            let name = entry.file_name().to_bytes();
            if name != b"." && name != b".." {
                return Err(DurableReservationError::AlreadyReserved);
            }
        }
        for (index, (filename, bytes)) in artifacts.iter().enumerate() {
            validate_fixed_filename(filename)?;
            if bytes.is_empty()
                || u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_RESERVATION_BYTES
                || artifacts[..index]
                    .iter()
                    .any(|(prior, _)| prior == filename)
            {
                return Err(DurableReservationError::CorruptReservation);
            }
        }

        let mut files = Vec::with_capacity(artifacts.len());
        for (filename, _) in artifacts {
            let descriptor = match openat(
                self.directory.descriptor(),
                *filename,
                OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::RUSR | Mode::WUSR,
            ) {
                Ok(descriptor) => descriptor,
                Err(error) => {
                    for (created_name, _) in &files {
                        let _ =
                            unlinkat(self.directory.descriptor(), *created_name, AtFlags::empty());
                    }
                    let _ = self.directory.descriptor().sync_all();
                    return Err(if error == Errno::EXIST {
                        DurableReservationError::AlreadyReserved
                    } else {
                        DurableReservationError::Filesystem
                    });
                }
            };
            let file = File::from(descriptor);
            validate_state_file(&file)?;
            files.push((*filename, file));
        }

        let write_result = (|| {
            for ((filename, bytes), (_, file)) in artifacts.iter().zip(files.iter_mut()) {
                file.write_all(bytes)
                    .map_err(|_| DurableReservationError::Filesystem)?;
                file.sync_all()
                    .map_err(|_| DurableReservationError::Filesystem)?;
                validate_state_file(file)?;
                let reopened = openat(
                    self.directory.descriptor(),
                    *filename,
                    OFlags::RDONLY | OFlags::NONBLOCK | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                    Mode::empty(),
                )
                .map(File::from)
                .map_err(|_| DurableReservationError::InsecureStateFile)?;
                let held = file
                    .metadata()
                    .map_err(|_| DurableReservationError::InsecureStateFile)?;
                let current = reopened
                    .metadata()
                    .map_err(|_| DurableReservationError::InsecureStateFile)?;
                validate_state_file(&reopened)?;
                if held.dev() != current.dev() || held.ino() != current.ino() {
                    return Err(DurableReservationError::InsecureStateFile);
                }
            }
            self.directory.revalidate()?;
            self.directory
                .descriptor()
                .sync_all()
                .map_err(|_| DurableReservationError::Filesystem)?;
            self.directory.revalidate()
        })();
        if write_result.is_err() {
            for (filename, _) in &files {
                let _ = unlinkat(self.directory.descriptor(), *filename, AtFlags::empty());
            }
            let _ = self.directory.descriptor().sync_all();
        }
        write_result
    }

    pub(crate) fn read_fixed_file(
        &self,
        filename: &str,
        maximum_bytes: u64,
    ) -> Result<Vec<u8>, DurableReservationError> {
        if filename.is_empty()
            || filename.contains('/')
            || filename == "."
            || filename == ".."
            || maximum_bytes == 0
        {
            return Err(DurableReservationError::InsecureStateFile);
        }
        self.directory.revalidate()?;
        let descriptor = openat(
            self.directory.descriptor(),
            filename,
            OFlags::RDONLY | OFlags::NONBLOCK | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|error| match error {
            Errno::NOENT => DurableReservationError::CorruptReservation,
            Errno::LOOP => DurableReservationError::InsecureStateFile,
            _ => DurableReservationError::Filesystem,
        })?;
        let mut file = File::from(descriptor);
        validate_state_file(&file)?;
        let mut bytes = Vec::new();
        Read::by_ref(&mut file)
            .take(maximum_bytes.saturating_add(1))
            .read_to_end(&mut bytes)
            .map_err(|_| DurableReservationError::Filesystem)?;
        if bytes.is_empty() || u64::try_from(bytes.len()).unwrap_or(u64::MAX) > maximum_bytes {
            return Err(DurableReservationError::CorruptReservation);
        }
        validate_state_file(&file)?;
        self.directory.revalidate()?;
        Ok(bytes)
    }

    pub(crate) fn load<Request, Output>(
        &self,
        expected_kind: ReservationKind,
    ) -> Result<Option<(Request, Output)>, DurableReservationError>
    where
        Request: DeserializeOwned,
        Output: DeserializeOwned,
    {
        self.directory.revalidate()?;
        self.reject_partial(expected_kind)?;
        let descriptor = match openat(
            self.directory.descriptor(),
            expected_kind.filename(),
            OFlags::RDONLY | OFlags::NONBLOCK | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        ) {
            Ok(descriptor) => descriptor,
            Err(Errno::NOENT) => return Ok(None),
            Err(Errno::LOOP) => return Err(DurableReservationError::InsecureStateFile),
            Err(_) => return Err(DurableReservationError::Filesystem),
        };
        let mut file = File::from(descriptor);
        validate_state_file(&file)?;

        let mut bytes = Vec::new();
        Read::by_ref(&mut file)
            .take(MAX_RESERVATION_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| DurableReservationError::Filesystem)?;
        if bytes.is_empty()
            || u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_RESERVATION_BYTES
        {
            return Err(DurableReservationError::CorruptReservation);
        }
        validate_state_file(&file)?;

        let header: ReservationHeader = serde_json::from_slice(&bytes)
            .map_err(|_| DurableReservationError::CorruptReservation)?;
        match header.schema_version {
            RESERVATION_SCHEMA_VERSION => {}
            version if version > RESERVATION_SCHEMA_VERSION => {
                return Err(DurableReservationError::FutureSchema);
            }
            _ => return Err(DurableReservationError::CorruptReservation),
        }
        let envelope: ReservationEnvelope<Request, Output> = serde_json::from_slice(&bytes)
            .map_err(|_| DurableReservationError::CorruptReservation)?;
        if envelope.kind != expected_kind {
            return Err(DurableReservationError::CorruptReservation);
        }
        self.directory.revalidate()?;
        Ok(Some((envelope.request, envelope.result)))
    }

    pub(crate) fn create<Request, Output>(
        &self,
        kind: ReservationKind,
        request: &Request,
        result: &Output,
    ) -> Result<(), DurableReservationError>
    where
        Request: Serialize,
        Output: Serialize,
    {
        self.directory.revalidate()?;
        self.reject_partial(kind)?;
        let bytes = serde_json::to_vec(&ReservationEnvelope {
            schema_version: RESERVATION_SCHEMA_VERSION,
            kind,
            request,
            result,
        })
        .map_err(|_| DurableReservationError::CorruptReservation)?;
        if bytes.is_empty()
            || u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_RESERVATION_BYTES
        {
            return Err(DurableReservationError::CorruptReservation);
        }

        let sequence = TEMP_FILE_SEQUENCE.fetch_add(1, Ordering::SeqCst);
        let temporary_name = format!(
            "{}{}.{}",
            kind.partial_prefix(),
            std::process::id(),
            sequence
        );
        let descriptor = openat(
            self.directory.descriptor(),
            temporary_name.as_str(),
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::RUSR | Mode::WUSR,
        )
        .map_err(|_| DurableReservationError::Filesystem)?;
        let mut temporary_file = File::from(descriptor);
        let write_result = (|| {
            validate_state_file(&temporary_file)?;
            temporary_file
                .write_all(&bytes)
                .map_err(|_| DurableReservationError::Filesystem)?;
            temporary_file
                .sync_all()
                .map_err(|_| DurableReservationError::Filesystem)?;
            validate_state_file(&temporary_file)?;
            renameat_with(
                self.directory.descriptor(),
                temporary_name.as_str(),
                self.directory.descriptor(),
                kind.filename(),
                RenameFlags::NOREPLACE,
            )
            .map_err(|error| {
                if error == Errno::EXIST {
                    DurableReservationError::AlreadyReserved
                } else {
                    DurableReservationError::Filesystem
                }
            })?;
            validate_state_file(&temporary_file)?;
            self.directory.revalidate()?;
            self.directory
                .descriptor()
                .sync_all()
                .map_err(|_| DurableReservationError::Filesystem)?;
            self.directory.revalidate()?;
            Ok(())
        })();

        if write_result.is_err() {
            let _ = unlinkat(
                self.directory.descriptor(),
                temporary_name.as_str(),
                AtFlags::empty(),
            );
            let _ = self.directory.descriptor().sync_all();
        }
        write_result
    }

    fn reject_partial(&self, kind: ReservationKind) -> Result<(), DurableReservationError> {
        let prefix = kind.partial_prefix();
        let entries = Dir::read_from(self.directory.descriptor())
            .map_err(|_| DurableReservationError::Filesystem)?;
        for entry in entries {
            let entry = entry.map_err(|_| DurableReservationError::Filesystem)?;
            if entry.file_name().to_bytes().starts_with(prefix.as_bytes()) {
                return Err(DurableReservationError::PartialReservation);
            }
        }
        Ok(())
    }
}

fn validate_directory(directory: &File) -> Result<std::fs::Metadata, DurableReservationError> {
    let metadata = directory
        .metadata()
        .map_err(|_| DurableReservationError::InsecureDirectory)?;
    if !metadata.file_type().is_dir()
        || metadata.uid() != geteuid().as_raw()
        || metadata.mode() & 0o7777 != 0o700
    {
        return Err(DurableReservationError::InsecureDirectory);
    }
    Ok(metadata)
}

fn validate_fixed_filename(filename: &str) -> Result<(), DurableReservationError> {
    if filename.is_empty()
        || filename.contains(char::from(47))
        || filename == "."
        || filename == ".."
    {
        return Err(DurableReservationError::InsecureStateFile);
    }
    Ok(())
}

fn validate_state_file(file: &File) -> Result<(), DurableReservationError> {
    let metadata = file
        .metadata()
        .map_err(|_| DurableReservationError::InsecureStateFile)?;
    if !metadata.file_type().is_file()
        || metadata.uid() != geteuid().as_raw()
        || metadata.mode() & 0o7777 != 0o600
        || metadata.nlink() != 1
    {
        return Err(DurableReservationError::InsecureStateFile);
    }
    Ok(())
}

fn validate_lease_file(file: &File) -> Result<std::fs::Metadata, StateDirectoryLeaseError> {
    validate_state_file(file).map_err(|_| StateDirectoryLeaseError::UnsafeState)?;
    let metadata = file
        .metadata()
        .map_err(|_| StateDirectoryLeaseError::UnsafeState)?;
    if metadata.len() != 0 {
        return Err(StateDirectoryLeaseError::UnsafeState);
    }
    Ok(metadata)
}

#[cfg(test)]
mod bridge_state_lease_tests {
    use std::{
        fs,
        os::unix::fs::{PermissionsExt as _, symlink},
        path::Path,
    };

    use super::{BRIDGE_STATE_LEASE_FILENAME, StateDirectoryLease, StateDirectoryLeaseError};

    fn secure_directory() -> tempfile::TempDir {
        let directory = tempfile::tempdir().expect("temporary state directory");
        fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700))
            .expect("owner-only state directory");
        directory
    }

    fn assert_unsafe(path: &Path) {
        assert_eq!(
            StateDirectoryLease::acquire(path).expect_err("unsafe state must fail closed"),
            StateDirectoryLeaseError::UnsafeState
        );
    }

    #[test]
    fn bridge_state_lease_is_exclusive_and_release_allows_the_next_holder() {
        let directory = secure_directory();
        let first = StateDirectoryLease::acquire(directory.path()).expect("first lease holder");
        assert_eq!(
            StateDirectoryLease::acquire(directory.path())
                .expect_err("concurrent state owner must fail closed"),
            StateDirectoryLeaseError::AlreadyHeld
        );

        drop(first);
        StateDirectoryLease::acquire(directory.path()).expect("lease after release");
    }

    #[test]
    fn bridge_state_lease_rejects_unsafe_directory_and_lock_file_aliases() {
        let directory = secure_directory();
        fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o750))
            .expect("make state directory unsafe");
        assert_unsafe(directory.path());

        fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700))
            .expect("restore state directory");
        let alias = directory.path().with_extension("alias");
        symlink(directory.path(), &alias).expect("state directory alias");
        assert_unsafe(&alias);
        fs::remove_file(alias).expect("remove state alias");

        let lease_path = directory.path().join(BRIDGE_STATE_LEASE_FILENAME);
        fs::write(&lease_path, []).expect("existing lease file");
        fs::set_permissions(&lease_path, fs::Permissions::from_mode(0o640))
            .expect("make lease file unsafe");
        assert_unsafe(directory.path());

        fs::set_permissions(&lease_path, fs::Permissions::from_mode(0o600))
            .expect("restore lease file mode");
        let hard_link = directory.path().join("lease-hard-link");
        fs::hard_link(&lease_path, &hard_link).expect("hard-linked lease file");
        assert_unsafe(directory.path());
        fs::remove_file(hard_link).expect("remove hard link");
        fs::remove_file(&lease_path).expect("remove lease file");

        let target = directory.path().join("lease-target");
        fs::write(&target, []).expect("symlink target");
        fs::set_permissions(&target, fs::Permissions::from_mode(0o600))
            .expect("private symlink target");
        symlink(&target, &lease_path).expect("symlinked lease file");
        assert_unsafe(directory.path());
    }
}
