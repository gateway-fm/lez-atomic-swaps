//! Owner-private startup configuration for the read-only Taker service.

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt, fs,
    os::unix::fs::{FileTypeExt as _, MetadataExt as _},
    path::{Component, Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use lez_bridge_protocol::RequestId;
use lez_swap_core::{Pair, SwapId};
use lez_swap_store::{
    MakerOfferId, SqliteTakerFacadeStore, TakerInitiationAuthorityV1, TakerInitiationFactsV1,
    TakerPrivateFileBindingV1,
};
use secp256k1::{PublicKey, SecretKey};
use serde::Deserialize;
use sha2::{Digest as _, Sha256};
use thiserror::Error;
use zeroize::Zeroizing;

use crate::{
    AuthenticatedOfferRefV1, MAX_TAKER_DELIVERY_SOURCES_V1, MAX_TAKER_OFFER_RESULTS_V1,
    RunLocalDelivery, TakerDependencyProbe, TakerFacadeBackend, TakerMakerIdentityV1,
    TakerTrustedTimeSource,
    secure_file::{PrivateFileIdentity, PrivateFileSnapshot, read_private_file_snapshot},
};

const MAXIMUM_STARTUP_CONFIGURATION_BYTES: u64 = 512 * 1024;
const STARTUP_SCHEMA_VERSION: u16 = 1;

const MAX_PREPARED_ZEC_INITIATIONS: usize = 256;
const MAX_PREPARED_INPUT_BYTES: u64 = 256 * 1024;
const MAX_PREPARED_RECEIPT_BYTES: u64 = 16 * 1024;
const MAX_SIGNING_KEY_BYTES: u64 = 32;
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
    initiation: Option<ConfiguredTakerInitiationContext>,
}

impl ConfiguredTakerServiceContext {
    /// Borrows the optional mutation context.
    #[must_use]
    pub const fn initiation(&self) -> Option<&ConfiguredTakerInitiationContext> {
        self.initiation.as_ref()
    }

    /// Mutably borrows the optional mutation context.
    #[must_use]
    pub const fn initiation_mut(&mut self) -> Option<&mut ConfiguredTakerInitiationContext> {
        self.initiation.as_mut()
    }

    /// Consumes the context and returns the read backend.
    #[must_use]
    pub fn into_backend(self) -> ConfiguredTakerFacadeBackend {
        self.backend
    }

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
        formatter
            .debug_struct("ConfiguredTakerServiceContext")
            .field("backend", &self.backend)
            .field("initiation_configured", &self.initiation.is_some())
            .finish_non_exhaustive()
    }
}

/// Existing registry plus a bounded static catalog of prepared ZEC authorities.
pub struct ConfiguredTakerInitiationContext {
    execute_prepared_zec: bool,
    registry: SqliteTakerFacadeStore,
    prepared_zec_by_offer: BTreeMap<Box<str>, PreparedZecTakerInitiationV1>,
}

impl ConfiguredTakerInitiationContext {
    /// Whether admitted prepared ZEC swaps execute Chat acceptance before response.
    #[must_use]
    pub const fn execution_enabled(&self) -> bool {
        self.execute_prepared_zec
    }

    /// Number of configured role-fixed ZEC initiation entries.
    #[must_use]
    pub fn prepared_zec_count(&self) -> usize {
        self.prepared_zec_by_offer.len()
    }

    /// Looks up one fixed entry by authenticated offer identity.
    #[must_use]
    pub fn prepared_zec_for_offer(
        &self,
        offer_id: &MakerOfferId,
    ) -> Option<&PreparedZecTakerInitiationV1> {
        self.prepared_zec_by_offer.get(offer_id.as_str())
    }

    /// Looks up one fixed entry by application swap identity.
    ///
    /// Startup validation rejects duplicate swap identities and bounds the catalog.
    #[must_use]
    pub fn prepared_zec_for_swap(&self, swap_id: &SwapId) -> Option<&PreparedZecTakerInitiationV1> {
        self.prepared_zec_by_offer
            .values()
            .find(|prepared| prepared.swap_id() == swap_id)
    }

