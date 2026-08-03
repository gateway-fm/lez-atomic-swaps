//! Owner-private file loading shared by Maker application binaries.

use std::{fs::File, io::Read as _, os::unix::fs::MetadataExt as _, path::Path};

use anyhow::{Context as _, ensure};
use rustix::fs::{CWD, Mode, OFlags, ResolveFlags, openat2};
use secp256k1::SecretKey;
use zeroize::Zeroizing;

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
    let mut file = openat2(
        CWD,
        path,
        OFlags::RDONLY | OFlags::NONBLOCK | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
        ResolveFlags::NO_SYMLINKS,
    )
    .map(File::from)
    .with_context(|| format!("open {purpose}"))?;
    let before = validate_private_file(&file, maximum_bytes, purpose)?;
    let mut bytes = Zeroizing::new(Vec::new());
    std::io::Read::by_ref(&mut file)
        .take(maximum_bytes + 1)
        .read_to_end(&mut bytes)
        .with_context(|| format!("read {purpose}"))?;
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
    Ok(bytes)
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
