//! Isolated one-shot worker for sealed XMR claim-authorization publication.

#![forbid(unsafe_code)]

use std::{
    fmt, fs,
    fs::File,
    io::Read as _,
    net::IpAddr,
    os::unix::fs::{MetadataExt as _, PermissionsExt as _},
    path::{Path, PathBuf},
    time::Duration,
};

use async_trait::async_trait;
use indexer_service_protocol::{BedrockStatus, Block, HashType as IndexedHash};
use indexer_service_rpc::RpcClient as _;
use jsonrpsee::http_client::{HttpClient, HttpClientBuilder};
use lez_bridge_adapter::{
    CapabilityFileXmrReleaseClientFactory, FreshLezBridgeTransportFactory as _,
};
use lez_bridge_protocol::{
    ChainClock, Hex32, MessageContext, Participant, RequestId, RunId, RuntimeDescriptor,
    XmrNativeEscrowTermsV3,
};
use lez_xmr_release_authority::{
    FinalizedLezClockError, FinalizedLezClockSource, PublicationAdmissionStatus,
    PublicationProtectionKey, ReleasePublicationOutcome, ReleaseState, ReleaseStore,
    XmrReleaseSubmissionBindingV3,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use url::{Host, Url};

const MAX_PUBLIC_CONFIG_BYTES: usize = 64 * 1024;
const MAX_PUBLIC_CONFIG_BYTES_U64: u64 = MAX_PUBLIC_CONFIG_BYTES as u64;
const RELEASE_REQUEST_TIMEOUT: Duration = Duration::from_mins(1);
const RELEASE_JOURNAL_NAME: &str = "xmr-release.sqlite3";
const INDEXER_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_INDEXER_REQUEST_BYTES: u32 = 2_800_000;
const MAX_INDEXER_RESPONSE_BYTES: u32 = 8 * 1024 * 1024;
const OFFICIAL_PUBLIC_INDEXER_ENDPOINT: &str = "https://testnet.lez.logos.co/";
const PINNED_V0_2_GENESIS_BLOCK_ID: u64 = 1;

/// Current strict public configuration schema.
pub const XMR_RELEASE_SERVICE_SCHEMA_VERSION: u16 = 1;

/// Complete outbound finalized-indexer route.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReleaseNodeRouteProfile {
    /// Explicit literal-loopback HTTP indexer.
    Local,
    /// Exact allowlisted official LEZ Testnet HTTPS origin.
    OfficialPublic,
}

impl ReleaseNodeRouteProfile {
    fn validate_indexer_endpoint(self, endpoint: &str) -> Result<(), XmrReleaseServiceError> {
        match self {
            Self::Local => validate_loopback_http_endpoint(endpoint),
            Self::OfficialPublic => validate_official_public_endpoint(endpoint),
        }
    }

    fn connect_indexer(
        self,
        endpoint: &str,
    ) -> Result<OfficialFinalizedClock, XmrReleaseServiceError> {
        self.validate_indexer_endpoint(endpoint)?;
        OfficialFinalizedClock::connect(endpoint)
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::OfficialPublic => "official_public",
        }
    }
}

/// Non-secret configuration for one exact publication attempt.
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct XmrReleaseServiceConfig {
    /// Strict config schema.
    pub schema_version: u16,
    /// Release-only bridge listener. It always remains literal loopback.
    pub sidecar_endpoint: String,
    /// Finalized official indexer endpoint selected by the node profile.
    pub indexer_endpoint: String,
    /// Complete local or official-public route policy.
    pub node_profile: ReleaseNodeRouteProfile,
    /// Composed run identity.
    pub run_id: RunId,
    /// Exact immutable Taker runtime.
    pub runtime: RuntimeDescriptor,
    /// Exact countersigned XMR-native escrow terms.
    pub terms: XmrNativeEscrowTermsV3,
    /// Non-secret journal-key rotation identifier.
    pub protection_key_id: String,
}

impl fmt::Debug for XmrReleaseServiceConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("XmrReleaseServiceConfig")
            .field("schema_version", &self.schema_version)
            .field("sidecar_endpoint", &self.sidecar_endpoint)
            .field("indexer_endpoint", &self.indexer_endpoint)
            .field("node_profile", &self.node_profile)
            .field("run_id", &self.run_id)
            .field("runtime", &self.runtime)
            .field("terms", &"[BOUND]")
            .field("protection_key_id", &self.protection_key_id)
            .finish()
    }
}

