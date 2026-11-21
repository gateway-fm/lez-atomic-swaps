//! The one rule under which a key file may be the credential systemd hands a service.
//!
//! systemd (`LoadCredential=`, `LoadCredentialEncrypted=`) places each credential in the
//! directory named by `$CREDENTIALS_DIRECTORY`, owned by root:root with mode 0400, or 0440
//! when it grants the service user read access through an ACL (whose mask shows as the group
//! bits). Every reader of a private key accepts that file in addition to its own owner rule,
//! and only there.

use std::{
    ffi::OsStr,
    os::unix::fs::MetadataExt as _,
    path::{Component, Path},
};

/// The environment variable systemd sets to the service's credential directory.
pub const CREDENTIALS_DIRECTORY: &str = "CREDENTIALS_DIRECTORY";

/// Whether a file's owner, group and permission bits (`mode & 0o7777`) are those of a
/// systemd credential: root:root and unwritable, mode 0400 or 0440.
#[must_use]
pub const fn credential_ownership_ok(owner: u32, group: u32, mode: u32) -> bool {
    owner == 0 && group == 0 && matches!(mode, 0o400 | 0o440)
}

/// Whether `path` names an entry directly inside this service's credential directory (the
/// process's `CREDENTIALS_DIRECTORY`), and that directory is root-owned and writable by root
/// alone, reached with no symbolic link anywhere in its path. The file's own path then has no
/// symbolic link before its last component, and only root can have put it there.
#[must_use]
pub fn is_credential_path(path: &Path) -> bool {
    is_credential_path_in(path, std::env::var_os(CREDENTIALS_DIRECTORY).as_deref())
}

/// [`is_credential_path`] with the credential directory given.
#[must_use]
pub fn is_credential_path_in(path: &Path, credentials_directory: Option<&OsStr>) -> bool {
    let Some(directory) = credentials_directory.map(Path::new) else {
        return false;
    };
    is_direct_child(path, directory)
        && open_directory_no_symlinks(directory)
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

#[cfg(target_os = "linux")]
fn open_directory_no_symlinks(directory: &Path) -> Option<std::fs::Metadata> {
    use rustix::fs::{CWD, Mode, OFlags, ResolveFlags, openat2};
    let descriptor = openat2(
        CWD,
        directory,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
        ResolveFlags::NO_SYMLINKS,
    )
    .ok()?;
    std::fs::File::from(descriptor).metadata().ok()
}

#[cfg(not(target_os = "linux"))]
fn open_directory_no_symlinks(_directory: &Path) -> Option<std::fs::Metadata> {
    None
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{credential_ownership_ok, is_credential_path_in, is_direct_child};

    #[test]
    fn only_an_unwritable_root_owned_file_is_a_credential() {
        assert!(credential_ownership_ok(0, 0, 0o400));
        assert!(credential_ownership_ok(0, 0, 0o440));
        for mode in [0o600, 0o640, 0o444, 0o460, 0o4400, 0o1440] {
            assert!(!credential_ownership_ok(0, 0, mode), "{mode:o}");
        }
        assert!(!credential_ownership_ok(0, 990, 0o440));
        assert!(!credential_ownership_ok(990, 0, 0o400));
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
        // Only a root-owned directory is trusted, and a test run as root owns its tempdir.
        assert_eq!(
            is_credential_path_in(&path, Some(run.path().as_os_str())),
            rustix::process::geteuid().is_root()
        );
        assert!(!is_credential_path_in(&path, None));
    }
}
