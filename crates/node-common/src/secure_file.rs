//! Owner-private file loading shared by Maker application binaries.

use std::{fs::File, io::Read as _, os::unix::fs::MetadataExt as _, path::Path};

use anyhow::{Context as _, ensure};
use rustix::fs::{CWD, Mode, OFlags, ResolveFlags, openat2};
use secp256k1::SecretKey;
use zeroize::Zeroizing;

/// Stable identity captured for one owner-private file snapshot.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PrivateFileIdentity {
    device: u64,
    inode: u64,
    length: u64,
}

impl PrivateFileIdentity {
    /// Device containing the exact file descriptor that supplied the bytes.
    #[must_use]
    pub const fn device(self) -> u64 {
        self.device
    }

    /// Inode of the exact file descriptor that supplied the bytes.
    #[must_use]
    pub const fn inode(self) -> u64 {
        self.inode
    }

    /// Stable byte length observed before and after reading the descriptor.
    #[must_use]
    pub const fn length(self) -> u64 {
        self.length
    }
}

/// Zeroizing private bytes bound to their stable same-descriptor identity.
pub struct PrivateFileSnapshot {
    bytes: Zeroizing<Vec<u8>>,
    identity: PrivateFileIdentity,
}

impl PrivateFileSnapshot {
    /// Borrows the bounded private bytes.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Returns the stable identity of the descriptor that supplied the bytes.
    #[must_use]
    pub const fn identity(&self) -> PrivateFileIdentity {
        self.identity
    }

    /// Consumes the snapshot while preserving zeroization of the returned bytes.
    #[must_use]
    pub fn into_bytes(self) -> Zeroizing<Vec<u8>> {
        self.bytes
    }
}

impl std::fmt::Debug for PrivateFileSnapshot {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PrivateFileSnapshot")
            .field("bytes", &"[REDACTED]")
            .field("identity", &self.identity)
            .finish()
    }
}

/// Reads bounded private bytes together with stable same-descriptor identity.
///
/// The path is opened without following symbolic links. Device, inode, and
/// length are captured and revalidated on that exact open descriptor before a
/// final path re-open rejects replacement during the read.
///
/// # Errors
///
/// Returns an error when the path cannot be opened safely, the file is not an
/// owner-private single-link regular file, its contents exceed `maximum_bytes`,
/// or its descriptor identity or path binding changes during the read.
pub fn read_private_file_snapshot(
    path: &Path,
    maximum_bytes: u64,
    purpose: &str,
) -> anyhow::Result<PrivateFileSnapshot> {
    read_private_file_snapshot_inner(path, maximum_bytes, purpose, || {})
}

fn read_private_file_snapshot_inner(
    path: &Path,
    maximum_bytes: u64,
    purpose: &str,
    after_read: impl FnOnce(),
) -> anyhow::Result<PrivateFileSnapshot> {
    let mut file = open_private_file(path, purpose)?;
    let before = validate_private_file(&file, maximum_bytes, purpose)?;
    let mut bytes = Zeroizing::new(Vec::new());
    std::io::Read::by_ref(&mut file)
        .take(maximum_bytes + 1)
        .read_to_end(&mut bytes)
        .with_context(|| format!("read {purpose}"))?;
    after_read();
    ensure!(
        bytes.len() as u64 <= maximum_bytes,
        "{purpose} is oversized"
    );
    let after = validate_private_file(&file, maximum_bytes, purpose)?;
    ensure!(
        before.dev() == after.dev()
            && before.ino() == after.ino()
            && before.len() == after.len()
            && after.len() == bytes.len() as u64,
        "{purpose} changed while it was read"
    );
    let identity = private_file_identity(&after);
    let reopened = open_private_file(path, purpose)?;
    let rebound = validate_private_file(&reopened, maximum_bytes, purpose)?;
    ensure!(
        private_file_identity(&rebound) == identity,
        "{purpose} path changed while it was read"
    );
    Ok(PrivateFileSnapshot { bytes, identity })
}

fn open_private_file(path: &Path, purpose: &str) -> anyhow::Result<File> {
    openat2(
        CWD,
        path,
        OFlags::RDONLY | OFlags::NONBLOCK | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
        ResolveFlags::NO_SYMLINKS,
    )
    .map(File::from)
    .with_context(|| format!("open {purpose}"))
}

fn private_file_identity(metadata: &std::fs::Metadata) -> PrivateFileIdentity {
    PrivateFileIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
        length: metadata.len(),
    }
}