/// Owner-private inputs held by the release process.
pub struct XmrReleaseServicePaths {
    capability_file: PathBuf,
    protection_key_file: PathBuf,
    state_directory: PathBuf,
}

impl XmrReleaseServicePaths {
    /// Binds the two credential files and fixed journal directory without opening them.
    #[must_use]
    pub fn new(
        capability_file: impl Into<PathBuf>,
        protection_key_file: impl Into<PathBuf>,
        state_directory: impl Into<PathBuf>,
    ) -> Self {
        Self {
            capability_file: capability_file.into(),
            protection_key_file: protection_key_file.into(),
            state_directory: state_directory.into(),
        }
    }

    fn journal_path(&self) -> PathBuf {
        self.state_directory.join(RELEASE_JOURNAL_NAME)
    }
}

impl fmt::Debug for XmrReleaseServicePaths {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("XmrReleaseServicePaths")
            .field("capability_file", &"[REDACTED]")
            .field("protection_key_file", &"[REDACTED]")
            .field("state_directory", &"[REDACTED]")
            .finish()
    }
}

/// Allowlisted payload-free process outcome.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum XmrReleaseServiceOutcome {
    /// The node accepted the exact authorization.
    AdmittedAccepted,
    /// The node already knew the exact authorization.
    AdmittedAlreadyKnown,
    /// The sole send may have reached the node; no retry is allowed.
    Ambiguous,
    /// The post-CAS clock gate proved that no send was made.
    Suppressed,
    /// A prior process owns or completed the sole attempt.
    ObserveOnly,
}

impl From<ReleasePublicationOutcome> for XmrReleaseServiceOutcome {
    fn from(value: ReleasePublicationOutcome) -> Self {
        match value {
            ReleasePublicationOutcome::Admitted(PublicationAdmissionStatus::Accepted) => {
                Self::AdmittedAccepted
            }
            ReleasePublicationOutcome::Admitted(PublicationAdmissionStatus::AlreadyKnown) => {
                Self::AdmittedAlreadyKnown
            }
            ReleasePublicationOutcome::Ambiguous => Self::Ambiguous,
            ReleasePublicationOutcome::Suppressed => Self::Suppressed,
            ReleasePublicationOutcome::ObserveOnly => Self::ObserveOnly,
        }
    }
}

/// Allowlisted authenticated journal state after the attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum XmrReleaseDurableState {
    /// Prepared and no process has won the send CAS.
    Prepared,
    /// A process won the CAS and may have been interrupted.
    PublicationStarted,
    /// Exact node admission is durably recorded.
    Admitted,
    /// Node admission is uncertain and retry is forbidden.
    Ambiguous,
    /// The decisive clock gate suppressed publication.
    Suppressed,
}

impl From<ReleaseState> for XmrReleaseDurableState {
    fn from(value: ReleaseState) -> Self {
        match value {
            ReleaseState::Prepared => Self::Prepared,
            ReleaseState::PublicationStarted => Self::PublicationStarted,
            ReleaseState::Admitted => Self::Admitted,
            ReleaseState::Ambiguous => Self::Ambiguous,
            ReleaseState::Suppressed => Self::Suppressed,
        }
    }
}

/// Exact JSON emitted by the one-shot process.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct XmrReleaseServiceReport {
    /// Strict report schema.
    pub schema_version: u16,
    /// Stable event name.
    pub event: &'static str,
    /// Redacted publication result.
    pub outcome: XmrReleaseServiceOutcome,
    /// Authenticated durable state after reloading the journal.
    pub durable_state: XmrReleaseDurableState,
    /// Selected public node route.
    pub node_profile: &'static str,
}

impl XmrReleaseServiceReport {
    fn new(
        outcome: ReleasePublicationOutcome,
        durable_state: ReleaseState,
        profile: ReleaseNodeRouteProfile,
    ) -> Self {
        Self {
            schema_version: XMR_RELEASE_SERVICE_SCHEMA_VERSION,
            event: "xmr_claim_authorization_publication",
            outcome: outcome.into(),
            durable_state: durable_state.into(),
            node_profile: profile.as_str(),
        }
    }

