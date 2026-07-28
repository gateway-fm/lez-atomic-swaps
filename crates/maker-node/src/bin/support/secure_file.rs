use std::{fs::File, io::Read as _, os::unix::fs::MetadataExt as _, path::Path};

use anyhow::{Context as _, ensure};
use rustix::fs::{CWD, Mode, OFlags, ResolveFlags, openat2};
use zeroize::Zeroizing;

pub(crate) fn read_private_file(
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

pub(crate) fn load_raw_secret(path: &Path, purpose: &str) -> anyhow::Result<Zeroizing<[u8; 32]>> {
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
