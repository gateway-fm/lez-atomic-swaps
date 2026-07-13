use std::{
    fs,
    fs::File,
    io::Read as _,
    path::{Component, Path, PathBuf},
};

use zeroize::Zeroizing;

#[derive(Clone, Default, Eq, PartialEq)]
pub(crate) struct FileLocation {
    canonical: PathBuf,
    #[cfg(unix)]
    device: Option<u64>,
    #[cfg(unix)]
    inode: Option<u64>,
    #[cfg(unix)]
    length: Option<u64>,
    #[cfg(unix)]
    modified_seconds: Option<i64>,
    #[cfg(unix)]
    modified_nanoseconds: Option<i64>,
    #[cfg(unix)]
    changed_seconds: Option<i64>,
    #[cfg(unix)]
    changed_nanoseconds: Option<i64>,
}

impl FileLocation {
    pub(crate) fn aliases(&self, other: &Self) -> bool {
        if self.canonical == other.canonical {
            return true;
        }
        #[cfg(unix)]
        if let (Some(left_device), Some(left_inode), Some(right_device), Some(right_inode)) =
            (self.device, self.inode, other.device, other.inode)
        {
            return left_device == right_device && left_inode == right_inode;
        }
        false
    }

    pub(crate) fn unchanged(&self, current: &Self) -> bool {
        if self.canonical != current.canonical {
            return false;
        }
        #[cfg(unix)]
        if let (Some(expected_device), Some(expected_inode)) = (self.device, self.inode) {
            return current.device == Some(expected_device)
                && current.inode == Some(expected_inode)
                && current.length == self.length
                && current.modified_seconds == self.modified_seconds
                && current.modified_nanoseconds == self.modified_nanoseconds
                && current.changed_seconds == self.changed_seconds
                && current.changed_nanoseconds == self.changed_nanoseconds;
        }
        true
    }

    fn existing(canonical: PathBuf, metadata: &fs::Metadata) -> Self {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt as _;

            Self {
                canonical,
                device: Some(metadata.dev()),
                inode: Some(metadata.ino()),
                length: Some(metadata.len()),
                modified_seconds: Some(metadata.mtime()),
                modified_nanoseconds: Some(metadata.mtime_nsec()),
                changed_seconds: Some(metadata.ctime()),
                changed_nanoseconds: Some(metadata.ctime_nsec()),
            }
        }
        #[cfg(not(unix))]
        Self { canonical }
    }

    fn missing(canonical: PathBuf) -> Self {
        Self {
            canonical,
            #[cfg(unix)]
            device: None,
            #[cfg(unix)]
            inode: None,
            #[cfg(unix)]
            length: None,
            #[cfg(unix)]
            modified_seconds: None,
            #[cfg(unix)]
            modified_nanoseconds: None,
            #[cfg(unix)]
            changed_seconds: None,
            #[cfg(unix)]
            changed_nanoseconds: None,
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) enum FilePrivacy {
    Public,
    OwnerPrivate,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SecureFileError {
    Unavailable,
    Unsafe,
}

pub(crate) fn read_bounded_identified(
    path: &Path,
    maximum: usize,
    privacy: FilePrivacy,
) -> Result<(Zeroizing<Vec<u8>>, FileLocation), SecureFileError> {
    let before = fs::symlink_metadata(path).map_err(|_| SecureFileError::Unavailable)?;
    validate_metadata(&before, maximum, privacy)?;

    let file = File::open(path).map_err(|_| SecureFileError::Unavailable)?;
    let opened = file.metadata().map_err(|_| SecureFileError::Unavailable)?;
    validate_metadata(&opened, maximum, privacy)?;
    let canonical = fs::canonicalize(path).map_err(|_| SecureFileError::Unavailable)?;
    let before_location = FileLocation::existing(canonical.clone(), &before);
    let opened_location = FileLocation::existing(canonical.clone(), &opened);
    if !before_location.unchanged(&opened_location) {
        return Err(SecureFileError::Unsafe);
    }

    let maximum_u64 = u64::try_from(maximum).map_err(|_| SecureFileError::Unsafe)?;
    let mut bytes = Zeroizing::new(Vec::with_capacity(maximum.saturating_add(1)));
    file.take(maximum_u64.saturating_add(1))
        .read_to_end(bytes.as_mut())
        .map_err(|_| SecureFileError::Unavailable)?;
    if bytes.is_empty() || bytes.len() > maximum {
        return Err(SecureFileError::Unsafe);
    }

    let after = fs::symlink_metadata(path).map_err(|_| SecureFileError::Unavailable)?;
    validate_metadata(&after, maximum, privacy)?;
    let after_location = FileLocation::existing(canonical, &after);
    if !opened_location.unchanged(&after_location) {
        return Err(SecureFileError::Unsafe);
    }
    Ok((bytes, opened_location))
}

pub(crate) fn canonical_location(path: &Path) -> Result<FileLocation, SecureFileError> {
    let normalized = normalized_absolute(path)?;
    match fs::symlink_metadata(path) {
        Ok(before) => {
            if !before.file_type().is_file() {
                return Err(SecureFileError::Unsafe);
            }
            let canonical = fs::canonicalize(path).map_err(|_| SecureFileError::Unavailable)?;
            let after = fs::symlink_metadata(path).map_err(|_| SecureFileError::Unavailable)?;
            if !after.file_type().is_file() {
                return Err(SecureFileError::Unsafe);
            }
            let expected = FileLocation::existing(canonical.clone(), &before);
            let current = FileLocation::existing(canonical, &after);
            if !expected.unchanged(&current) {
                return Err(SecureFileError::Unsafe);
            }
            Ok(current)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(FileLocation::missing(normalized))
        }
        Err(_) => Err(SecureFileError::Unavailable),
    }
}

fn normalized_absolute(path: &Path) -> Result<PathBuf, SecureFileError> {
    if !path.is_absolute() {
        return Err(SecureFileError::Unsafe);
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt as _;

        let raw = path.as_os_str().as_bytes();
        if !raw.starts_with(b"/")
            || raw.len() == 1
            || raw[1..]
                .split(|byte| *byte == b'/')
                .any(|segment| segment.is_empty() || segment == b"." || segment == b"..")
        {
            return Err(SecureFileError::Unsafe);
        }
    }
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::RootDir => normalized.push(Path::new("/")),
            Component::Normal(part) => normalized.push(part),
            Component::CurDir | Component::ParentDir | Component::Prefix(_) => {
                return Err(SecureFileError::Unsafe);
            }
        }
    }
    if normalized != path {
        return Err(SecureFileError::Unsafe);
    }
    Ok(normalized)
}

fn validate_metadata(
    metadata: &fs::Metadata,
    maximum: usize,
    privacy: FilePrivacy,
) -> Result<(), SecureFileError> {
    let maximum = u64::try_from(maximum).map_err(|_| SecureFileError::Unsafe)?;
    if !metadata.file_type().is_file() || metadata.len() == 0 || metadata.len() > maximum {
        return Err(SecureFileError::Unsafe);
    }
    #[cfg(unix)]
    if matches!(privacy, FilePrivacy::OwnerPrivate) {
        use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};

        if metadata.permissions().mode() & 0o7777 != 0o600 || metadata.nlink() != 1 {
            return Err(SecureFileError::Unsafe);
        }
    }
    Ok(())
}