    /// Returns true only when exact node admission is durable.
    #[must_use]
    pub const fn is_durably_admitted(self) -> bool {
        matches!(self.durable_state, XmrReleaseDurableState::Admitted)
    }
}

/// Redacted one-shot service failure.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum XmrReleaseServiceError {
    /// Public JSON or route configuration was absent, unstable, or invalid.
    #[error("XMR release public configuration is invalid")]
    InvalidPublicConfiguration,
    /// Private inputs alias or cannot be established safely.
    #[error("XMR release private file layout is invalid")]
    InvalidPrivateFileLayout,
    /// The journal protection key could not be safely loaded.
    #[error("XMR release protection key is unavailable")]
    ProtectionKeyUnavailable,
    /// The authenticated release-only client could not be built.
    #[error("XMR release client is unavailable")]
    ReleaseClientUnavailable,
    /// The sealed journal could not be opened or authenticated.
    #[error("XMR release journal is unavailable")]
    ReleaseJournalUnavailable,
    /// Publication failed before a durable terminal outcome.
    #[error("XMR release publication failed")]
    PublicationFailed,
}

#[derive(Clone)]
struct OfficialFinalizedClock {
    client: HttpClient,
}

impl fmt::Debug for OfficialFinalizedClock {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OfficialFinalizedClock")
            .finish_non_exhaustive()
    }
}

impl OfficialFinalizedClock {
    fn connect(endpoint: &str) -> Result<Self, XmrReleaseServiceError> {
        let client = HttpClientBuilder::default()
            .max_request_size(MAX_INDEXER_REQUEST_BYTES)
            .max_response_size(MAX_INDEXER_RESPONSE_BYTES)
            .request_timeout(INDEXER_REQUEST_TIMEOUT)
            .max_concurrent_requests(1)
            .build(endpoint)
            .map_err(|_| XmrReleaseServiceError::InvalidPublicConfiguration)?;
        Ok(Self { client })
    }

    async fn stable_genesis_bound_clock(
        &self,
        expected_genesis_hash: Hex32,
    ) -> Result<ChainClock, FinalizedLezClockError> {
        if expected_genesis_hash == Hex32::from_bytes([0; 32]) {
            return Err(FinalizedLezClockError::Unavailable);
        }
        let finalized_before = self
            .client
            .get_last_finalized_block_id()
            .await
            .map_err(|_| FinalizedLezClockError::Unavailable)?
            .ok_or(FinalizedLezClockError::Unavailable)?;
        if finalized_before < PINNED_V0_2_GENESIS_BLOCK_ID {
            return Err(FinalizedLezClockError::Unavailable);
        }
        let genesis_before = self
            .read_finalized_block(PINNED_V0_2_GENESIS_BLOCK_ID)
            .await?;
        if genesis_before.header.hash.0 != *expected_genesis_hash.as_bytes() {
            return Err(FinalizedLezClockError::Unavailable);
        }
        let tip_before = self.read_finalized_block(finalized_before).await?;
        if tip_before.header.hash.0 == [0; 32] || tip_before.header.timestamp == 0 {
            return Err(FinalizedLezClockError::Unavailable);
        }
        let genesis_after = self
            .read_finalized_block(PINNED_V0_2_GENESIS_BLOCK_ID)
            .await?;
        let tip_after = self.read_finalized_block(finalized_before).await?;
        if genesis_after != genesis_before || tip_after != tip_before {
            return Err(FinalizedLezClockError::Unavailable);
        }
        let finalized_after = self
            .client
            .get_last_finalized_block_id()
            .await
            .map_err(|_| FinalizedLezClockError::Unavailable)?
            .ok_or(FinalizedLezClockError::Unavailable)?;
        if finalized_after != finalized_before {
            return Err(FinalizedLezClockError::Unavailable);
        }
        Ok(ChainClock::new(
            Hex32::from_bytes(tip_before.header.hash.0),
            tip_before.header.block_id,
            tip_before.header.timestamp,
        ))
    }

