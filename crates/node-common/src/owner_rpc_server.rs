//! Reusable owner-local Unix RPC server configuration and socket ownership.

use std::{
    fs::{self, File},
    io::{self, Write as _},
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
    ensure!(path.is_absolute(), "owner RPC socket path must be absolute");
    let runtime = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .context("owner RPC socket needs a runtime directory")?;
    validate_runtime_directory(runtime)?;
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error).context("inspect owner RPC socket path"),
        Ok(_) => bail!("refusing to replace existing owner RPC socket path"),
    }

    let listener = UnixListener::bind(path).context("bind owner RPC Unix socket")?;
    let guard = OwnedPath::capture(path).context("capture owner RPC socket identity")?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .context("set owner RPC socket mode")?;
    let metadata = fs::symlink_metadata(path).context("verify owner RPC socket")?;
    ensure!(
        metadata.file_type().is_socket()
            && metadata.uid() == rustix::process::geteuid().as_raw()
            && metadata.mode() & 0o7777 == 0o600,
        "owner RPC socket is not an owner-only socket"
    );
    Ok((listener, guard))
}

/// Publishes an inode-guarded readiness file beside an owner socket.
///
/// # Errors
///
/// Returns an error for a non-absolute or cross-directory path, an existing
/// destination, or a write, synchronization, or identity-capture failure.
pub fn publish_ready_file(path: &Path, socket: &Path, role: &str) -> anyhow::Result<OwnedPath> {
    ensure!(path.is_absolute(), "{role} readiness path must be absolute");
    ensure!(
        path.parent() == socket.parent(),
        "{role} readiness file must share the socket runtime directory"
    );
    let parent = path
        .parent()
        .with_context(|| format!("{role} readiness path has no parent"))?;
    let mut staged = tempfile::Builder::new()
        .prefix(".node-ready.")
        .tempfile_in(parent)
        .with_context(|| format!("stage {role} readiness file"))?;
    writeln!(staged, "{}", socket.display())
        .with_context(|| format!("write {role} readiness file"))?;
    staged
        .as_file_mut()
        .sync_all()
        .with_context(|| format!("sync staged {role} readiness file"))?;
    staged
        .persist_noclobber(path)
        .map_err(|error| error.error)
        .with_context(|| format!("publish {role} readiness file without clobber"))?;
    File::open(parent)?.sync_all()?;
    OwnedPath::capture(path).with_context(|| format!("capture {role} readiness file identity"))
}

/// Validates the effective-user-owned mode-0700 real runtime directory.
///
/// # Errors
///
/// Returns an error when metadata cannot be read, the path is not a real
/// directory, ownership differs from the effective user, or mode is not 0700.
pub fn validate_runtime_directory(path: &Path) -> anyhow::Result<()> {
    let metadata = fs::symlink_metadata(path).context("inspect owner RPC runtime directory")?;
    ensure!(
        metadata.file_type().is_dir(),
        "owner RPC runtime path must be a real directory"
    );
    ensure!(
        metadata.uid() == rustix::process::geteuid().as_raw(),
        "owner RPC runtime directory must be owned by the Node user"
    );
    ensure!(
        metadata.mode() & 0o7777 == 0o700,
        "owner RPC runtime directory must have mode 0700"
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
