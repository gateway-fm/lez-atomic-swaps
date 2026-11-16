//! Compile-time contract for kernel-backed authority boundaries.
//!
//! On Linux, actor configuration sealing uses `memfd_create` plus seals and
//! private file creation uses `openat2` resolution constraints. macOS has
//! `O_NOFOLLOW_ANY` for the same path-resolution rule, but no sealed anonymous
//! files and no execution from a descriptor; `maker_actor_process` states what
//! replaces them there and what is weaker. No other platform is supported:
//! a silent portable fallback would weaken these invariants unannounced.

use std::{
    fs::{File, Metadata},
    os::unix::fs::MetadataExt as _,
    path::Path,
};

#[cfg(target_os = "macos")]
use rustix::fs::openat;
use rustix::fs::{CWD, Mode, OFlags};
#[cfg(target_os = "linux")]
use rustix::fs::{ResolveFlags, openat2};

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
#[cfg(target_os = "linux")]
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

/// The macOS form of the same rule: `O_NOFOLLOW_ANY` refuses a symlink in any
/// component of the path, the last one included, as `RESOLVE_NO_SYMLINKS`
/// plus `O_NOFOLLOW` do on Linux. The kernel rejects `O_NOFOLLOW` combined
/// with it (`EINVAL`), so a caller's `O_NOFOLLOW` is folded into it.
///
/// # Errors
///
/// Returns the raw `openat` errno; a symlink anywhere in the path is `ELOOP`.
#[cfg(target_os = "macos")]
pub fn open_no_symlinks(path: &Path, flags: OFlags, mode: Mode) -> rustix::io::Result<File> {
    openat(
        CWD,
        path,
        flags.difference(OFlags::NOFOLLOW) | OFlags::NOFOLLOW_ANY | OFlags::CLOEXEC,
        mode,
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

#[cfg(all(test, target_os = "macos"))]
mod macos_tests {
    use std::os::unix::fs::symlink;

    use rustix::io::Errno;

    use super::*;

    fn private_root() -> (tempfile::TempDir, std::path::PathBuf) {
        let directory = tempfile::tempdir().expect("temporary root");
        // The temporary directory lives under /var, itself a symlink.
        let root = std::fs::canonicalize(directory.path()).expect("canonical root");
        (directory, root)
    }

    #[test]
    fn open_no_symlinks_refuses_a_symlink_in_any_component() {
        let (_guard, root) = private_root();
        std::fs::create_dir(root.join("real")).expect("real directory");
        std::fs::write(root.join("real/file"), b"bytes").expect("real file");
        symlink(root.join("real"), root.join("alias")).expect("directory symlink");
        symlink(root.join("real/file"), root.join("file-alias")).expect("file symlink");

        assert!(open_no_symlinks(&root.join("real/file"), OFlags::RDONLY, Mode::empty()).is_ok());
        for refused in [root.join("alias/file"), root.join("file-alias")] {
            assert_eq!(
                open_no_symlinks(&refused, OFlags::RDONLY, Mode::empty()).err(),
                Some(Errno::LOOP),
                "{} must be refused",
                refused.display()
            );
        }
        assert_eq!(
            open_no_symlinks(
                &root.join("alias/created"),
                OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL,
                Mode::RUSR | Mode::WUSR,
            )
            .err(),
            Some(Errno::LOOP)
        );
        assert!(!root.join("real/created").exists());
    }

    #[test]
    fn open_no_symlinks_refuses_the_uncanonical_temporary_directory() {
        let directory = tempfile::tempdir().expect("temporary root");
        std::fs::write(directory.path().join("file"), b"bytes").expect("file");
        let canonical = std::fs::canonicalize(directory.path()).expect("canonical root");
        if canonical == directory.path() {
            return;
        }
        assert_eq!(
            open_no_symlinks(
                &directory.path().join("file"),
                OFlags::RDONLY,
                Mode::empty()
            )
            .err(),
            Some(Errno::LOOP)
        );
        assert!(open_no_symlinks(&canonical.join("file"), OFlags::RDONLY, Mode::empty()).is_ok());
    }

    #[test]
    fn opened_descriptors_do_not_survive_exec() {
        let (_guard, root) = private_root();
        std::fs::write(root.join("file"), b"bytes").expect("file");
        let file =
            open_no_symlinks(&root.join("file"), OFlags::RDONLY, Mode::empty()).expect("open file");
        let flags = rustix::io::fcntl_getfd(&file).expect("descriptor flags");
        assert!(flags.contains(rustix::io::FdFlags::CLOEXEC));
    }
}