    async fn read_finalized_block(&self, block_id: u64) -> Result<Block, FinalizedLezClockError> {
        let by_id = self
            .client
            .get_block_by_id(block_id)
            .await
            .map_err(|_| FinalizedLezClockError::Unavailable)?
            .ok_or(FinalizedLezClockError::Unavailable)?;
        if by_id.header.block_id != block_id || by_id.bedrock_status != BedrockStatus::Finalized {
            return Err(FinalizedLezClockError::Unavailable);
        }
        let by_hash = self
            .client
            .get_block_by_hash(IndexedHash(by_id.header.hash.0))
            .await
            .map_err(|_| FinalizedLezClockError::Unavailable)?
            .ok_or(FinalizedLezClockError::Unavailable)?;
        if by_hash != by_id {
            return Err(FinalizedLezClockError::Unavailable);
        }
        Ok(by_id)
    }
}

#[async_trait]
impl FinalizedLezClockSource for OfficialFinalizedClock {
    async fn read_genesis_bound_finalized_clock(
        &mut self,
        expected_genesis_block_hash: Hex32,
    ) -> Result<ChainClock, FinalizedLezClockError> {
        self.stable_genesis_bound_clock(expected_genesis_block_hash)
            .await
    }
}

/// Reads one bounded stable regular public JSON configuration.
///
/// # Errors
///
/// Rejects missing, symlinked, empty, oversized, replaced, or invalid JSON files.
pub fn read_xmr_release_service_config(
    path: impl AsRef<Path>,
) -> Result<XmrReleaseServiceConfig, XmrReleaseServiceError> {
    let path = path.as_ref();
    let before = fs::symlink_metadata(path)
        .map_err(|_| XmrReleaseServiceError::InvalidPublicConfiguration)?;
    validate_public_metadata(&before)?;
    let file = File::open(path).map_err(|_| XmrReleaseServiceError::InvalidPublicConfiguration)?;
    let opened = file
        .metadata()
        .map_err(|_| XmrReleaseServiceError::InvalidPublicConfiguration)?;
    validate_public_metadata(&opened)?;
    if !same_public_file(&before, &opened) {
        return Err(XmrReleaseServiceError::InvalidPublicConfiguration);
    }
    let mut bytes =
        Vec::with_capacity(usize::try_from(opened.len()).unwrap_or(MAX_PUBLIC_CONFIG_BYTES) + 1);
    (&file)
        .take(MAX_PUBLIC_CONFIG_BYTES_U64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| XmrReleaseServiceError::InvalidPublicConfiguration)?;
    let opened_after = file
        .metadata()
        .map_err(|_| XmrReleaseServiceError::InvalidPublicConfiguration)?;
    let path_after = fs::symlink_metadata(path)
        .map_err(|_| XmrReleaseServiceError::InvalidPublicConfiguration)?;
    if !stable_public_file(&opened, &opened_after)
        || !stable_public_file(&opened, &path_after)
        || bytes.is_empty()
        || bytes.len() > MAX_PUBLIC_CONFIG_BYTES
    {
        return Err(XmrReleaseServiceError::InvalidPublicConfiguration);
    }
    serde_json::from_slice(&bytes).map_err(|_| XmrReleaseServiceError::InvalidPublicConfiguration)
}