/// Reads one bounded owner-private regular file without following symbolic links.
///
/// # Errors
///
/// Returns an error when the path cannot be opened safely, the file metadata is
/// not private and stable, or its contents exceed `maximum_bytes`.
pub fn read_private_file(
    path: &Path,
    maximum_bytes: u64,
    purpose: &str,
) -> anyhow::Result<Zeroizing<Vec<u8>>> {
    Ok(read_private_file_snapshot(path, maximum_bytes, purpose)?.into_bytes())
}

/// Loads one nonzero raw 32-byte secret from an owner-private file.
///
/// # Errors
///
/// Returns an error when [`read_private_file`] rejects the file or its contents
/// are not exactly 32 bytes or are all zero.
pub fn load_raw_secret(path: &Path, purpose: &str) -> anyhow::Result<Zeroizing<[u8; 32]>> {
    let bytes = read_private_file(path, 32, purpose)?;
    ensure!(
        bytes.len() == 32,
        "{purpose} must contain exactly 32 raw bytes"
    );
    let mut secret = Zeroizing::new([0_u8; 32]);
    secret.copy_from_slice(&bytes);
    ensure!(
        secret.iter().any(|byte| *byte != 0),
        "{purpose} must be nonzero"
    );
    Ok(secret)
}

/// Loads a secp256k1 secret encoded as 32 raw bytes or 64 hexadecimal digits.
///
/// # Errors
///
/// Returns an error when [`read_private_file`] rejects the file or its contents
/// do not encode a valid secp256k1 secret key in either accepted representation.
pub fn load_secp256k1_secret(path: &Path, purpose: &str) -> anyhow::Result<SecretKey> {
    let encoded = read_private_file(path, 65, purpose)?;
    if encoded.len() == 32 {
        return SecretKey::from_slice(encoded.as_slice())
            .with_context(|| format!("validate {purpose}"));
    }
    let text = std::str::from_utf8(&encoded)
        .with_context(|| format!("{purpose} must be raw bytes or UTF-8 hex"))?
        .trim();
    ensure!(
        text.len() == 64,
        "{purpose} must contain exactly 32 raw bytes or 32 bytes as hex"
    );
    let mut bytes = Zeroizing::new([0_u8; 32]);
    hex::decode_to_slice(text, bytes.as_mut()).with_context(|| format!("decode {purpose}"))?;
    SecretKey::from_slice(bytes.as_ref()).with_context(|| format!("validate {purpose}"))
}

fn validate_private_file(
    file: &File,
    maximum_bytes: u64,
    purpose: &str,
) -> anyhow::Result<std::fs::Metadata> {
    let metadata = file
        .metadata()
        .with_context(|| format!("inspect {purpose}"))?;
    let mode = metadata.mode() & 0o7777;
    ensure!(
        metadata.file_type().is_file()
            && metadata.uid() == rustix::process::geteuid().as_raw()
            && matches!(mode, 0o400 | 0o600)
            && metadata.nlink() == 1
            && metadata.len() <= maximum_bytes,
        "{purpose} must be an owner-owned, single-link mode-0400-or-0600 regular file within its size bound"
    );
    Ok(metadata)
}

#[cfg(test)]
mod tests {
    use std::{
        fs::{self, OpenOptions},
        io::Write as _,
        os::unix::fs::PermissionsExt as _,
    };

    use super::read_private_file_snapshot_inner;

    #[test]
    fn snapshot_rejects_length_mutation_after_reading_same_descriptor() {
        let run = tempfile::tempdir().unwrap();
        let path = run.path().join("mutated.bin");
        private_file(&path, b"before");

        let result = read_private_file_snapshot_inner(&path, 64, "test authority", || {
            OpenOptions::new()
                .append(true)
                .open(&path)
                .unwrap()
                .write_all(b"-after")
                .unwrap();
        });

        let error = result.unwrap_err().to_string();
        assert!(error.contains("changed while it was read"));
        assert!(!error.contains(path.to_str().unwrap()));
        assert!(!error.contains("before"));
    }

    #[test]
    fn snapshot_rejects_path_replacement_after_reading_same_descriptor() {
        let run = tempfile::tempdir().unwrap();
        let path = run.path().join("replaced.bin");
        let retired = run.path().join("retired.bin");
        private_file(&path, b"same-length");

        let result = read_private_file_snapshot_inner(&path, 64, "test authority", || {
            fs::rename(&path, &retired).unwrap();
            private_file(&path, b"same-length");
        });

        let error = result.unwrap_err().to_string();
        assert!(error.contains("path changed while it was read"));
        assert!(!error.contains(path.to_str().unwrap()));
        assert!(!error.contains("same-length"));
    }

    fn private_file(path: &std::path::Path, bytes: &[u8]) {
        fs::write(path, bytes).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
    }
}