    /// Captures or revalidates the completed receipt for this process incarnation.
    pub(crate) fn bind_prepared_zec_receipt(&mut self, swap_id: &SwapId) -> Result<(), ()> {
        let prepared = self
            .prepared_zec_by_offer
            .values_mut()
            .find(|prepared| prepared.swap_id() == swap_id)
            .ok_or(())?;
        let binding =
            load_required_receipt_binding(prepared.execution.receipt_output()).map_err(|_| ())?;
        if prepared
            .execution
            .receipt_binding
            .is_some_and(|expected| expected != binding)
        {
            return Err(());
        }
        prepared.execution.receipt_binding = Some(binding);
        Ok(())
    }

    /// Mutably borrows the already-existing standalone registry.
    #[must_use]
    pub const fn registry_mut(&mut self) -> &mut SqliteTakerFacadeStore {
        &mut self.registry
    }
}

impl fmt::Debug for ConfiguredTakerInitiationContext {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ConfiguredTakerInitiationContext")
            .field("execution_enabled", &self.execute_prepared_zec)
            .field("registry", &"[REDACTED]")
            .field("prepared_zec_count", &self.prepared_zec_by_offer.len())
            .finish_non_exhaustive()
    }
}

/// Static service-owned authority selected after a client supplies public facts.
#[derive(Clone)]
pub struct PreparedZecTakerInitiationV1 {
    facts: TakerInitiationFactsV1,
    reservation_id: RequestId,
    authority: TakerInitiationAuthorityV1,
    execution: PreparedZecExecutionV1,
}

impl PreparedZecTakerInitiationV1 {
    /// Exact route-fixed public facts prepared by the owner.
    #[must_use]
    pub const fn facts(&self) -> &TakerInitiationFactsV1 {
        &self.facts
    }

    /// Fixed application swap identity; never supplied by the client.
    #[must_use]
    pub const fn swap_id(&self) -> &SwapId {
        self.facts.swap_id()
    }

    /// Fixed authenticated offer identity.
    #[must_use]
    pub const fn offer_id(&self) -> &MakerOfferId {
        self.facts.offer_id()
    }

    /// Fixed Maker reservation identity.
    pub const fn reservation_id(&self) -> &RequestId {
        &self.reservation_id
    }

    /// Maker identity derived from the referenced Delivery source.
    #[must_use]
    pub const fn maker_identity(&self) -> &[u8; 33] {
        self.facts.maker_identity()
    }

    /// Borrows the complete redacted authority for atomic registry admission.
    #[must_use]
    pub const fn authority(&self) -> &TakerInitiationAuthorityV1 {
        &self.authority
    }

    /// Borrows the redacted execution-ready material retained at startup.
    #[must_use]
    pub const fn execution(&self) -> &PreparedZecExecutionV1 {
        &self.execution
    }
}

impl fmt::Debug for PreparedZecTakerInitiationV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PreparedZecTakerInitiationV1")
            .field("configured", &true)
            .finish_non_exhaustive()
    }
}

/// Process-incarnation binding for one completed prepared receipt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PreparedReceiptBindingV1 {
    sha256: [u8; 32],
    identity: PrivateFileIdentity,
}

impl PreparedReceiptBindingV1 {
    pub(crate) const fn sha256(self) -> [u8; 32] {
        self.sha256
    }

    pub(crate) const fn identity(self) -> PrivateFileIdentity {
        self.identity
    }
}

/// Cloneable, execution-ready ZEC input retained inside the owner process.
///
/// Its public surface is intentionally opaque: only the service executor in
/// this crate can borrow private bytes or configured paths, while `Debug`
/// never reveals either.
#[derive(Clone)]
pub struct PreparedZecExecutionV1 {
    authenticated_offer: AuthenticatedOfferRefV1,
    unsigned_draft_path: PathBuf,
    unsigned_draft: Zeroizing<Vec<u8>>,
    unsigned_draft_sha256: [u8; 32],
    signing_key_path: PathBuf,
    signing_key: Zeroizing<[u8; 32]>,
    source_config_path: PathBuf,
    source_config_sha256: [u8; 32],
    chat_socket: PathBuf,
    agreement_output: PathBuf,
    actor_root: PathBuf,
    receipt_output: PathBuf,
    receipt_binding: Option<PreparedReceiptBindingV1>,
}

