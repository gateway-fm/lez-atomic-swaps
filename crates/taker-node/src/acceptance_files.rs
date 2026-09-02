//! Owner-private acceptance and receipt file publishing shared by every pair's
//! Taker take path: exact no-clobber writes, normalized private paths, and the
//! replay summary each acceptance prints.

use std::{
    fs::{self, File},
    io::Write as _,
    os::unix::fs::{MetadataExt as _, PermissionsExt as _},
    path::{Component, Path, PathBuf},
};

use anyhow::{Context as _, ensure};
use serde::Serialize;
use tempfile::NamedTempFile;

use crate::secure_file::read_private_file;

/// Maximum bytes accepted for one owner-private acceptance receipt.
pub const MAX_TAKER_RECEIPT_BYTES: u64 = 16 * 1024;

#[derive(Debug, Serialize)]
pub struct ReplayOutput {
    pub proposal: bool,
    pub completion: bool,
    pub agreement_file: bool,
}

pub fn decode_sha256(value: &str, label: &str) -> anyhow::Result<[u8; 32]> {
    let decoded = hex::decode(value).with_context(|| format!("decode receipt {label} digest"))?;
    decoded
        .try_into()
        .map_err(|_| anyhow::anyhow!("receipt {label} digest has the wrong length"))
}

pub fn resolved_new_path(path: &Path, label: &str) -> anyhow::Result<PathBuf> {
    ensure!(
        normalized_absolute(path),
        "{label} path must be normalized and absolute"
    );
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .with_context(|| format!("{label} path needs a parent directory"))?;
    let file_name = path
        .file_name()
        .with_context(|| format!("{label} path needs a file name"))?;
    let parent_metadata =
        fs::symlink_metadata(parent).with_context(|| format!("inspect {label} parent"))?;
    ensure!(
        parent_metadata.file_type().is_dir()
            && parent_metadata.uid() == rustix::process::geteuid().as_raw()
            && parent_metadata.permissions().mode() & 0o7777 == 0o700,
        "{label} parent must be an owner-owned mode-0700 real directory"
    );
    Ok(fs::canonicalize(parent)
        .with_context(|| format!("resolve {label} parent"))?
        .join(file_name))
}

#[must_use]
pub fn normalized_absolute(path: &Path) -> bool {
    path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, Component::RootDir | Component::Normal(_)))
}

pub fn publish_exact_new(
    path: &Path,
    bytes: &[u8],
    max_bytes: u64,
    label: &'static str,
) -> anyhow::Result<bool> {
    ensure!(path.is_absolute(), "{label} path must be absolute");
    match fs::symlink_metadata(path) {
        Ok(_) => return validate_existing_output(path, bytes, max_bytes, label).map(|()| true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error).with_context(|| format!("inspect {label} path")),
    }
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .with_context(|| format!("{label} needs a parent directory"))?;
    let parent_metadata =
        fs::symlink_metadata(parent).with_context(|| format!("inspect {label} parent"))?;
    ensure!(
        parent_metadata.file_type().is_dir()
            && parent_metadata.uid() == rustix::process::geteuid().as_raw()
            && parent_metadata.permissions().mode() & 0o7777 == 0o700,
        "{label} parent must be an owner-owned mode-0700 real directory"
    );
    let mut temporary =
        NamedTempFile::new_in(parent).with_context(|| format!("create temporary {label}"))?;
    temporary
        .as_file_mut()
        .write_all(bytes)
        .with_context(|| format!("write temporary {label}"))?;
    temporary
        .as_file_mut()
        .sync_all()
        .with_context(|| format!("sync temporary {label}"))?;
    match temporary.persist_noclobber(path) {
        Ok(file) => {
            file.sync_all()
                .with_context(|| format!("sync persisted {label}"))?;
            File::open(parent)
                .and_then(|directory| directory.sync_all())
                .with_context(|| format!("sync {label} directory"))?;
            validate_existing_output(path, bytes, max_bytes, label)?;
            Ok(false)
        }
        Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {
            validate_existing_output(path, bytes, max_bytes, label)?;
            Ok(true)
        }
        Err(error) => Err(error.error).with_context(|| format!("publish {label} without clobber")),
    }
}

fn validate_existing_output(
    path: &Path,
    expected: &[u8],
    max_bytes: u64,
    label: &'static str,
) -> anyhow::Result<()> {
    let actual = read_private_file(path, max_bytes, label)?;
    ensure!(
        actual.as_slice() == expected,
        "{label} already exists with different bytes"
    );
    Ok(())
}