/// Executes the sole prepared-journal publication attempt.
///
/// The returned report contains no transaction ID, authorization, bearer,
/// protection-key material, or private path.
///
/// # Errors
///
/// Fails closed on configuration, credential, journal, clock, or publication errors.
pub async fn run_xmr_release_service_once(
    config: XmrReleaseServiceConfig,
    paths: &XmrReleaseServicePaths,
) -> Result<XmrReleaseServiceReport, XmrReleaseServiceError> {
    validate_config(&config)?;
    validate_private_paths(paths)?;
    let mut clock = config
        .node_profile
        .connect_indexer(&config.indexer_endpoint)?;
    let binding = XmrReleaseSubmissionBindingV3::new(
        config.run_id.clone(),
        config.runtime.clone(),
        config.terms,
    )
    .map_err(|_| XmrReleaseServiceError::InvalidPublicConfiguration)?;
    let key = PublicationProtectionKey::from_owner_private_file(
        config.protection_key_id,
        &paths.protection_key_file,
    )
    .map_err(|_| XmrReleaseServiceError::ProtectionKeyUnavailable)?;
    let journal_path = paths.journal_path();
    let store = ReleaseStore::open(&journal_path)
        .map_err(|_| XmrReleaseServiceError::ReleaseJournalUnavailable)?;
    let swap_id = *config.terms.to_input().swap_id.as_bytes();
    let snapshot = store
        .load_xmr_claim_release(swap_id, &config.run_id, &key)
        .map_err(|_| XmrReleaseServiceError::ReleaseJournalUnavailable)?;
    let client = CapabilityFileXmrReleaseClientFactory::new(
        config.sidecar_endpoint,
        &paths.capability_file,
        config.run_id.clone(),
        config.runtime,
        RELEASE_REQUEST_TIMEOUT,
    )
    .fresh_transport()
    .map_err(|_| XmrReleaseServiceError::ReleaseClientUnavailable)?;
    let outcome = store
        .publish_xmr_claim_release(snapshot, &key, &binding, &client, &mut clock)
        .await
        .map_err(|_| XmrReleaseServiceError::PublicationFailed)?;
    let terminal = store
        .load_xmr_claim_release(swap_id, &config.run_id, &key)
        .map_err(|_| XmrReleaseServiceError::ReleaseJournalUnavailable)?;
    Ok(XmrReleaseServiceReport::new(
        outcome,
        terminal.state(),
        config.node_profile,
    ))
}

fn validate_config(config: &XmrReleaseServiceConfig) -> Result<(), XmrReleaseServiceError> {
    if config.schema_version != XMR_RELEASE_SERVICE_SCHEMA_VERSION
        || config.protection_key_id.is_empty()
        || config.protection_key_id.len() > 128
    {
        return Err(XmrReleaseServiceError::InvalidPublicConfiguration);
    }
    validate_loopback_http_endpoint(&config.sidecar_endpoint)?;
    config
        .node_profile
        .validate_indexer_endpoint(&config.indexer_endpoint)?;
    let context = MessageContext::new(
        config.run_id.clone(),
        RequestId::new("release-config")
            .map_err(|_| XmrReleaseServiceError::InvalidPublicConfiguration)?,
        Participant::Taker,
    );
    config
        .terms
        .validate_runtime_binding(&context, &config.runtime)
        .map_err(|_| XmrReleaseServiceError::InvalidPublicConfiguration)
}

fn validate_loopback_http_endpoint(endpoint: &str) -> Result<(), XmrReleaseServiceError> {
    let parsed =
        Url::parse(endpoint).map_err(|_| XmrReleaseServiceError::InvalidPublicConfiguration)?;
    let loopback = match parsed.host() {
        Some(Host::Ipv4(address)) => IpAddr::V4(address).is_loopback(),
        Some(Host::Ipv6(address)) => IpAddr::V6(address).is_loopback(),
        Some(Host::Domain(_)) | None => false,
    };
    if parsed.scheme() != "http"
        || !loopback
        || parsed.port().is_none_or(|port| port == 0)
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
        || parsed.path() != "/"
    {
        return Err(XmrReleaseServiceError::InvalidPublicConfiguration);
    }
    Ok(())
}

fn validate_official_public_endpoint(endpoint: &str) -> Result<(), XmrReleaseServiceError> {
    let parsed =
        Url::parse(endpoint).map_err(|_| XmrReleaseServiceError::InvalidPublicConfiguration)?;
    if endpoint != OFFICIAL_PUBLIC_INDEXER_ENDPOINT
        || parsed.scheme() != "https"
        || parsed.host_str() != Some("testnet.lez.logos.co")
        || parsed.port().is_some()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.path() != "/"
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return Err(XmrReleaseServiceError::InvalidPublicConfiguration);
    }
    Ok(())
}

