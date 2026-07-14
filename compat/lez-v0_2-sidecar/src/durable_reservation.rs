use std::{
    fmt,
    fs::File,
    io::{Read, Write},
    os::unix::fs::MetadataExt,
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
};

use rustix::{
    fs::{
        AtFlags, CWD, Dir, Mode, OFlags, RenameFlags, ResolveFlags, openat, openat2, renameat_with,
        unlinkat,
    },
    io::Errno,
    process::geteuid,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

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

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ReservationKind {
    NativeEscrow,
    VaultClaim,
}

impl ReservationKind {
    const fn filename(self) -> &'static str {
        match self {
            Self::NativeEscrow => "native-escrow-reservation.v1.json",
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

pub(crate) struct DurableReservationStore {
    directory: File,
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
        let directory = openat2(
            CWD,
            path,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
            ResolveFlags::NO_SYMLINKS,
        )
        .map(File::from)
        .map_err(|_| DurableReservationError::InsecureDirectory)?;
        validate_directory(&directory)?;
        Ok(Self { directory })
    }

    pub(crate) fn load<Request, Output>(
        &self,
        expected_kind: ReservationKind,
    ) -> Result<Option<(Request, Output)>, DurableReservationError>
    where
        Request: DeserializeOwned,
        Output: DeserializeOwned,
    {
        validate_directory(&self.directory)?;
        self.reject_partial(expected_kind)?;
        let descriptor = match openat(
            &self.directory,
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
        validate_directory(&self.directory)?;
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
        validate_directory(&self.directory)?;
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
            &self.directory,
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
                &self.directory,
                temporary_name.as_str(),
                &self.directory,
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
            validate_directory(&self.directory)?;
            self.directory
                .sync_all()
                .map_err(|_| DurableReservationError::Filesystem)?;
            validate_directory(&self.directory)?;
            Ok(())
        })();

        if write_result.is_err() {
            let _ = unlinkat(&self.directory, temporary_name.as_str(), AtFlags::empty());
            let _ = self.directory.sync_all();
        }
        write_result
    }

    fn reject_partial(&self, kind: ReservationKind) -> Result<(), DurableReservationError> {
        let prefix = kind.partial_prefix();
        let entries =
            Dir::read_from(&self.directory).map_err(|_| DurableReservationError::Filesystem)?;
        for entry in entries {
            let entry = entry.map_err(|_| DurableReservationError::Filesystem)?;
            if entry.file_name().to_bytes().starts_with(prefix.as_bytes()) {
                return Err(DurableReservationError::PartialReservation);
            }
        }
        Ok(())
    }
}

fn validate_directory(directory: &File) -> Result<(), DurableReservationError> {
    let metadata = directory
        .metadata()
        .map_err(|_| DurableReservationError::InsecureDirectory)?;
    if !metadata.file_type().is_dir()
        || metadata.uid() != geteuid().as_raw()
        || metadata.mode() & 0o7777 != 0o700
    {
        return Err(DurableReservationError::InsecureDirectory);
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
