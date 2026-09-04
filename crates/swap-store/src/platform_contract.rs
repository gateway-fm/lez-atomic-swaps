//! Compile-time contract for Linux kernel-backed authority boundaries.
//!
//! Actor configuration sealing uses `memfd_create` plus seals and private file
//! creation uses `openat2` resolution constraints. Portable fallbacks would
//! silently weaken those invariants, so the runtime package is Linux-only.

use std::{
    fs::{File, Metadata},
    os::unix::fs::MetadataExt as _,
    path::Path,
};

use rustix::fs::{CWD, Mode, OFlags, ResolveFlags, openat2};

/// Opens `path` without following a symlink in any component and without
/// leaking the descriptor across `exec`.
///
/// Callers pass only the access and creation flags they need and keep their
/// own error mapping; the resolution policy is what every owner-private file
/// in the runtime shares.
///
/// # Errors
///
/// Returns the raw `openat2` errno so callers can still distinguish `EXIST`,
/// `LOOP`, and `NOENT`.
pub fn open_no_symlinks(path: &Path, flags: OFlags, mode: Mode) -> rustix::io::Result<File> {
    openat2(
        CWD,
        path,
        flags | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        mode,
        ResolveFlags::NO_SYMLINKS,
    )
    .map(File::from)
}

/// Whether `metadata` describes a regular file only the current user can
/// touch: owned by the effective user, exactly `mode`, and hard-linked once.
#[must_use]
pub fn is_owner_private_regular_file(metadata: &Metadata, mode: u32) -> bool {
    metadata.file_type().is_file()
        && metadata.uid() == rustix::process::geteuid().as_raw()
        && metadata.mode() & 0o7777 == mode
        && metadata.nlink() == 1
}
