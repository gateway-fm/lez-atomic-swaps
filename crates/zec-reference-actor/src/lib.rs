//! Secure configuration boundary for a one-shot, role-fixed reference actor.

use std::{
    collections::HashSet,
    fmt, fs,
    fs::File,
    io::Read as _,
    path::{Path, PathBuf},
};

use clap::{Parser, Subcommand};
use serde::Deserialize;
use thiserror::Error;
use zeroize::Zeroizing;

const CONFIG_SCHEMA_VERSION: u16 = 1;
const MAX_CONFIG_BYTES: usize = 64 * 1024;
const MAX_CONFIG_BYTES_U64: u64 = 64 * 1024;
const MAX_RUN_ID_BYTES: usize = 128;

/// Role permanently bound to one actor configuration.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ActorRole {
    /// Liquidity-providing participant.
    Maker,
    /// Offer-taking participant.
    Taker,
}

/// Exactly one lifecycle action performed by an actor process.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Subcommand)]
pub enum ActorCommand {
    /// Validate and durably activate the signed agreement.
    Activate,
    /// Reconcile and attempt one eligible chain effect.
    Drive,
    /// Return a secret-free durable status snapshot.
    Status,
}

/// Process arguments for the one-shot actor.
#[derive(Clone, Debug, Parser)]
#[command(about = "One-shot role-fixed LEZ/Zcash reference actor")]
pub struct ActorCli {
    /// Owner-private, bounded JSON configuration.
    #[arg(long, value_name = "PRIVATE_JSON")]
    pub config: PathBuf,
    /// Single lifecycle action; the process exits after it completes.
    #[command(subcommand)]
    pub command: ActorCommand,
}

/// Role-local paths and immutable run identity for one actor.
#[derive(Clone, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ActorConfig {
    #[serde(skip)]
    source_identity: PathBuf,
    schema_version: u16,
    role: ActorRole,
    run_id: String,
    role_state_db: PathBuf,
    bridge_journal_db: PathBuf,
    bridge_capability_file: PathBuf,
    zcash_key_file: PathBuf,
}

impl ActorConfig {
    /// Loads one owner-private config without retaining its source bytes.
    ///
    /// # Errors
    ///
    /// Rejects missing, non-regular, symlinked, overlarge, non-0600, malformed,
    /// ambiguous, or internally path-sharing configuration.
    pub fn load_private(path: impl AsRef<Path>) -> Result<Self, ActorConfigError> {
        let (bytes, source_identity) = read_private_config(path.as_ref())?;
        let mut config: Self = serde_json::from_slice(bytes.as_slice())
            .map_err(|_| ActorConfigError::InvalidConfiguration)?;
        config.source_identity = source_identity;
        config.validate()?;
        Ok(config)
    }

    /// Fixed local role.
    #[must_use]
    pub const fn role(&self) -> ActorRole {
        self.role
    }

    /// Run identity shared only with the opposite actor in this swap run.
    #[must_use]
    pub fn run_id(&self) -> &str {
        &self.run_id
    }

    fn validate(&self) -> Result<(), ActorConfigError> {
        if self.schema_version != CONFIG_SCHEMA_VERSION || !valid_run_id(&self.run_id) {
            return Err(ActorConfigError::InvalidConfiguration);
        }
        let identities = self.path_identities()?;
        if identities.iter().collect::<HashSet<_>>().len() != identities.len()
            || identities.contains(&self.source_identity)
        {
            return Err(ActorConfigError::InvalidConfiguration);
        }
        Ok(())
    }

    fn path_identities(&self) -> Result<[PathBuf; 4], ActorConfigError> {
        Ok([
            canonical_location(&self.role_state_db)?,
            canonical_location(&self.bridge_journal_db)?,
            canonical_location(&self.bridge_capability_file)?,
            canonical_location(&self.zcash_key_file)?,
        ])
    }
}

impl fmt::Debug for ActorConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ActorConfig")
            .field("source_identity", &"[REDACTED]")
            .field("schema_version", &self.schema_version)
            .field("role", &self.role)
            .field("run_id", &self.run_id)
            .field("role_state_db", &"[REDACTED]")
            .field("bridge_journal_db", &"[REDACTED]")
            .field("bridge_capability_file", &"[REDACTED]")
            .field("zcash_key_file", &"[REDACTED]")
            .finish()
    }
}

