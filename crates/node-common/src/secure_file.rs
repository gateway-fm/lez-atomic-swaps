//! Owner-private file loading shared by Maker application binaries.

use std::{
    ffi::OsStr,
    fs::File,
    io::Read as _,
    os::unix::fs::MetadataExt as _,
    path::{Component, Path},
};

use anyhow::{Context as _, ensure};
use lez_swap_store::open_no_symlinks;
use rustix::fs::{Mode, OFlags};
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
    let credential =
        is_systemd_credential_path(path, std::env::var_os("CREDENTIALS_DIRECTORY").as_deref());
    let mut file = open_private_file(path, purpose)?;
    let before = validate_private_file(&file, maximum_bytes, purpose, credential)?;
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
    let after = validate_private_file(&file, maximum_bytes, purpose, credential)?;
    ensure!(
        before.dev() == after.dev()
            && before.ino() == after.ino()
            && before.len() == after.len()
            && after.len() == bytes.len() as u64,
        "{purpose} changed while it was read"
    );
    let identity = private_file_identity(&after);
    let reopened = open_private_file(path, purpose)?;
    let rebound = validate_private_file(&reopened, maximum_bytes, purpose, credential)?;
    ensure!(
        private_file_identity(&rebound) == identity,
        "{purpose} path changed while it was read"
    );
    Ok(PrivateFileSnapshot { bytes, identity })
}

fn open_private_file(path: &Path, purpose: &str) -> anyhow::Result<File> {
    open_no_symlinks(path, OFlags::RDONLY | OFlags::NONBLOCK, Mode::empty())
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
    systemd_credential: bool,
) -> anyhow::Result<std::fs::Metadata> {
    let metadata = file
        .metadata()
        .with_context(|| format!("inspect {purpose}"))?;
    ensure!(
        metadata.file_type().is_file()
            && private_file_ownership_ok(
                metadata.uid(),
                metadata.gid(),
                metadata.mode() & 0o7777,
                rustix::process::geteuid().as_raw(),
                systemd_credential,
            )
            && metadata.nlink() == 1
            && metadata.len() <= maximum_bytes,
        "{purpose} must be an owner-owned, single-link mode-0400-or-0600 regular file within its size bound"
    );
    Ok(metadata)
}

/// Owned by the effective user with mode 0400 or 0600; or, for a file systemd hands the
/// service as a credential, owned by root:root and unwritable (0400, or 0440 when systemd
/// grants the service user read access through an ACL, whose mask shows as the group bits).
const fn private_file_ownership_ok(
    owner: u32,
    group: u32,
    mode: u32,
    effective_user: u32,
    systemd_credential: bool,
) -> bool {
    (owner == effective_user && matches!(mode, 0o400 | 0o600))
        || (systemd_credential && owner == 0 && group == 0 && matches!(mode, 0o400 | 0o440))
}

/// Whether `path` names an entry directly inside the systemd credential directory, given as
/// the service's `CREDENTIALS_DIRECTORY`, and that directory is root-owned and writable by
/// root alone. The comparison is by path components; the file itself is then opened with no
/// symbolic link anywhere in its path, so the entry really is in that directory, and only
/// root can have put it there.
fn is_systemd_credential_path(path: &Path, credentials_directory: Option<&OsStr>) -> bool {
    let Some(directory) = credentials_directory.map(Path::new) else {
        return false;
    };
    is_direct_child(path, directory)
        && open_no_symlinks(
            directory,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NONBLOCK,
            Mode::empty(),
        )
        .ok()
        .and_then(|directory| directory.metadata().ok())
        .is_some_and(|metadata| metadata.uid() == 0 && metadata.mode() & 0o022 == 0)
}

fn is_direct_child(path: &Path, directory: &Path) -> bool {
    directory.is_absolute()
        && !directory
            .components()
            .any(|component| component == Component::ParentDir)
        && path.parent() == Some(directory)
        && matches!(path.components().next_back(), Some(Component::Normal(_)))
}

#[cfg(test)]
mod tests {
    use std::{
        fs::{self, OpenOptions},
        io::Write as _,
        os::unix::fs::PermissionsExt as _,
        path::Path,
    };

    use super::{is_direct_child, private_file_ownership_ok, read_private_file_snapshot_inner};

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

    #[test]
    fn ownership_rule_is_unchanged_outside_the_credential_directory() {
        let service = 990;
        for credential in [false, true] {
            assert!(private_file_ownership_ok(
                service, service, 0o600, service, credential
            ));
            assert!(private_file_ownership_ok(
                service, service, 0o400, service, credential
            ));
            assert!(!private_file_ownership_ok(
                service, service, 0o640, service, credential
            ));
            assert!(!private_file_ownership_ok(
                service, service, 0o440, service, credential
            ));
            assert!(!private_file_ownership_ok(
                991, 991, 0o600, service, credential
            ));
        }
        assert!(!private_file_ownership_ok(0, 0, 0o400, service, false));
        assert!(!private_file_ownership_ok(0, 0, 0o440, service, false));
    }

    #[test]
    fn credential_rule_accepts_only_an_unwritable_root_owned_file() {
        let service = 990;
        assert!(private_file_ownership_ok(0, 0, 0o400, service, true));
        assert!(private_file_ownership_ok(0, 0, 0o440, service, true));
        for mode in [0o600, 0o640, 0o444, 0o460, 0o4400, 0o1440] {
            assert!(
                !private_file_ownership_ok(0, 0, mode, service, true),
                "{mode:o}"
            );
        }
        assert!(!private_file_ownership_ok(0, service, 0o440, service, true));
        assert!(!private_file_ownership_ok(991, 0, 0o400, service, true));
    }

    #[test]
    fn credential_path_must_sit_directly_in_the_named_directory() {
        let directory = Path::new("/run/credentials/lez-maker-node.service");
        assert!(is_direct_child(
            &directory.join("delivery-signing.key"),
            directory
        ));
        assert!(is_direct_child(
            Path::new("/run/credentials/lez-maker-node.service/./key"),
            directory
        ));
        for path in [
            "/run/credentials/lez-maker-node.service/sub/key",
            "/run/credentials/lez-maker-node.service/../other.service/key",
            "/run/credentials/lez-maker-node.service/..",
            "/run/credentials/lez-maker-node.service",
            "/etc/lez/maker/key",
            "run/credentials/lez-maker-node.service/key",
        ] {
            assert!(!is_direct_child(Path::new(path), directory), "{path}");
        }
        assert!(!is_direct_child(
            Path::new("run/credentials/x/key"),
            Path::new("run/credentials/x")
        ));
        assert!(!is_direct_child(
            Path::new("/run/credentials/x/../y/key"),
            Path::new("/run/credentials/x/../y")
        ));
    }

    #[test]
    fn credential_directory_must_be_owned_by_root() {
        let run = tempfile::tempdir().unwrap();
        let path = run.path().join("key");
        private_file(&path, &[7; 32]);
        // Only a root-owned directory is trusted, and a test run as root owns its tempdir.
        assert_eq!(
            super::is_systemd_credential_path(&path, Some(run.path().as_os_str())),
            rustix::process::geteuid().is_root()
        );
        assert!(!super::is_systemd_credential_path(&path, None));
    }

    fn private_file(path: &std::path::Path, bytes: &[u8]) {
        fs::write(path, bytes).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
    }
}
