//! Fresh authenticated bridge clients backed by a private capability file.

use std::{
    fmt, fs,
    fs::File,
    io::Read as _,
    path::{Path, PathBuf},
    time::Duration,
};

use lez_bridge_client::{
    BridgeClient, BridgeClientConfig, BridgeClientError, SidecarCapability, XmrReleaseClient,
};
use lez_bridge_protocol::{RunId, RuntimeDescriptor};
use thiserror::Error;

use crate::FreshLezBridgeTransportFactory;

const MAX_CAPABILITY_FILE_BYTES: usize = 130;
const MAX_CAPABILITY_FILE_BYTES_U64: u64 = 130;

/// Creates one new authenticated sidecar client per bridge attempt.
///
/// Capability material is never cached. Each call reopens and revalidates the
/// private file, allowing deliberate rotation while keeping the secret out of
/// the long-lived composition object.
#[derive(Clone)]
pub struct CapabilityFileBridgeClientFactory {
    endpoint: String,
    capability_file: PathBuf,
    expected_run_id: RunId,
    expected_runtime: RuntimeDescriptor,
    request_timeout: Duration,
}

impl CapabilityFileBridgeClientFactory {
    /// Binds public sidecar identity to a private capability-file location.
    #[must_use]
    pub fn new(
        endpoint: impl Into<String>,
        capability_file: impl Into<PathBuf>,
        expected_run_id: RunId,
        expected_runtime: RuntimeDescriptor,
        request_timeout: Duration,
    ) -> Self {
        Self {
            endpoint: endpoint.into(),
            capability_file: capability_file.into(),
            expected_run_id,
            expected_runtime,
            request_timeout,
        }
    }
}

/// Creates one new release-only authenticated sidecar client per publication process.
///
/// The factory deliberately cannot construct the ordinary [`BridgeClient`].
/// Capability material is loaded from the private file only when a fresh
/// release client is requested, so rotation and restart preserve the narrow
/// release-service API.
#[derive(Clone)]
pub struct CapabilityFileXmrReleaseClientFactory {
    endpoint: String,
    capability_file: PathBuf,
    expected_run_id: RunId,
    expected_runtime: RuntimeDescriptor,
    request_timeout: Duration,
}

impl CapabilityFileXmrReleaseClientFactory {
    /// Binds public sidecar identity to a release-service-owned capability file.
    #[must_use]
    pub fn new(
        endpoint: impl Into<String>,
        capability_file: impl Into<PathBuf>,
        expected_run_id: RunId,
        expected_runtime: RuntimeDescriptor,
        request_timeout: Duration,
    ) -> Self {
        Self {
            endpoint: endpoint.into(),
            capability_file: capability_file.into(),
            expected_run_id,
            expected_runtime,
            request_timeout,
        }
    }
}

impl fmt::Debug for CapabilityFileXmrReleaseClientFactory {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CapabilityFileXmrReleaseClientFactory")
            .field("endpoint", &self.endpoint)
            .field("capability_file", &"[REDACTED]")
            .field("expected_run_id", &self.expected_run_id)
            .field("expected_runtime", &self.expected_runtime)
            .field("request_timeout", &self.request_timeout)
            .finish()
    }
}

impl fmt::Debug for CapabilityFileBridgeClientFactory {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CapabilityFileBridgeClientFactory")
            .field("endpoint", &self.endpoint)
            .field("capability_file", &"[REDACTED]")
            .field("expected_run_id", &self.expected_run_id)
            .field("expected_runtime", &self.expected_runtime)
            .field("request_timeout", &self.request_timeout)
            .finish()
    }
}

/// Failure to load private capability material or construct a bounded client.
#[derive(Debug, Error)]
pub enum CapabilityFileBridgeClientFactoryError {
    /// The configured private file could not be inspected, opened, or read.
    #[error("LEZ sidecar capability file is unavailable")]
    CapabilityFileUnavailable,
    /// The path was not a bounded regular file with the required private mode.
    #[error("LEZ sidecar capability file is unsafe")]
    UnsafeCapabilityFile,
    /// File contents were not one valid bounded sidecar capability.
    #[error("LEZ sidecar capability is invalid")]
    InvalidCapability,
    /// Public endpoint, identity, or timeout configuration was invalid.
    #[error("LEZ bridge client configuration is invalid")]
    ClientConfiguration(#[source] BridgeClientError),
}

impl FreshLezBridgeTransportFactory for CapabilityFileBridgeClientFactory {
    type Transport = BridgeClient;
    type Error = CapabilityFileBridgeClientFactoryError;

