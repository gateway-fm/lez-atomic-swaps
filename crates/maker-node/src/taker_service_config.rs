//! Owner-private startup configuration for the read-only Taker service.

use std::{
    fmt,
    os::unix::fs::{FileTypeExt as _, MetadataExt as _},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use secp256k1::PublicKey;
use serde::Deserialize;
use thiserror::Error;

use crate::{
    MAX_TAKER_DELIVERY_SOURCES_V1, MAX_TAKER_OFFER_RESULTS_V1, RunLocalDelivery,
    TakerDependencyProbe, TakerFacadeBackend, TakerMakerIdentityV1, TakerTrustedTimeSource,
    secure_file::read_private_file,
};

const MAXIMUM_STARTUP_CONFIGURATION_BYTES: u64 = 64 * 1024;
const STARTUP_SCHEMA_VERSION: u16 = 1;

/// Fixed path-free failures while constructing Taker service dependencies.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum TakerServiceStartupError {
    /// The owner-private configuration file could not be read safely.
    #[error("Taker service startup configuration is unavailable")]
    ConfigurationUnavailable,
    /// The decoded schema, bounds, identity, or configured path was invalid.
    #[error("Taker service startup configuration is invalid")]
    InvalidConfiguration,
    /// A configured Delivery subscriber could not be opened safely.
    #[error("Taker service Delivery source is unavailable")]
    DeliveryUnavailable,
}

/// Metadata-only availability probe for one configured owner-local Chat socket.
pub struct OwnerChatSocketProbe {
    socket: PathBuf,
}

impl OwnerChatSocketProbe {
    /// Creates a probe for one absolute socket path.
    ///
    /// # Errors
    ///
    /// Returns `InvalidConfiguration` when the socket path is not absolute.
    pub fn new(socket: PathBuf) -> Result<Self, TakerServiceStartupError> {
        if !socket.is_absolute() {
            return Err(TakerServiceStartupError::InvalidConfiguration);
        }
        Ok(Self { socket })
    }
}

impl fmt::Debug for OwnerChatSocketProbe {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OwnerChatSocketProbe")
            .field("configured", &true)
            .finish()
    }
}

impl TakerDependencyProbe for OwnerChatSocketProbe {
    fn is_available(&self) -> bool {
        std::fs::symlink_metadata(&self.socket).is_ok_and(|metadata| {
            metadata.file_type().is_socket()
                && metadata.uid() == rustix::process::geteuid().as_raw()
                && metadata.mode() & 0o7777 == 0o600
        })
    }
}

/// Trusted wall-clock implementation injected into the Taker facade backend.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemTakerTrustedTime;

impl TakerTrustedTimeSource for SystemTakerTrustedTime {
    fn now_unix_seconds(&self) -> Option<u64> {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .map(|duration| duration.as_secs())
    }
}

/// Read-only Taker backend assembled from owner-private startup configuration.
pub type ConfiguredTakerFacadeBackend =
    TakerFacadeBackend<SystemTakerTrustedTime, OwnerChatSocketProbe>;

/// Loads, validates, and constructs one read-only Taker service backend.
///
/// Private locations and pinned Maker identities remain inside the returned
/// backend and are omitted from its Debug implementation.
///
/// # Errors
///
/// Returns a fixed `TakerServiceStartupError` without exposing a configured path,
/// identity, parser detail, or underlying adapter failure.
pub fn load_taker_service_backend(
    path: &Path,
) -> Result<ConfiguredTakerFacadeBackend, TakerServiceStartupError> {
    let bytes = read_private_file(
        path,
        MAXIMUM_STARTUP_CONFIGURATION_BYTES,
        "Taker service startup configuration",
    )
    .map_err(|_| TakerServiceStartupError::ConfigurationUnavailable)?;
    let configuration: StartupConfigurationV1 = serde_json::from_slice(&bytes)
        .map_err(|_| TakerServiceStartupError::InvalidConfiguration)?;
    validate_configuration(&configuration)?;

    let mut delivery_sources = Vec::with_capacity(configuration.delivery_sources.len());
    for source in configuration.delivery_sources {
        let maker = PublicKey::from_slice(source.maker_public_key.as_bytes())
            .map_err(|_| TakerServiceStartupError::InvalidConfiguration)?;
        let subscriber = RunLocalDelivery::subscriber(source.directory, maker)
            .map_err(|_| TakerServiceStartupError::DeliveryUnavailable)?;
        delivery_sources.push(subscriber);
    }
    let chat = configuration
        .chat_socket
        .map(OwnerChatSocketProbe::new)
        .transpose()?;
    TakerFacadeBackend::new(
        delivery_sources,
        SystemTakerTrustedTime,
        chat,
        configuration.maximum_offers,
    )
    .map_err(|_| TakerServiceStartupError::InvalidConfiguration)
}

fn validate_configuration(
    configuration: &StartupConfigurationV1,
) -> Result<(), TakerServiceStartupError> {
    if configuration.schema_version != STARTUP_SCHEMA_VERSION
        || configuration.delivery_sources.len() > MAX_TAKER_DELIVERY_SOURCES_V1
        || configuration.maximum_offers == 0
        || configuration.maximum_offers > MAX_TAKER_OFFER_RESULTS_V1
        || configuration
            .delivery_sources
            .iter()
            .any(|source| !source.directory.is_absolute())
        || configuration
            .chat_socket
            .as_ref()
            .is_some_and(|socket| !socket.is_absolute())
    {
        return Err(TakerServiceStartupError::InvalidConfiguration);
    }
    Ok(())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StartupConfigurationV1 {
    schema_version: u16,
    delivery_sources: Vec<DeliverySourceV1>,
    #[serde(default)]
    chat_socket: Option<PathBuf>,
    maximum_offers: usize,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DeliverySourceV1 {
    directory: PathBuf,
    maker_public_key: TakerMakerIdentityV1,
}
