//! Process lifecycle seam shared by standalone and future Logos Core supervisors.

use std::{
    fs,
    os::unix::fs::{FileTypeExt as _, MetadataExt as _, PermissionsExt as _},
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus},
    time::{Duration, Instant},
};

use async_trait::async_trait;
use rustix::process::{Pid, Signal, kill_process};
use thiserror::Error;

use crate::{ListRequest, MakerHealthV1, call_local_rpc};

const POLL_INTERVAL: Duration = Duration::from_millis(20);
const STARTUP_CLEANUP_GRACE: Duration = Duration::from_secs(2);

/// Immutable launch inputs for the maker daemon lifecycle seam.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MakerDaemonLaunchConfig {
    executable: PathBuf,
    database: PathBuf,
    socket: PathBuf,
    ready_file: PathBuf,
    startup_timeout: Duration,
    health_timeout: Duration,
}

impl MakerDaemonLaunchConfig {
    /// Validates one same-binary maker daemon launch description.
    ///
    /// # Errors
    ///
    /// Returns [`MakerDaemonLifecycleError::InvalidConfig`] unless every path is
    /// absolute, the executable is a real executable file, the readiness file
    /// shares the socket directory, and both timeouts are non-zero.
    pub fn new(
        executable: impl Into<PathBuf>,
        database: impl Into<PathBuf>,
        socket: impl Into<PathBuf>,
        ready_file: impl Into<PathBuf>,
        startup_timeout: Duration,
        health_timeout: Duration,
    ) -> Result<Self, MakerDaemonLifecycleError> {
        let config = Self {
            executable: executable.into(),
            database: database.into(),
            socket: socket.into(),
            ready_file: ready_file.into(),
            startup_timeout,
            health_timeout,
        };
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> Result<(), MakerDaemonLifecycleError> {
        if !self.executable.is_absolute()
            || !self.database.is_absolute()
            || !self.socket.is_absolute()
            || !self.ready_file.is_absolute()
        {
            return Err(MakerDaemonLifecycleError::InvalidConfig(
                "all lifecycle paths must be absolute",
            ));
        }
        if self.socket.parent() != self.ready_file.parent() {
            return Err(MakerDaemonLifecycleError::InvalidConfig(
                "readiness file must share the socket runtime directory",
            ));
        }
        if self.startup_timeout.is_zero() || self.health_timeout.is_zero() {
            return Err(MakerDaemonLifecycleError::InvalidConfig(
                "startup and health timeouts must be non-zero",
            ));
        }
        let metadata = fs::symlink_metadata(&self.executable).map_err(|source| {
            MakerDaemonLifecycleError::InspectExecutable {
                path: self.executable.clone(),
                source,
            }
        })?;
        if !metadata.file_type().is_file() || metadata.permissions().mode() & 0o111 == 0 {
            return Err(MakerDaemonLifecycleError::InvalidConfig(
                "daemon executable must be a real executable file",
            ));
        }
        Ok(())
    }
}

/// Read-only health result returned through the daemon's normal local RPC.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MakerDaemonHealth {
    endpoint: PathBuf,
    process_id: u32,
}

impl MakerDaemonHealth {
    /// Owner-local endpoint published by the running daemon.
    #[must_use]
    pub fn endpoint(&self) -> &Path {
        &self.endpoint
    }

    /// Operating-system process identifier owned by this adapter instance.
    #[must_use]
    pub const fn process_id(&self) -> u32 {
        self.process_id
    }
}