fn validate_private_paths(paths: &XmrReleaseServicePaths) -> Result<(), XmrReleaseServiceError> {
    let state_metadata = fs::symlink_metadata(&paths.state_directory)
        .map_err(|_| XmrReleaseServiceError::InvalidPrivateFileLayout)?;
    if !state_metadata.file_type().is_dir()
        || state_metadata.uid() != rustix::process::geteuid().as_raw()
        || state_metadata.permissions().mode() & 0o7777 != 0o700
    {
        return Err(XmrReleaseServiceError::InvalidPrivateFileLayout);
    }
    let capability = fs::canonicalize(&paths.capability_file)
        .map_err(|_| XmrReleaseServiceError::InvalidPrivateFileLayout)?;
    let protection_key = fs::canonicalize(&paths.protection_key_file)
        .map_err(|_| XmrReleaseServiceError::InvalidPrivateFileLayout)?;
    let state_directory = fs::canonicalize(&paths.state_directory)
        .map_err(|_| XmrReleaseServiceError::InvalidPrivateFileLayout)?;
    let journal = fs::canonicalize(paths.journal_path())
        .map_err(|_| XmrReleaseServiceError::InvalidPrivateFileLayout)?;
    if capability == protection_key
        || capability == journal
        || protection_key == journal
        || capability == state_directory
        || protection_key == state_directory
    {
        Err(XmrReleaseServiceError::InvalidPrivateFileLayout)
    } else {
        Ok(())
    }
}

fn validate_public_metadata(metadata: &fs::Metadata) -> Result<(), XmrReleaseServiceError> {
    if !metadata.file_type().is_file()
        || metadata.len() == 0
        || metadata.len() > MAX_PUBLIC_CONFIG_BYTES_U64
    {
        Err(XmrReleaseServiceError::InvalidPublicConfiguration)
    } else {
        Ok(())
    }
}

fn same_public_file(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    left.dev() == right.dev() && left.ino() == right.ino()
}

fn stable_public_file(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    same_public_file(left, right)
        && left.len() == right.len()
        && left.mode() == right.mode()
        && left.mtime() == right.mtime()
        && left.mtime_nsec() == right.mtime_nsec()
        && left.ctime() == right.ctime()
        && left.ctime_nsec() == right.ctime_nsec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_is_payload_free_and_exact() {
        let report = XmrReleaseServiceReport::new(
            ReleasePublicationOutcome::Admitted(PublicationAdmissionStatus::Accepted),
            ReleaseState::Admitted,
            ReleaseNodeRouteProfile::Local,
        );
        assert_eq!(
            serde_json::to_value(report).unwrap(),
            serde_json::json!({
                "schema_version": 1,
                "event": "xmr_claim_authorization_publication",
                "outcome": "admitted_accepted",
                "durable_state": "admitted",
                "node_profile": "local"
            })
        );
        assert!(report.is_durably_admitted());
    }

    #[test]
    fn endpoint_profiles_are_closed() {
        assert!(validate_loopback_http_endpoint("http://127.0.0.1:8779/").is_ok());
        assert!(validate_loopback_http_endpoint("http://[::1]:8779/").is_ok());
        assert!(validate_official_public_endpoint(OFFICIAL_PUBLIC_INDEXER_ENDPOINT).is_ok());
        for endpoint in [
            "http://localhost:8779/",
            "https://127.0.0.1:8779/",
            "http://127.0.0.1/",
            "http://127.0.0.1:8779/path",
            "https://example.com/",
            "https://testnet.lez.logos.co/path",
        ] {
            assert!(validate_loopback_http_endpoint(endpoint).is_err());
            if endpoint != OFFICIAL_PUBLIC_INDEXER_ENDPOINT {
                assert!(validate_official_public_endpoint(endpoint).is_err());
            }
        }
    }

    #[test]
    fn private_paths_and_errors_are_redacted() {
        let paths = XmrReleaseServicePaths::new("/secret/bearer", "/secret/key", "/secret/state");
        let rendered = format!("{paths:?}");
        assert!(!rendered.contains("/secret"));
        for error in [
            XmrReleaseServiceError::InvalidPublicConfiguration,
            XmrReleaseServiceError::InvalidPrivateFileLayout,
            XmrReleaseServiceError::ProtectionKeyUnavailable,
            XmrReleaseServiceError::ReleaseClientUnavailable,
            XmrReleaseServiceError::ReleaseJournalUnavailable,
            XmrReleaseServiceError::PublicationFailed,
        ] {
            assert!(!error.to_string().contains("/secret"));
            assert!(!format!("{error:?}").contains("/secret"));
        }
    }
}
