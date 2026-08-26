//! Reusable owner-local Unix RPC server configuration and socket ownership.

use std::{
    fs, io,
    os::unix::fs::{FileTypeExt as _, MetadataExt as _, PermissionsExt as _},
    path::{Path, PathBuf},
};

use anyhow::{Context as _, bail, ensure};
use jsonrpsee::server::{BatchRequestConfig, ServerConfig};
use tokio::net::UnixListener;

const MAXIMUM_CONNECTIONS: u32 = 16;

/// Builds the bounded HTTP-only configuration used by owner-local RPC services.
#[must_use]
pub fn server_config(maximum_body_bytes: u32) -> ServerConfig {
    ServerConfig::builder()
        .max_request_body_size(maximum_body_bytes)
        .max_response_body_size(maximum_body_bytes)
        .max_connections(MAXIMUM_CONNECTIONS)
        .set_batch_request_config(BatchRequestConfig::Disabled)
        .http_only()
        .build()
}

/// Binds one owner-only Unix socket without replacing an existing path.
///
/// # Errors
///
/// Returns an error for a non-absolute path, an unsafe runtime directory, an
/// existing endpoint, a bind or permission failure, or failed post-bind checks.
pub fn bind_owner_socket(path: &Path) -> anyhow::Result<(UnixListener, OwnedPath)> {
    ensure!(path.is_absolute(), "maker RPC socket path must be absolute");
    let runtime = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .context("maker RPC socket needs a runtime directory")?;
    validate_runtime_directory(runtime)?;
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error).context("inspect maker RPC socket path"),
        Ok(_) => bail!("refusing to replace existing maker RPC socket path"),
    }

    let listener = UnixListener::bind(path).context("bind maker RPC Unix socket")?;
    let guard = OwnedPath::capture(path).context("capture maker RPC socket identity")?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .context("set maker RPC socket mode")?;
    let metadata = fs::symlink_metadata(path).context("verify maker RPC socket")?;
    ensure!(
        metadata.file_type().is_socket()
            && metadata.uid() == rustix::process::geteuid().as_raw()
            && metadata.mode() & 0o7777 == 0o600,
        "maker RPC socket is not an owner-only socket"
    );
    Ok((listener, guard))
}

/// Validates the effective-user-owned mode-0700 real runtime directory.
///
/// # Errors
///
/// Returns an error when metadata cannot be read, the path is not a real
/// directory, ownership differs from the effective user, or mode is not 0700.
pub fn validate_runtime_directory(path: &Path) -> anyhow::Result<()> {
    let metadata = fs::symlink_metadata(path).context("inspect maker RPC runtime directory")?;
    ensure!(
        metadata.file_type().is_dir(),
        "maker RPC runtime path must be a real directory"
    );
    ensure!(
        metadata.uid() == rustix::process::geteuid().as_raw(),
        "maker RPC runtime directory must be owned by the daemon user"
    );
    ensure!(
        metadata.mode() & 0o7777 == 0o700,
        "maker RPC runtime directory must have mode 0700"
    );
    Ok(())
}

/// Removes a path on drop only while it retains the captured inode identity.
#[derive(Debug)]
pub struct OwnedPath {
    path: PathBuf,
    device: u64,
    inode: u64,
}

impl OwnedPath {
    /// Captures one existing path's device and inode for identity-safe cleanup.
    ///
    /// # Errors
    ///
    /// Returns an I/O error when the path metadata cannot be inspected.
    pub fn capture(path: &Path) -> io::Result<Self> {
        let metadata = fs::symlink_metadata(path)?;
        Ok(Self {
            path: path.to_path_buf(),
            device: metadata.dev(),
            inode: metadata.ino(),
        })
    }
}

impl Drop for OwnedPath {
    fn drop(&mut self) {
        let Ok(metadata) = fs::symlink_metadata(&self.path) else {
            return;
        };
        if metadata.dev() == self.device && metadata.ino() == self.inode {
            let _ = fs::remove_file(&self.path);
        }
    }
}