#[allow(dead_code)]
impl PreparedZecExecutionV1 {
    pub(crate) const fn authenticated_offer(&self) -> &AuthenticatedOfferRefV1 {
        &self.authenticated_offer
    }

    pub(crate) fn unsigned_draft_path(&self) -> &Path {
        &self.unsigned_draft_path
    }

    pub(crate) fn unsigned_draft(&self) -> &[u8] {
        &self.unsigned_draft
    }

    pub(crate) const fn unsigned_draft_sha256(&self) -> [u8; 32] {
        self.unsigned_draft_sha256
    }

    pub(crate) fn signing_key(&self) -> &[u8; 32] {
        &self.signing_key
    }

    pub(crate) fn signing_key_path(&self) -> &Path {
        &self.signing_key_path
    }

    pub(crate) fn source_config_path(&self) -> &Path {
        &self.source_config_path
    }

    pub(crate) const fn source_config_sha256(&self) -> [u8; 32] {
        self.source_config_sha256
    }

    pub(crate) fn chat_socket(&self) -> &Path {
        &self.chat_socket
    }

    pub(crate) fn agreement_output(&self) -> &Path {
        &self.agreement_output
    }

    pub(crate) fn actor_root(&self) -> &Path {
        &self.actor_root
    }

    pub(crate) fn receipt_output(&self) -> &Path {
        &self.receipt_output
    }

    pub(crate) const fn receipt_binding(&self) -> Option<PreparedReceiptBindingV1> {
        self.receipt_binding
    }
}