/// A secret-safe configuration or actor-isolation failure.
#[derive(Clone, Debug, Eq, PartialEq, Error)]
pub enum ActorConfigError {
    /// The config could not be securely inspected, opened, or read.
    #[error("actor configuration is unavailable")]
    ConfigurationUnavailable,
    /// The config was not an owner-private bounded regular file.
    #[error("actor configuration file is unsafe")]
    UnsafeConfigurationFile,
    /// The config contents or local path bindings were invalid.
    #[error("actor configuration is invalid")]
    InvalidConfiguration,
    /// Two role-fixed actors did not describe one isolated swap run.
    #[error("actor pair isolation is invalid")]
    InvalidActorPair,
}

/// Confirms that maker and taker represent distinct users in the same run.
///
/// # Errors
///
/// Rejects equal roles, different runs, or any shared role-local state,
/// journal, capability, or signing-key path after canonical path resolution.
pub fn validate_actor_pair(
    left: &ActorConfig,
    right: &ActorConfig,
) -> Result<(), ActorConfigError> {
    if left.role == right.role || left.run_id != right.run_id {
        return Err(ActorConfigError::InvalidActorPair);
    }
    let left_paths = left
        .path_identities()
        .map_err(|_| ActorConfigError::InvalidActorPair)?;
    let right_paths = right
        .path_identities()
        .map_err(|_| ActorConfigError::InvalidActorPair)?;
    if left.source_identity == right.source_identity
        || left_paths
            .iter()
            .any(|left_path| right_paths.contains(left_path))
        || left_paths.contains(&right.source_identity)
        || right_paths.contains(&left.source_identity)
    {
        return Err(ActorConfigError::InvalidActorPair);
    }
    Ok(())
}

fn read_private_config(path: &Path) -> Result<(Zeroizing<Vec<u8>>, PathBuf), ActorConfigError> {
    let before =
        fs::symlink_metadata(path).map_err(|_| ActorConfigError::ConfigurationUnavailable)?;
    validate_private_metadata(&before)?;
    let file = File::open(path).map_err(|_| ActorConfigError::ConfigurationUnavailable)?;
    let opened = file
        .metadata()
        .map_err(|_| ActorConfigError::ConfigurationUnavailable)?;
    validate_private_metadata(&opened)?;
    if !same_file(&before, &opened) {
        return Err(ActorConfigError::UnsafeConfigurationFile);
    }

    let mut bytes = Zeroizing::new(Vec::with_capacity(MAX_CONFIG_BYTES + 1));
    file.take(MAX_CONFIG_BYTES_U64 + 1)
        .read_to_end(bytes.as_mut())
        .map_err(|_| ActorConfigError::ConfigurationUnavailable)?;
    if bytes.is_empty() || bytes.len() > MAX_CONFIG_BYTES {
        return Err(ActorConfigError::UnsafeConfigurationFile);
    }
    let source_identity =
        fs::canonicalize(path).map_err(|_| ActorConfigError::ConfigurationUnavailable)?;
    let after =
        fs::symlink_metadata(path).map_err(|_| ActorConfigError::ConfigurationUnavailable)?;
    validate_private_metadata(&after)?;
    if !same_file(&opened, &after) {
        return Err(ActorConfigError::UnsafeConfigurationFile);
    }
    Ok((bytes, source_identity))
}

fn validate_private_metadata(metadata: &fs::Metadata) -> Result<(), ActorConfigError> {
    if !metadata.file_type().is_file()
        || metadata.len() == 0
        || metadata.len() > MAX_CONFIG_BYTES_U64
    {
        return Err(ActorConfigError::UnsafeConfigurationFile);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        if metadata.permissions().mode() & 0o7777 != 0o600 {
            return Err(ActorConfigError::UnsafeConfigurationFile);
        }
    }
    Ok(())
}

fn canonical_location(path: &Path) -> Result<PathBuf, ActorConfigError> {
    if !path.is_absolute() {
        return Err(ActorConfigError::InvalidConfiguration);
    }
    if path.exists() {
        return fs::canonicalize(path).map_err(|_| ActorConfigError::InvalidConfiguration);
    }
    let parent = path
        .parent()
        .ok_or(ActorConfigError::InvalidConfiguration)?;
    let name = path
        .file_name()
        .ok_or(ActorConfigError::InvalidConfiguration)?;
    let parent = fs::canonicalize(parent).map_err(|_| ActorConfigError::InvalidConfiguration)?;
    Ok(parent.join(name))
}

fn valid_run_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_RUN_ID_BYTES
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, 45 | 46 | 95)
        })
}

#[cfg(unix)]
fn same_file(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt as _;
    left.dev() == right.dev() && left.ino() == right.ino()
}

#[cfg(not(unix))]
fn same_file(_left: &fs::Metadata, _right: &fs::Metadata) -> bool {
    true
}
