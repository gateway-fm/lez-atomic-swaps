//! Containment checks for the paths a role builds on disk.
//!
//! A swap's files live under a root the operator configures, in directories
//! and files named by ids (reservation ids, derived request ids, digests) or
//! by constants. The ids are validated by their types before they reach a
//! path; these checks repeat that at the point where a name is joined onto
//! its root or a whole configured path is opened, so a name can never walk
//! out of its root even if a type's grammar is widened later.

use std::{
    ffi::OsStr,
    path::{Component, Path, PathBuf},
};

use thiserror::Error;

/// Why a name or a path was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum PathSafetyError {
    /// The name is empty.
    #[error("the path component is empty")]
    Empty,
    /// The name holds a separator (`/` or `\`), a NUL or a drive colon.
    #[error("the path component holds a separator, NUL or drive prefix")]
    ForbiddenCharacter,
    /// The name is not exactly one plain component (`.`, `..`, a root or
    /// prefix).
    #[error("the path component is not one plain name")]
    NotSingleComponent,
    /// The joined path does not stay directly under its root.
    #[error("the joined path leaves its root")]
    Escapes,
    /// The path is relative.
    #[error("the path is not absolute")]
    NotAbsolute,
    /// The path holds a `.` or `..` component.
    #[error("the path holds a `.` or `..` component")]
    NotNormalized,
}

/// Checks that `component` is exactly one plain path component: not empty,
/// not `.` or `..`, with no `/`, `\`, NUL or `:` (a drive prefix elsewhere),
/// and parsed as a single [`Component::Normal`] equal to the whole input.
///
/// # Errors
///
/// Fails with the reason the name is refused.
pub fn check_component(component: &str) -> Result<&str, PathSafetyError> {
    if component.is_empty() {
        return Err(PathSafetyError::Empty);
    }
    if component
        .chars()
        .any(|character| matches!(character, '/' | '\\' | '\0' | ':'))
    {
        return Err(PathSafetyError::ForbiddenCharacter);
    }
    let mut components = Path::new(component).components();
    match (components.next(), components.next()) {
        (Some(Component::Normal(name)), None) if name == OsStr::new(component) => Ok(component),
        _ => Err(PathSafetyError::NotSingleComponent),
    }
}

/// `root.join(component)` for one checked plain name ([`check_component`]),
/// verified to stay directly under `root`.
///
/// # Errors
///
/// Fails when `component` is not one plain name, or the joined path is not
/// a direct child of `root`.
pub fn join_component(root: &Path, component: &str) -> Result<PathBuf, PathSafetyError> {
    let component = check_component(component)?;
    let joined = root.join(component);
    if joined.starts_with(root) && joined.parent() == Some(root) {
        Ok(joined)
    } else {
        Err(PathSafetyError::Escapes)
    }
}

/// Checks that a whole path (one the operator configures, or one a Node
/// stored and reads back) is absolute and normalised: its components are
/// the root and plain names only, no `.` or `..`.
///
/// # Errors
///
/// Fails when `path` is relative or holds a `.` or `..` component.
pub fn check_normalized_absolute(path: &Path) -> Result<&Path, PathSafetyError> {
    if !path.is_absolute() {
        return Err(PathSafetyError::NotAbsolute);
    }
    if path
        .components()
        .all(|component| matches!(component, Component::RootDir | Component::Normal(_)))
    {
        Ok(path)
    } else {
        Err(PathSafetyError::NotNormalized)
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::{PathSafetyError, check_component, check_normalized_absolute, join_component};

    #[test]
    fn plain_names_join_directly_under_their_root() {
        let root = Path::new("/var/lib/lez/swaps");
        for name in [
            "reservation-0001",
            "a1b2c3d4e5f6",
            "zec-taker-prepared.json",
            ".stage",
            "round-xmr-r1.json",
            "........",
            "name with space",
        ] {
            assert_eq!(join_component(root, name), Ok(root.join(name)), "{name}");
        }
        assert_eq!(
            join_component(Path::new("relative/root"), "file"),
            Ok(PathBuf::from("relative/root/file"))
        );
    }

    #[test]
    fn hostile_names_are_refused() {
        let root = Path::new("/var/lib/lez/swaps");
        for (name, error) in [
            ("", PathSafetyError::Empty),
            (".", PathSafetyError::NotSingleComponent),
            ("..", PathSafetyError::NotSingleComponent),
            ("../x", PathSafetyError::ForbiddenCharacter),
            ("..\\x", PathSafetyError::ForbiddenCharacter),
            ("/abs", PathSafetyError::ForbiddenCharacter),
            ("/", PathSafetyError::ForbiddenCharacter),
            ("a/b", PathSafetyError::ForbiddenCharacter),
            ("a/", PathSafetyError::ForbiddenCharacter),
            ("a\\b", PathSafetyError::ForbiddenCharacter),
            ("a\0b", PathSafetyError::ForbiddenCharacter),
            ("\0", PathSafetyError::ForbiddenCharacter),
            ("C:", PathSafetyError::ForbiddenCharacter),
            ("C:\\Windows", PathSafetyError::ForbiddenCharacter),
            ("C:x", PathSafetyError::ForbiddenCharacter),
            ("\\\\server\\share", PathSafetyError::ForbiddenCharacter),
        ] {
            assert_eq!(check_component(name), Err(error), "{name:?}");
            assert_eq!(join_component(root, name), Err(error), "{name:?}");
        }
    }

    #[test]
    fn whole_paths_must_be_absolute_and_normalised() {
        for path in ["/", "/var/lib/lez/swaps", "/var/lib/lez/./swaps", "/a//b/"] {
            assert!(check_normalized_absolute(Path::new(path)).is_ok(), "{path}");
        }
        for (path, error) in [
            ("", PathSafetyError::NotAbsolute),
            ("relative/file", PathSafetyError::NotAbsolute),
            ("./file", PathSafetyError::NotAbsolute),
            ("../file", PathSafetyError::NotAbsolute),
            ("/var/lib/../etc/passwd", PathSafetyError::NotNormalized),
            ("/..", PathSafetyError::NotNormalized),
            ("/var/lib/lez/..", PathSafetyError::NotNormalized),
        ] {
            assert_eq!(
                check_normalized_absolute(Path::new(path)),
                Err(error),
                "{path}"
            );
        }
    }
}