impl fmt::Debug for PreparedZecExecutionV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PreparedZecExecutionV1")
            .field("configured", &true)
            .finish_non_exhaustive()
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

    let initiation = configuration
        .initiation
        .as_ref()
        .map(|configured| {
            build_initiation_context(
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
        initiation,
    })
}

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
    let context = load_taker_service_context(path)?;
    if context.initiation.is_some() {
        return Err(TakerServiceStartupError::InvalidConfiguration);
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

    let Some(initiation) = &configuration.initiation else {
        return Ok(());
    };
    if initiation.prepared_zec.len() > MAX_PREPARED_ZEC_INITIATIONS
        || !validate_normalized_absolute(&initiation.registry_database)
        || (!initiation.prepared_zec.is_empty() && configuration.chat_socket.is_none())
    {
        return Err(TakerServiceStartupError::InvalidConfiguration);
    }

    let mut swaps = BTreeSet::new();
    let mut offers = BTreeSet::new();
    let mut reservations = BTreeSet::new();
    let mut paths = BTreeSet::from([initiation.registry_database.as_path()]);
    for prepared in &initiation.prepared_zec {
        if !source_ids.contains(prepared.source_id.as_ref())
            || !swaps.insert(prepared.swap_id.as_str())
            || !offers.insert(prepared.offer_id.as_str())
            || !reservations.insert(prepared.reservation_id.as_str())
            || prepared.foreign_units == 0
            || prepared.lez_units == 0
        {
            return Err(TakerServiceStartupError::InvalidConfiguration);
        }
        for path in [
            &prepared.signed_envelope.path,
            &prepared.unsigned_draft.path,
            &prepared.signing_key.path,
            &prepared.source_config.path,
            &prepared.agreement_output,
            &prepared.actor_root,
            &prepared.receipt_output,
        ] {
            if !validate_normalized_absolute(path) || !paths.insert(path.as_path()) {
                return Err(TakerServiceStartupError::InvalidConfiguration);
            }
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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InitiationConfigurationV1 {
    #[serde(default)]
    execute_prepared_zec: bool,
    registry_database: PathBuf,
    prepared_zec: Vec<PreparedZecConfigurationV1>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PreparedZecConfigurationV1 {
    source_id: Box<str>,
    swap_id: SwapId,
    offer_id: MakerOfferId,
    reservation_id: RequestId,
    foreign_units: u64,
    lez_units: u128,
    signed_envelope: ImmutablePrivateFileV1,
    unsigned_draft: ImmutablePrivateFileV1,
    signing_key: SecretPrivateFileV1,
    source_config: ImmutablePrivateFileV1,
    agreement_output: PathBuf,
    actor_root: PathBuf,
    receipt_output: PathBuf,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ImmutablePrivateFileV1 {
    path: PathBuf,
    #[serde(deserialize_with = "deserialize_sha256")]
    sha256: [u8; 32],
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SecretPrivateFileV1 {
    path: PathBuf,
}

fn build_initiation_context(
    configuration: &InitiationConfigurationV1,
    source_bindings: &BTreeMap<Box<str>, (usize, [u8; 33])>,
    delivery_sources: &[RunLocalDelivery],
    chat_socket: Option<&Path>,
) -> Result<ConfiguredTakerInitiationContext, TakerServiceStartupError> {
    let registry = SqliteTakerFacadeStore::open_existing(&configuration.registry_database)
        .map_err(|_| TakerServiceStartupError::InitiationUnavailable)?;
    let mut prepared_zec_by_offer = BTreeMap::new();

    for configured in &configuration.prepared_zec {
        let (source_index, maker_identity) = source_bindings
            .get(&configured.source_id)
            .copied()
            .ok_or(TakerServiceStartupError::InvalidConfiguration)?;
        let (signed_envelope, signed_snapshot) = read_immutable_snapshot_binding(
            &configured.signed_envelope,
            "prepared Taker signed envelope",
        )?;
        let authenticated = delivery_sources
            .get(source_index)
            .ok_or(TakerServiceStartupError::InvalidConfiguration)?
            .authenticate_envelope(signed_snapshot.bytes())
            .map_err(|_| TakerServiceStartupError::InvalidConfiguration)?;
        let route = authenticated.offer().route();
        if authenticated.maker_identity() != &maker_identity
            || authenticated.offer().id() != &configured.offer_id
            || route.pair() != Pair::Zcash
            || authenticated
                .offer()
                .quote_foreign_amount(configured.foreign_units)
                .ok()
                != Some(configured.lez_units)
            || authenticated.commitment() != configured.signed_envelope.sha256
        {
            return Err(TakerServiceStartupError::InvalidConfiguration);
        }
        let (unsigned_draft, unsigned_draft_snapshot) = read_immutable_snapshot_binding(
            &configured.unsigned_draft,
            "prepared Taker unsigned draft",
        )?;
        let (signing_key, signing_key_bytes) =
            read_secret_binding(&configured.signing_key, "prepared Taker signing key")?;
        let source_config = read_immutable_binding(
            &configured.source_config,
            "prepared Taker actor configuration",
        )?;
        let authority = TakerInitiationAuthorityV1::new(
            configured.source_id.clone(),
            configured.reservation_id.clone(),
            signed_envelope,
            unsigned_draft,
            signing_key,
            source_config,
            configured.agreement_output.clone(),
            configured.actor_root.clone(),
            configured.receipt_output.clone(),
        )
        .map_err(|_| TakerServiceStartupError::InvalidConfiguration)?;
        let facts = TakerInitiationFactsV1::new(
            configured.swap_id.clone(),
            configured.offer_id.clone(),
            route,
            maker_identity,
            configured.signed_envelope.sha256,
            configured.foreign_units,
            configured.lez_units,
        )
        .map_err(|_| TakerServiceStartupError::InvalidConfiguration)?;
        let execution = PreparedZecExecutionV1 {
            authenticated_offer: authenticated,
            unsigned_draft_path: configured.unsigned_draft.path.clone(),
            unsigned_draft: unsigned_draft_snapshot.into_bytes(),
            unsigned_draft_sha256: configured.unsigned_draft.sha256,
            signing_key_path: configured.signing_key.path.clone(),
            signing_key: signing_key_bytes,
            source_config_path: configured.source_config.path.clone(),
            source_config_sha256: configured.source_config.sha256,
            chat_socket: chat_socket
                .ok_or(TakerServiceStartupError::InvalidConfiguration)?
                .to_path_buf(),
            agreement_output: configured.agreement_output.clone(),
            actor_root: configured.actor_root.clone(),
            receipt_output: configured.receipt_output.clone(),
            receipt_binding: load_optional_receipt_binding(&configured.receipt_output)?,
        };
        let entry = PreparedZecTakerInitiationV1 {
            facts,
            reservation_id: configured.reservation_id.clone(),
            authority,
            execution,
        };
        if prepared_zec_by_offer
            .insert(configured.offer_id.as_str().into(), entry)
            .is_some()
        {
            return Err(TakerServiceStartupError::InvalidConfiguration);
        }
    }

    Ok(ConfiguredTakerInitiationContext {
        execute_prepared_zec: configuration.execute_prepared_zec,
        registry,
        prepared_zec_by_offer,
    })
}

fn load_optional_receipt_binding(
    path: &Path,
) -> Result<Option<PreparedReceiptBindingV1>, TakerServiceStartupError> {
    match fs::symlink_metadata(path) {
        Ok(_) => load_required_receipt_binding(path).map(Some),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err(TakerServiceStartupError::InitiationUnavailable),
    }
}

fn load_required_receipt_binding(
    path: &Path,
) -> Result<PreparedReceiptBindingV1, TakerServiceStartupError> {
    let snapshot = read_private_file_snapshot(
        path,
        MAX_PREPARED_RECEIPT_BYTES,
        "prepared Taker acceptance receipt",
    )
    .map_err(|_| TakerServiceStartupError::InitiationUnavailable)?;
    Ok(PreparedReceiptBindingV1 {
        sha256: Sha256::digest(snapshot.bytes()).into(),
        identity: snapshot.identity(),
    })
}

fn read_immutable_binding(
    configured: &ImmutablePrivateFileV1,
    purpose: &str,
) -> Result<TakerPrivateFileBindingV1, TakerServiceStartupError> {
    read_immutable_snapshot_binding(configured, purpose).map(|(binding, _snapshot)| binding)
}

fn read_immutable_snapshot_binding(
    configured: &ImmutablePrivateFileV1,
    purpose: &str,
) -> Result<(TakerPrivateFileBindingV1, PrivateFileSnapshot), TakerServiceStartupError> {
    let snapshot = read_prepared_snapshot(&configured.path, MAX_PREPARED_INPUT_BYTES, purpose)?;
    if Sha256::digest(snapshot.bytes()).as_slice() != configured.sha256 {
        return Err(TakerServiceStartupError::InvalidConfiguration);
    }
    let identity = snapshot.identity();
    let binding = TakerPrivateFileBindingV1::immutable(
        configured.path.clone(),
        configured.sha256,
        identity.device(),
        identity.inode(),
    )
    .map_err(|_| TakerServiceStartupError::InvalidConfiguration)?;
    Ok((binding, snapshot))
}

fn read_secret_binding(
    configured: &SecretPrivateFileV1,
    purpose: &str,
) -> Result<(TakerPrivateFileBindingV1, Zeroizing<[u8; 32]>), TakerServiceStartupError> {
    let snapshot = read_prepared_snapshot(&configured.path, MAX_SIGNING_KEY_BYTES, purpose)?;
    let identity = snapshot.identity();
    if snapshot.bytes().len() != 32 || SecretKey::from_slice(snapshot.bytes()).is_err() {
        return Err(TakerServiceStartupError::InvalidConfiguration);
    }

    let mut key_bytes = Zeroizing::new([0_u8; 32]);
    key_bytes.copy_from_slice(snapshot.bytes());
    let binding = TakerPrivateFileBindingV1::secret(
        configured.path.clone(),
        identity.device(),
        identity.inode(),
    )
    .map_err(|_| TakerServiceStartupError::InvalidConfiguration)?;
    Ok((binding, key_bytes))
}

fn read_prepared_snapshot(
    path: &Path,
    maximum_bytes: u64,
    purpose: &str,
) -> Result<PrivateFileSnapshot, TakerServiceStartupError> {
    read_private_file_snapshot(path, maximum_bytes, purpose)
        .map_err(|_| TakerServiceStartupError::InitiationUnavailable)
}

fn deserialize_sha256<'de, D>(deserializer: D) -> Result<[u8; 32], D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::Error as _;

    let encoded = Box::<str>::deserialize(deserializer)?;
    if encoded.len() != 64
        || encoded
            .bytes()
            .any(|byte| !byte.is_ascii_digit() && !(b'a'..=b'f').contains(&byte))
    {
        return Err(D::Error::custom("invalid SHA-256"));
    }
    let mut digest = [0_u8; 32];
    hex::decode_to_slice(encoded.as_bytes(), &mut digest)
        .map_err(|_| D::Error::custom("invalid SHA-256"))?;
    Ok(digest)
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
