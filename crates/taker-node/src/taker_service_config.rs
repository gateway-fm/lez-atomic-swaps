//! Owner-private startup configuration for the Taker Node: discovery-only by
//! default, prepared-route lifecycle when a validated catalog is configured.
//! The prepared-ZEC catalog lives in [`prepared_zec`] under the `pair-zec` feature.

#[cfg(feature = "pair-zec")]
mod prepared_zec;

#[cfg(feature = "pair-zec")]
use prepared_zec::InitiationConfigurationV1;
#[cfg(feature = "pair-zec")]
pub(crate) use prepared_zec::PreparedReceiptBindingV1;
#[cfg(feature = "pair-zec")]
pub use prepared_zec::{
    ConfiguredTakerInitiationContext, PreparedZecExecutionV1, PreparedZecTakerInitiationV1,
};

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    os::unix::fs::{FileTypeExt as _, MetadataExt as _},
    path::{Component, Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use secp256k1::PublicKey;
use serde::Deserialize;
use thiserror::Error;

use crate::{
    MAX_TAKER_DELIVERY_SOURCES_V1, MAX_TAKER_OFFER_RESULTS_V1, RunLocalDelivery,
    TakerDependencyProbe, TakerFacadeBackend, TakerMakerIdentityV1, TakerTrustedTimeSource,
    secure_file::read_private_file_snapshot,
};

const MAXIMUM_STARTUP_CONFIGURATION_BYTES: u64 = 512 * 1024;
const STARTUP_SCHEMA_VERSION: u16 = 1;

const MAX_SOURCE_ID_BYTES: usize = 128;

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
    /// Prepared initiation custody or the existing registry is unavailable.
    #[error("Taker service initiation authority is unavailable")]
    InitiationUnavailable,
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

/// Complete owner-private dependencies for the isolated Taker service.
pub struct ConfiguredTakerServiceContext {
    backend: ConfiguredTakerFacadeBackend,
    #[cfg(feature = "pair-zec")]
    initiation: Option<ConfiguredTakerInitiationContext>,
}

impl ConfiguredTakerServiceContext {
    #[cfg(feature = "pair-zec")]
    /// Borrows the optional mutation context.
    #[must_use]
    pub const fn initiation(&self) -> Option<&ConfiguredTakerInitiationContext> {
        self.initiation.as_ref()
    }

    /// Consumes the context and returns the read backend.
    #[must_use]
    pub fn into_backend(self) -> ConfiguredTakerFacadeBackend {
        self.backend
    }

    #[cfg(feature = "pair-zec")]
    /// Consumes the context into its read and optional mutation dependencies.
    #[must_use]
    pub fn into_parts(
        self,
    ) -> (
        ConfiguredTakerFacadeBackend,
        Option<ConfiguredTakerInitiationContext>,
    ) {
        (self.backend, self.initiation)
    }
}

impl fmt::Debug for ConfiguredTakerServiceContext {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut debug = formatter.debug_struct("ConfiguredTakerServiceContext");
        debug.field("backend", &self.backend);
        #[cfg(feature = "pair-zec")]
        debug.field("initiation_configured", &self.initiation.is_some());
        debug.finish_non_exhaustive()
    }
}

/// Loads the complete Taker service context from one stable configuration snapshot.
///
/// # Errors
///
/// Returns only fixed path-free startup errors.
pub fn load_taker_service_context(
    path: &Path,
) -> Result<ConfiguredTakerServiceContext, TakerServiceStartupError> {
    let bytes = read_private_file_snapshot(
        path,
        MAXIMUM_STARTUP_CONFIGURATION_BYTES,
        "Taker service startup configuration",
    )
    .map_err(|_| TakerServiceStartupError::ConfigurationUnavailable)?;
    let configuration: StartupConfigurationV1 = serde_json::from_slice(bytes.bytes())
        .map_err(|_| TakerServiceStartupError::InvalidConfiguration)?;
    validate_configuration(&configuration)?;

    let mut source_bindings = BTreeMap::new();
    let mut delivery_sources = Vec::with_capacity(configuration.delivery_sources.len());
    for (source_index, source) in configuration.delivery_sources.iter().enumerate() {
        let maker = PublicKey::from_slice(source.maker_public_key.as_bytes())
            .map_err(|_| TakerServiceStartupError::InvalidConfiguration)?;
        if let Some(source_id) = &source.source_id {
            source_bindings.insert(source_id.clone(), (source_index, maker.serialize()));
        }
        let subscriber = RunLocalDelivery::subscriber(source.directory.clone(), maker)
            .map_err(|_| TakerServiceStartupError::DeliveryUnavailable)?;
        delivery_sources.push(subscriber);
    }

    #[cfg(feature = "pair-zec")]
    let initiation = configuration
        .initiation
        .as_ref()
        .map(|configured| {
            prepared_zec::build_initiation_context(
                configured,
                &source_bindings,
                &delivery_sources,
                configuration.chat_socket.as_deref(),
            )
        })
        .transpose()?;
    let chat = configuration
        .chat_socket
        .clone()
        .map(OwnerChatSocketProbe::new)
        .transpose()?;
    let backend = TakerFacadeBackend::new(
        delivery_sources,
        SystemTakerTrustedTime,
        chat,
        configuration.maximum_offers,
    )
    .map_err(|_| TakerServiceStartupError::InvalidConfiguration)?;
    Ok(ConfiguredTakerServiceContext {
        backend,
        #[cfg(feature = "pair-zec")]
        initiation,
    })
}

/// Loads, validates, and constructs the discovery-only Taker backend.
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
    let context = load_taker_service_context(path)?;
    #[cfg(feature = "pair-zec")]
    {
        if context.initiation.is_some() {
            return Err(TakerServiceStartupError::InvalidConfiguration);
        }
    }
    Ok(context.into_backend())
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
            .any(|source| !validate_normalized_absolute(&source.directory))
        || configuration
            .chat_socket
            .as_ref()
            .is_some_and(|socket| !validate_normalized_absolute(socket))
    {
        return Err(TakerServiceStartupError::InvalidConfiguration);
    }

    let mut source_ids = BTreeSet::new();
    for source in &configuration.delivery_sources {
        if let Some(source_id) = source.source_id.as_deref()
            && (!valid_source_id(source_id) || !source_ids.insert(source_id))
        {
            return Err(TakerServiceStartupError::InvalidConfiguration);
        }
    }

    #[cfg(feature = "pair-zec")]
    {
        if let Some(initiation) = &configuration.initiation {
            prepared_zec::validate_initiation(
                initiation,
                &source_ids,
                configuration.chat_socket.is_some(),
            )?;
        }
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
    #[cfg(feature = "pair-zec")]
    #[serde(default)]
    initiation: Option<InitiationConfigurationV1>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DeliverySourceV1 {
    directory: PathBuf,
    maker_public_key: TakerMakerIdentityV1,
    #[serde(default)]
    source_id: Option<Box<str>>,
}

fn validate_normalized_absolute(path: &Path) -> bool {
    path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, Component::RootDir | Component::Normal(_)))
}

fn valid_source_id(source_id: &str) -> bool {
    !source_id.is_empty()
        && source_id.len() <= MAX_SOURCE_ID_BYTES
        && source_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}