    fn fresh_transport(&self) -> Result<Self::Transport, Self::Error> {
        let capability = read_capability(&self.capability_file)?;
        BridgeClient::connect(BridgeClientConfig::new(
            self.endpoint.clone(),
            capability,
            self.expected_run_id.clone(),
            self.expected_runtime.clone(),
            self.request_timeout,
        ))
        .map_err(CapabilityFileBridgeClientFactoryError::ClientConfiguration)
    }
}

impl FreshLezBridgeTransportFactory for CapabilityFileXmrReleaseClientFactory {
    type Transport = XmrReleaseClient;
    type Error = CapabilityFileBridgeClientFactoryError;

    fn fresh_transport(&self) -> Result<Self::Transport, Self::Error> {
        let capability = read_capability(&self.capability_file)?;
        XmrReleaseClient::connect(BridgeClientConfig::new(
            self.endpoint.clone(),
            capability,
            self.expected_run_id.clone(),
            self.expected_runtime.clone(),
            self.request_timeout,
        ))
        .map_err(CapabilityFileBridgeClientFactoryError::ClientConfiguration)
    }
}

fn read_capability(
    path: &Path,
) -> Result<SidecarCapability, CapabilityFileBridgeClientFactoryError> {
    let before = fs::symlink_metadata(path)
        .map_err(|_| CapabilityFileBridgeClientFactoryError::CapabilityFileUnavailable)?;
    validate_metadata(&before)?;

    let file = File::open(path)
        .map_err(|_| CapabilityFileBridgeClientFactoryError::CapabilityFileUnavailable)?;
    let opened = file
        .metadata()
        .map_err(|_| CapabilityFileBridgeClientFactoryError::CapabilityFileUnavailable)?;
    validate_metadata(&opened)?;
    if !same_file(&before, &opened) {
        return Err(CapabilityFileBridgeClientFactoryError::UnsafeCapabilityFile);
    }

    let mut bytes = Vec::with_capacity(MAX_CAPABILITY_FILE_BYTES + 1);
    (&file)
        .take(MAX_CAPABILITY_FILE_BYTES_U64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| CapabilityFileBridgeClientFactoryError::CapabilityFileUnavailable)?;

    let opened_after = file
        .metadata()
        .map_err(|_| CapabilityFileBridgeClientFactoryError::CapabilityFileUnavailable)?;
    let path_after = fs::symlink_metadata(path)
        .map_err(|_| CapabilityFileBridgeClientFactoryError::CapabilityFileUnavailable)?;
    validate_metadata(&opened_after)?;
    validate_metadata(&path_after)?;
    if !stable_file(&opened, &opened_after) || !stable_file(&opened, &path_after) {
        bytes.fill(0);
        return Err(CapabilityFileBridgeClientFactoryError::UnsafeCapabilityFile);
    }
    if bytes.is_empty() || bytes.len() > MAX_CAPABILITY_FILE_BYTES {
        bytes.fill(0);
        return Err(CapabilityFileBridgeClientFactoryError::UnsafeCapabilityFile);
    }
    if bytes.ends_with(b"\r\n") {
        bytes.truncate(bytes.len() - 2);
    } else if bytes.ends_with(b"\n") {
        bytes.truncate(bytes.len() - 1);
    }
    let value = String::from_utf8(bytes).map_err(|error| {
        let mut bytes = error.into_bytes();
        bytes.fill(0);
        CapabilityFileBridgeClientFactoryError::InvalidCapability
    })?;
    SidecarCapability::new(value)
        .map_err(|_| CapabilityFileBridgeClientFactoryError::InvalidCapability)
}

fn validate_metadata(
    metadata: &fs::Metadata,
) -> Result<(), CapabilityFileBridgeClientFactoryError> {
    if !metadata.file_type().is_file()
        || metadata.len() == 0
        || metadata.len() > MAX_CAPABILITY_FILE_BYTES_U64
    {
        return Err(CapabilityFileBridgeClientFactoryError::UnsafeCapabilityFile);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};

        if metadata.permissions().mode() & 0o7777 != 0o600
            || metadata.nlink() != 1
            || metadata.uid() != rustix::process::geteuid().as_raw()
        {
            return Err(CapabilityFileBridgeClientFactoryError::UnsafeCapabilityFile);
        }
    }
    Ok(())
}

#[cfg(unix)]
fn same_file(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt as _;

    left.dev() == right.dev() && left.ino() == right.ino()
}

#[cfg(unix)]
fn stable_file(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt as _;

    same_file(left, right)
        && left.len() == right.len()
        && left.uid() == right.uid()
        && left.mode() == right.mode()
        && left.nlink() == right.nlink()
        && left.mtime() == right.mtime()
        && left.mtime_nsec() == right.mtime_nsec()
        && left.ctime() == right.ctime()
        && left.ctime_nsec() == right.ctime_nsec()
}

#[cfg(not(unix))]
fn stable_file(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    same_file(left, right) && left.len() == right.len()
}

#[cfg(not(unix))]
fn same_file(_left: &fs::Metadata, _right: &fs::Metadata) -> bool {
    true
}