/// Lifecycle failures that a Logos Core or standalone supervisor must surface.
#[derive(Debug, Error)]
pub enum MakerDaemonLifecycleError {
    /// Launch configuration violates the owner-local process contract.
    #[error("invalid maker daemon lifecycle configuration: {0}")]
    InvalidConfig(&'static str),
    /// The configured daemon executable could not be inspected.
    #[error("inspect maker daemon executable {path}")]
    InspectExecutable {
        /// Configured executable path.
        path: PathBuf,
        /// Filesystem error.
        #[source]
        source: std::io::Error,
    },
    /// A second process cannot be started by the same adapter.
    #[error("maker daemon lifecycle already owns a process")]
    AlreadyRunning,
    /// Process creation failed.
    #[error("spawn maker daemon")]
    Spawn(#[source] std::io::Error),
    /// Child status polling or reaping failed.
    #[error("poll or reap maker daemon")]
    Wait(#[source] std::io::Error),
    /// The process exited before publishing the exact readiness handoff.
    #[error("maker daemon exited before readiness: {0}")]
    ExitedBeforeReady(ExitStatus),
    /// The process did not publish readiness before its configured deadline.
    #[error("maker daemon readiness timed out")]
    StartupTimeout,
    /// The readiness file or endpoint violates the daemon contract.
    #[error("invalid maker daemon readiness handoff: {0}")]
    InvalidReadiness(&'static str),
    /// The adapter has no live ready process.
    #[error("maker daemon is not running")]
    NotRunning,
    /// The daemon exceeded the bounded read-only health deadline.
    #[error("maker daemon health RPC timed out")]
    HealthTimeout,
    /// The daemon failed its read-only RPC health query.
    #[error("maker daemon health RPC failed")]
    Health(#[source] anyhow::Error),
    /// Sending the graceful termination signal failed.
    #[error("signal maker daemon for graceful stop")]
    Signal(#[source] rustix::io::Errno),
    /// The process exceeded the caller's graceful-stop deadline and was killed.
    #[error("maker daemon graceful stop timed out")]
    StopTimeout,
    /// A graceful signal produced an unsuccessful process result.
    #[error("maker daemon exited unsuccessfully: {0}")]
    UnsuccessfulExit(ExitStatus),
}

/// Minimal lifecycle surface expected by future Logos Core daemon mode.
///
/// Implementations invoke the normal maker daemon binary and use its owner-local
/// readiness and RPC contracts. They must not open `SQLite` or deserialize wallet
/// and claim credentials.
#[async_trait]
pub trait MakerDaemonLifecycle {
    /// Starts one maker daemon and waits for exact readiness plus bounded health.
    async fn start(
        &mut self,
        config: MakerDaemonLaunchConfig,
    ) -> Result<(), MakerDaemonLifecycleError>;

    /// Returns the ready owner-local endpoint, or `None` while stopped.
    fn endpoint(&self) -> Option<&Path>;

    /// Probes the running process through a bounded read-only daemon RPC.
    async fn health(&mut self) -> Result<MakerDaemonHealth, MakerDaemonLifecycleError>;

    /// Requests SIGTERM shutdown and enforces the caller's grace period.
    async fn stop(&mut self, grace_period: Duration) -> Result<(), MakerDaemonLifecycleError>;
}

/// Exact-child process adapter suitable for a future Logos Core host.
///
/// This adapter accepts no key material. Deployment owns credential delivery;
/// the adapter owns only one exact child, readiness handoff, and RPC endpoint.
#[derive(Debug, Default)]
pub struct ProcessMakerDaemon {
    child: Option<Child>,
    endpoint: Option<PathBuf>,
    health_timeout: Option<Duration>,
}

#[async_trait]
impl MakerDaemonLifecycle for ProcessMakerDaemon {
    async fn start(
        &mut self,
        config: MakerDaemonLaunchConfig,
    ) -> Result<(), MakerDaemonLifecycleError> {
        if self.child.is_some() {
            return Err(MakerDaemonLifecycleError::AlreadyRunning);
        }
        config.validate()?;
        let child = Command::new(&config.executable)
            .arg("--socket")
            .arg(&config.socket)
            .arg("--database")
            .arg(&config.database)
            .arg("--ready-file")
            .arg(&config.ready_file)
            .spawn()
            .map_err(MakerDaemonLifecycleError::Spawn)?;
        self.child = Some(child);

        let deadline = Instant::now() + config.startup_timeout;
        loop {
            if let Some(status) = self
                .child
                .as_mut()
                .expect("child exists during startup")
                .try_wait()
                .map_err(MakerDaemonLifecycleError::Wait)?
            {
                self.clear();
                return Err(MakerDaemonLifecycleError::ExitedBeforeReady(status));
            }
            if let Ok(published) = fs::read_to_string(&config.ready_file) {
                if let Err(error) = validate_readiness(&config, &published) {
                    self.abort_start().await;
                    return Err(error);
                }
                self.endpoint = Some(config.socket.clone());
                self.health_timeout = Some(config.health_timeout);
                if let Err(error) = self.health().await {
                    self.abort_start().await;
                    return Err(error);
                }
                return Ok(());
            }
            if Instant::now() >= deadline {
                self.abort_start().await;
                return Err(MakerDaemonLifecycleError::StartupTimeout);
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }

    fn endpoint(&self) -> Option<&Path> {
        self.endpoint.as_deref()
    }

    async fn health(&mut self) -> Result<MakerDaemonHealth, MakerDaemonLifecycleError> {
        let child = self
            .child
            .as_mut()
            .ok_or(MakerDaemonLifecycleError::NotRunning)?;
        if child
            .try_wait()
            .map_err(MakerDaemonLifecycleError::Wait)?
            .is_some()
        {
            self.clear();
            return Err(MakerDaemonLifecycleError::NotRunning);
        }
        let endpoint = self
            .endpoint
            .clone()
            .ok_or(MakerDaemonLifecycleError::NotRunning)?;
        let timeout = self
            .health_timeout
            .ok_or(MakerDaemonLifecycleError::NotRunning)?;
        let response = tokio::time::timeout(
            timeout,
            call_local_rpc::<_, MakerHealthV1>(&endpoint, "maker_health", &ListRequest::default()),
        )
        .await
        .map_err(|_| MakerDaemonLifecycleError::HealthTimeout)?
        .map_err(MakerDaemonLifecycleError::Health)?;
        if !response.is_ready() {
            return Err(MakerDaemonLifecycleError::Health(anyhow::anyhow!(
                "maker daemon returned a non-ready health state"
            )));
        }
        Ok(MakerDaemonHealth {
            endpoint,
            process_id: child.id(),
        })
    }

    async fn stop(&mut self, grace_period: Duration) -> Result<(), MakerDaemonLifecycleError> {
        let Some(child) = self.child.as_mut() else {
            self.clear();
            return Ok(());
        };
        let result = stop_child(child, grace_period).await;
        self.clear();
        result
    }
}

impl ProcessMakerDaemon {
    async fn abort_start(&mut self) {
        if let Some(child) = self.child.as_mut() {
            let _ = stop_child(child, STARTUP_CLEANUP_GRACE).await;
        }
        self.clear();
    }

    fn clear(&mut self) {
        self.child = None;
        self.endpoint = None;
        self.health_timeout = None;
    }
}

impl Drop for ProcessMakerDaemon {
    fn drop(&mut self) {
        if let Some(child) = self.child.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn validate_readiness(
    config: &MakerDaemonLaunchConfig,
    published: &str,
) -> Result<(), MakerDaemonLifecycleError> {
    if published.trim() != config.socket.to_string_lossy() {
        return Err(MakerDaemonLifecycleError::InvalidReadiness(
            "published endpoint differs from configured socket",
        ));
    }
    let metadata = fs::symlink_metadata(&config.socket).map_err(|_| {
        MakerDaemonLifecycleError::InvalidReadiness("published socket does not exist")
    })?;
    if !metadata.file_type().is_socket()
        || metadata.permissions().mode() & 0o7777 != 0o600
        || metadata.uid() != rustix::process::geteuid().as_raw()
    {
        return Err(MakerDaemonLifecycleError::InvalidReadiness(
            "published endpoint is not the owner mode-0600 Unix socket",
        ));
    }
    Ok(())
}

async fn stop_child(
    child: &mut Child,
    grace_period: Duration,
) -> Result<(), MakerDaemonLifecycleError> {
    if let Some(status) = child.try_wait().map_err(MakerDaemonLifecycleError::Wait)? {
        return status
            .success()
            .then_some(())
            .ok_or(MakerDaemonLifecycleError::UnsuccessfulExit(status));
    }
    kill_process(Pid::from_child(&*child), Signal::TERM)
        .map_err(MakerDaemonLifecycleError::Signal)?;
    let deadline = Instant::now() + grace_period;
    loop {
        if let Some(status) = child.try_wait().map_err(MakerDaemonLifecycleError::Wait)? {
            return status
                .success()
                .then_some(())
                .ok_or(MakerDaemonLifecycleError::UnsuccessfulExit(status));
        }
        if Instant::now() >= deadline {
            child.kill().map_err(MakerDaemonLifecycleError::Wait)?;
            child.wait().map_err(MakerDaemonLifecycleError::Wait)?;
            return Err(MakerDaemonLifecycleError::StopTimeout);
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}
