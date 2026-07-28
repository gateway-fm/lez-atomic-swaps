use std::{
    collections::HashSet,
    fmt,
    path::{Path, PathBuf},
    time::Duration,
};

use lez_bridge_client::SidecarCapability;
use lez_bridge_protocol::{DiscoveryWindow, Hex32, Participant, RunId, RuntimeDescriptor};
use lez_swap_core::SwapId;
use lez_zec_swap_sdk::{
    ClaimPreimage, MAX_ZEC_AGREEMENT_RECORD_BYTES, MAX_ZEC_FUNDING_INPUTS, ProtectedClaimKey,
};
use secp256k1::SecretKey;
use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error as _};
use sha2::{Digest as _, Sha256};
use thiserror::Error;
use url::{Host, Url};
use zeroize::Zeroizing;

use crate::secure_file::{
    FileLocation, FilePrivacy, SecureFileError, canonical_location, read_bounded_identified,
    read_bounded_sealed_memfd,
};

const CONFIG_SCHEMA_VERSION: u16 = 3;
const MAX_CONFIG_BYTES: usize = 64 * 1024;
const CLAIM_KEY_BYTES: usize = 32;
const ZCASH_KEY_BYTES: usize = 32;
const PREIMAGE_BYTES: usize = 32;
const MAX_CAPABILITY_FILE_BYTES: usize = 130;
const MAX_COOKIE_FILE_BYTES: usize = 1_026;
const MAX_COOKIE_BYTES: usize = 1_024;
const MAX_API_KEY_FILE_BYTES: usize = 1_026;
const MAX_API_KEY_BYTES: usize = 1_024;
const MAX_REQUEST_TIMEOUT_MILLIS: u64 = 60_000;
const MAX_COUNTERPARTY_SCAN_BLOCKS: u32 = 50_000;
const TATUM_TESTNET_ZEBRA_ENDPOINT: &str = "https://zcash-testnet-zebrad.gateway.tatum.io/";

/// Role permanently bound to one actor configuration.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ActorRole {
    /// Liquidity-providing participant.
    Maker,
    /// Offer-taking participant.
    Taker,
}

impl ActorRole {
    pub(crate) const fn bridge_participant(self) -> Participant {
        match self {
            Self::Maker => Participant::Maker,
            Self::Taker => Participant::Taker,
        }
    }

    pub(crate) const fn sdk_participant(self) -> lez_swap_core::Participant {
        match self {
            Self::Maker => lez_swap_core::Participant::Maker,
            Self::Taker => lez_swap_core::Participant::Taker,
        }
    }
}

/// Zcash consensus network selected for one actor.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ZcashNetworkConfig {
    /// Zcash main network.
    Main,
    /// Public Zcash test network.
    Test,
    /// Deterministic local Zcash network.
    Regtest,
}

/// Zebra JSON-RPC chain spelling reported by the node.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ZebraRpcChainConfig {
    /// Main-chain RPC identity.
    Main,
    /// Test-family RPC identity, including Regtest.
    Test,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
enum ConsensusBranchConfig {
    #[serde(rename = "00000000")]
    Sprout,
    #[serde(rename = "5ba81b19")]
    Overwinter,
    #[serde(rename = "76b809bb")]
    Sapling,
    #[serde(rename = "2bb40e60")]
    Blossom,
    #[serde(rename = "f5b9230b")]
    Heartwood,
    #[serde(rename = "e9ff75a6")]
    Canopy,
    #[serde(rename = "c2d6d0b4")]
    Nu5,
    #[serde(rename = "c8e71055")]
    Nu6,
    #[serde(rename = "4dec4df0")]
    Nu6_1,
    #[serde(rename = "5437f330")]
    Nu6_2,
}

impl ConsensusBranchConfig {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Sprout => "00000000",
            Self::Overwinter => "5ba81b19",
            Self::Sapling => "76b809bb",
            Self::Blossom => "2bb40e60",
            Self::Heartwood => "f5b9230b",
            Self::Canopy => "e9ff75a6",
            Self::Nu5 => "c2d6d0b4",
            Self::Nu6 => "c8e71055",
            Self::Nu6_1 => "4dec4df0",
            Self::Nu6_2 => "5437f330",
        }
    }
}

#[derive(Clone, Eq, PartialEq)]
struct LoopbackHttpEndpoint(Url);

impl LoopbackHttpEndpoint {
    fn new(value: &str) -> Result<Self, &'static str> {
        let url = Url::parse(value).map_err(|_| "invalid actor endpoint")?;
        let loopback = match url.host() {
            Some(Host::Ipv4(address)) => address.is_loopback(),
            Some(Host::Ipv6(address)) => address.is_loopback(),
            Some(Host::Domain(_)) | None => false,
        };
        if url.scheme() != "http"
            || !loopback
            || url.port().is_none_or(|port| port == 0)
            || !url.username().is_empty()
            || url.password().is_some()
            || url.path() != "/"
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err("invalid actor endpoint");
        }
        Ok(Self(url))
    }

    const fn as_url(&self) -> &Url {
        &self.0
    }
}

impl<'de> Deserialize<'de> for LoopbackHttpEndpoint {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(&value).map_err(D::Error::custom)
    }
}

impl Serialize for LoopbackHttpEndpoint {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.0.as_str())
    }
}

#[derive(Clone, Eq, PartialEq)]
struct TatumTestnetHttpsEndpoint(Url);

impl TatumTestnetHttpsEndpoint {
    fn new(value: &str) -> Result<Self, &'static str> {
        let url = Url::parse(value).map_err(|_| "invalid public Zebra endpoint")?;
        if url.as_str() != TATUM_TESTNET_ZEBRA_ENDPOINT
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err("invalid public Zebra endpoint");
        }
        Ok(Self(url))
    }

    const fn as_url(&self) -> &Url {
        &self.0
    }
}

impl<'de> Deserialize<'de> for TatumTestnetHttpsEndpoint {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(&value).map_err(D::Error::custom)
    }
}

impl Serialize for TatumTestnetHttpsEndpoint {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.0.as_str())
    }
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct ClaimRecoveryConfig {
    key_id: Box<str>,
    key_file: PathBuf,
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct BridgeConfig {
    endpoint: LoopbackHttpEndpoint,
    journal_db: PathBuf,
    capability_file: PathBuf,
    runtime: RuntimeDescriptor,
    request_timeout_millis: u64,
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct ZebraIdentityConfig {
    network: ZcashNetworkConfig,
    rpc_chain: ZebraRpcChainConfig,
    consensus_branch_id: ConsensusBranchConfig,
    genesis_hash: Hex32,
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum ZebraRouteConfig {
    DeterministicLocal {
        endpoint: LoopbackHttpEndpoint,
        cookie_file: Option<PathBuf>,
    },
    SelfHostedCookie {
        endpoint: LoopbackHttpEndpoint,
        cookie_file: PathBuf,
    },
    TatumTestnetXApiKey {
        endpoint: TatumTestnetHttpsEndpoint,
        api_key_file: PathBuf,
    },
}

impl ZebraRouteConfig {
    const fn endpoint(&self) -> &Url {
        match self {
            Self::DeterministicLocal { endpoint, .. } | Self::SelfHostedCookie { endpoint, .. } => {
                endpoint.as_url()
            }
            Self::TatumTestnetXApiKey { endpoint, .. } => endpoint.as_url(),
        }
    }

    fn cookie_file(&self) -> Option<&PathBuf> {
        match self {
            Self::DeterministicLocal { cookie_file, .. } => cookie_file.as_ref(),
            Self::SelfHostedCookie { cookie_file, .. } => Some(cookie_file),
            Self::TatumTestnetXApiKey { .. } => None,
        }
    }

    const fn api_key_file(&self) -> Option<&PathBuf> {
        match self {
            Self::TatumTestnetXApiKey { api_key_file, .. } => Some(api_key_file),
            Self::DeterministicLocal { .. } | Self::SelfHostedCookie { .. } => None,
        }
    }

    const fn matches_network(&self, network: ZcashNetworkConfig) -> bool {
        matches!(
            (self, network),
            (Self::DeterministicLocal { .. }, ZcashNetworkConfig::Regtest)
                | (
                    Self::SelfHostedCookie { .. },
                    ZcashNetworkConfig::Main | ZcashNetworkConfig::Test
                )
                | (Self::TatumTestnetXApiKey { .. }, ZcashNetworkConfig::Test)
        )
    }

    fn same_route(&self, other: &Self) -> bool {
        matches!(
            (self, other),
            (
                Self::DeterministicLocal { endpoint: left, .. },
                Self::DeterministicLocal { endpoint: right, .. }
            ) if left == right
        ) || matches!(
            (self, other),
            (
                Self::SelfHostedCookie { endpoint: left, .. },
                Self::SelfHostedCookie { endpoint: right, .. }
            ) if left == right
        ) || matches!(
            (self, other),
            (
                Self::TatumTestnetXApiKey { endpoint: left, .. },
                Self::TatumTestnetXApiKey { endpoint: right, .. }
            ) if left == right
        )
    }
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct ZebraConfig {
    route: ZebraRouteConfig,
    identity: ZebraIdentityConfig,
    counterparty_scan_blocks: u32,
}

/// Agreement-committed transparent output candidate inspected before funding.
#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CandidateOutpoint {
    transaction_id: Hex32,
    output_index: u32,
}

impl CandidateOutpoint {
    /// Creates one exact RPC-display-order transparent output candidate.
    #[must_use]
    pub const fn new(transaction_id: Hex32, output_index: u32) -> Self {
        Self {
            transaction_id,
            output_index,
        }
    }

    /// Canonical RPC-display-order transaction identifier.
    pub const fn transaction_id(&self) -> Hex32 {
        self.transaction_id
    }

    /// Transparent output index within the transaction.
    #[must_use]
    pub const fn output_index(&self) -> u32 {
        self.output_index
    }
}

#[derive(Clone, Default, Eq, PartialEq)]
struct PathBindings {
    agreement: FileLocation,
    role_state: FileLocation,
    claim_key: FileLocation,
    preimage: Option<FileLocation>,
    zcash_key: FileLocation,
    bridge_journal: FileLocation,
    capability: FileLocation,
    cookie: Option<FileLocation>,
    api_key: Option<FileLocation>,
}

impl PathBindings {
    fn local(&self) -> Vec<&FileLocation> {
        let mut local = vec![
            &self.role_state,
            &self.claim_key,
            &self.zcash_key,
            &self.bridge_journal,
            &self.capability,
        ];
        local.extend(self.preimage.iter());
        local.extend(self.cookie.iter());
        local.extend(self.api_key.iter());
        local
    }

    fn all(&self) -> Vec<&FileLocation> {
        let mut all = vec![&self.agreement];
        all.extend(self.local());
        all
    }
}

/// Complete validated runtime configuration for one role-fixed actor.
///
/// This type intentionally cannot be deserialized directly. Construction must
/// pass the private-file, semantic, and path-graph checks in
/// [`ActorConfig::load_private`].
#[derive(Clone, Eq, PartialEq)]
pub struct ActorConfig {
    source_identity: FileLocation,
    bindings: PathBindings,
    schema_version: u16,
    role: ActorRole,
    run_id: RunId,
    swap_id: SwapId,
    signed_agreement_file: PathBuf,
    signed_agreement_sha256: Hex32,
    role_state_db: PathBuf,
    claim_recovery: ClaimRecoveryConfig,
    claim_preimage_file: Option<PathBuf>,
    zcash_key_file: PathBuf,
    bridge: BridgeConfig,
    zebra: ZebraConfig,
    lez_discovery_window: DiscoveryWindow,
    zcash_funding_outpoints: Vec<CandidateOutpoint>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RawActorConfig {
    schema_version: u16,
    role: ActorRole,
    run_id: RunId,
    swap_id: SwapId,
    signed_agreement_file: PathBuf,
    signed_agreement_sha256: Hex32,
    role_state_db: PathBuf,
    claim_recovery: ClaimRecoveryConfig,
    claim_preimage_file: Option<PathBuf>,
    zcash_key_file: PathBuf,
    bridge: BridgeConfig,
    zebra: ZebraConfig,
    lez_discovery_window: DiscoveryWindow,
    zcash_funding_outpoints: Vec<CandidateOutpoint>,
}

/// Compares exact verified config bytes with a ZEC Maker scheduler manifest.
///
/// Full path, credential, agreement, and runtime validation still occurs when
/// the actor consumes these same bytes from sealed FD 196. This pre-spawn check
/// prevents a role, application-swap, or role-state database substitution.
///
/// # Errors
///
/// Rejects oversized, malformed, unsupported, non-Maker, wrong-swap, or
/// wrong-state config bytes.
pub fn validate_maker_manifest_config_bytes(
    bytes: &[u8],
    expected_swap_id: &SwapId,
    expected_state_database: &Path,
) -> Result<(), ActorConfigError> {
    if bytes.is_empty() || bytes.len() > MAX_CONFIG_BYTES {
        return Err(ActorConfigError::InvalidConfiguration);
    }
    let raw: RawActorConfig =
        serde_json::from_slice(bytes).map_err(|_| ActorConfigError::InvalidConfiguration)?;
    if raw.schema_version != CONFIG_SCHEMA_VERSION
        || raw.role != ActorRole::Maker
        || &raw.swap_id != expected_swap_id
        || raw.role_state_db != expected_state_database
    {
        return Err(ActorConfigError::InvalidConfiguration);
    }
    Ok(())
}

/// Complete primitive inputs for one deterministic-local-v0.2 actor config.
///
/// This constructor exists so a local fixture provisioner can emit the exact
/// private schema without duplicating the load-only wire representation. The
/// resulting file must still pass [`ActorConfig::load_private`].
#[derive(Clone, Debug)]
pub(crate) struct DeterministicLocalV0_2ActorConfigInput {
    /// Fixed role represented by this config.
    pub(crate) role: ActorRole,
    /// Shared isolated run identity.
    pub(crate) run_id: RunId,
    /// Shared application swap identity.
    pub(crate) swap_id: SwapId,
    /// Shared canonical countersigned agreement file.
    pub(crate) signed_agreement_file: PathBuf,
    /// SHA-256 of the exact countersigned agreement bytes.
    pub(crate) signed_agreement_sha256: Hex32,
    /// Role-local lifecycle database path.
    pub(crate) role_state_db: PathBuf,
    /// Role-local recovery-key identifier.
    pub(crate) claim_recovery_key_id: Box<str>,
    /// Role-local recovery-key file.
    pub(crate) claim_recovery_key_file: PathBuf,
    /// Role-local preimage file, present only for the Zcash funder.
    pub(crate) claim_preimage_file: Option<PathBuf>,
    /// Role-local transparent Zcash secret-key file.
    pub(crate) zcash_key_file: PathBuf,
    /// Dedicated role-local sidecar endpoint.
    pub(crate) bridge_endpoint: Url,
    /// Role-local sidecar operation journal.
    pub(crate) bridge_journal_db: PathBuf,
    /// Role-local sidecar bearer-capability file.
    pub(crate) bridge_capability_file: PathBuf,
    /// Exact role-specific v0.2 runtime descriptor.
    pub(crate) bridge_runtime: RuntimeDescriptor,
    /// Shared isolated Zebra Regtest endpoint.
    pub(crate) zebra_endpoint: Url,
    /// Exact pinned Regtest genesis hash.
    pub(crate) zebra_genesis_hash: Hex32,
    /// Finite counterparty observation horizon.
    pub(crate) counterparty_scan_blocks: u32,
    /// Bounded LEZ canonical discovery window.
    pub(crate) lez_discovery_window: DiscoveryWindow,
    /// Exact candidates disclosed only to the local Zcash funder.
    pub(crate) zcash_funding_outpoints: Vec<CandidateOutpoint>,
}

/// Encodes the exact load-only actor schema for a deterministic local v0.2 run.
///
/// # Errors
///
/// Rejects a non-loopback endpoint or an unexpected in-memory serialization
/// failure. The written file must still be reloaded with
/// [`ActorConfig::load_private`] before use.
pub(crate) fn encode_deterministic_local_v0_2_actor_config(
    input: DeterministicLocalV0_2ActorConfigInput,
) -> Result<Vec<u8>, ActorConfigError> {
    let bridge_endpoint = LoopbackHttpEndpoint::new(input.bridge_endpoint.as_str())
        .map_err(|_| ActorConfigError::InvalidConfiguration)?;
    let zebra_endpoint = LoopbackHttpEndpoint::new(input.zebra_endpoint.as_str())
        .map_err(|_| ActorConfigError::InvalidConfiguration)?;
    let raw = RawActorConfig {
        schema_version: CONFIG_SCHEMA_VERSION,
        role: input.role,
        run_id: input.run_id,
        swap_id: input.swap_id,
        signed_agreement_file: input.signed_agreement_file,
        signed_agreement_sha256: input.signed_agreement_sha256,
        role_state_db: input.role_state_db,
        claim_recovery: ClaimRecoveryConfig {
            key_id: input.claim_recovery_key_id,
            key_file: input.claim_recovery_key_file,
        },
        claim_preimage_file: input.claim_preimage_file,
        zcash_key_file: input.zcash_key_file,
        bridge: BridgeConfig {
            endpoint: bridge_endpoint,
            journal_db: input.bridge_journal_db,
            capability_file: input.bridge_capability_file,
            runtime: input.bridge_runtime,
            request_timeout_millis: 10_000,
        },
        zebra: ZebraConfig {
            route: ZebraRouteConfig::DeterministicLocal {
                endpoint: zebra_endpoint,
                cookie_file: None,
            },
            identity: ZebraIdentityConfig {
                network: ZcashNetworkConfig::Regtest,
                rpc_chain: ZebraRpcChainConfig::Test,
                consensus_branch_id: ConsensusBranchConfig::Nu6_2,
                genesis_hash: input.zebra_genesis_hash,
            },
            counterparty_scan_blocks: input.counterparty_scan_blocks,
        },
        lez_discovery_window: input.lez_discovery_window,
        zcash_funding_outpoints: input.zcash_funding_outpoints,
    };
    serde_json::to_vec_pretty(&raw).map_err(|_| ActorConfigError::InvalidConfiguration)
}

/// Re-encodes one validated local actor around an exact new agreement and
/// fresh mutable state locations without loading or returning any secret.
pub(crate) fn encode_rebound_local_v0_2_actor_config(
    source: &ActorConfig,
    swap_id: SwapId,
    agreement_file: PathBuf,
    agreement_sha256: Hex32,
    role_state_db: PathBuf,
    bridge_journal_db: PathBuf,
) -> Result<Vec<u8>, ActorConfigError> {
    if !matches!(
        source.zebra.route,
        ZebraRouteConfig::DeterministicLocal { .. }
    ) {
        return Err(ActorConfigError::InvalidConfiguration);
    }
    let mut bridge = source.bridge.clone();
    bridge.journal_db = bridge_journal_db;
    let raw = RawActorConfig {
        schema_version: source.schema_version,
        role: source.role,
        run_id: source.run_id.clone(),
        swap_id,
        signed_agreement_file: agreement_file,
        signed_agreement_sha256: agreement_sha256,
        role_state_db,
        claim_recovery: source.claim_recovery.clone(),
        claim_preimage_file: source.claim_preimage_file.clone(),
        zcash_key_file: source.zcash_key_file.clone(),
        bridge,
        zebra: source.zebra.clone(),
        lez_discovery_window: source.lez_discovery_window,
        zcash_funding_outpoints: source.zcash_funding_outpoints.clone(),
    };
    serde_json::to_vec_pretty(&raw).map_err(|_| ActorConfigError::InvalidConfiguration)
}

impl ActorConfig {
    /// Loads one owner-private config without loading effect credentials.
    ///
    /// # Errors
    ///
    /// Rejects an unavailable, non-regular, symlinked, overlarge, non-0600,
    /// malformed, ambiguous, or path-sharing configuration.
    pub fn load_private(path: impl AsRef<Path>) -> Result<Self, ActorConfigError> {
        let path = path.as_ref();
        let (bytes, source_identity) =
            read_bounded_identified(path, MAX_CONFIG_BYTES, FilePrivacy::OwnerPrivate)
                .map_err(map_config_file_error)?;
        Self::from_identified_bytes(&bytes, source_identity)
    }

    /// Loads one anonymous, immutable config from the fixed inherited descriptor.
    ///
    /// # Errors
    ///
    /// Rejects a missing/wrong descriptor, linked or mutable file, incomplete seals,
    /// unsafe owner/mode/size, malformed JSON, or invalid/aliased path bindings.
    pub fn load_private_fd(fd: i32) -> Result<Self, ActorConfigError> {
        let (bytes, source_identity) =
            read_bounded_sealed_memfd(fd, MAX_CONFIG_BYTES).map_err(map_config_file_error)?;
        Self::from_identified_bytes(&bytes, source_identity)
    }

    fn from_identified_bytes(
        bytes: &Zeroizing<Vec<u8>>,
        source_identity: FileLocation,
    ) -> Result<Self, ActorConfigError> {
        let raw: RawActorConfig = serde_json::from_slice(bytes.as_slice())
            .map_err(|_| ActorConfigError::InvalidConfiguration)?;
        let mut config = Self {
            source_identity,
            bindings: PathBindings::default(),
            schema_version: raw.schema_version,
            role: raw.role,
            run_id: raw.run_id,
            swap_id: raw.swap_id,
            signed_agreement_file: raw.signed_agreement_file,
            signed_agreement_sha256: raw.signed_agreement_sha256,
            role_state_db: raw.role_state_db,
            claim_recovery: raw.claim_recovery,
            claim_preimage_file: raw.claim_preimage_file,
            zcash_key_file: raw.zcash_key_file,
            bridge: raw.bridge,
            zebra: raw.zebra,
            lez_discovery_window: raw.lez_discovery_window,
            zcash_funding_outpoints: raw.zcash_funding_outpoints,
        };
        config.validate()?;
        Ok(config)
    }

    /// Loads only the recovery key required by offline durable status.
    ///
    /// # Errors
    ///
    /// Rejects unavailable, replaced, unsafe, or invalid recovery-key material.
    pub fn load_status_material(&self) -> Result<StatusMaterial, ActorConfigError> {
        Ok(StatusMaterial {
            claim_recovery_key: self.load_claim_recovery_key()?,
        })
    }

    /// Loads fresh material required to activate one signed agreement.
    ///
    /// # Errors
    ///
    /// Rejects unavailable, replaced, unsafe, or invalid activation material.
    pub fn load_activate_material(&self) -> Result<ActivateMaterial, ActorConfigError> {
        let signed_agreement_wire = self.read_command_file(
            &self.signed_agreement_file,
            MAX_ZEC_AGREEMENT_RECORD_BYTES,
            FilePrivacy::Public,
            &self.bindings.agreement,
        )?;
        let agreement_sha256: [u8; 32] = Sha256::digest(signed_agreement_wire.as_slice()).into();
        if &agreement_sha256 != self.signed_agreement_sha256.as_bytes() {
            return Err(ActorConfigError::InvalidCommandMaterial);
        }
        Ok(ActivateMaterial {
            signed_agreement_wire,
            claim_recovery_key: self.load_claim_recovery_key()?,
            zcash_secret_key: self.load_zcash_key()?,
            claim_preimage: self.load_claim_preimage()?,
            sidecar_capability: self.load_sidecar_capability()?,
        })
    }

    /// Loads fresh credentials required by one effect-driving invocation.
    ///
    /// # Errors
    ///
    /// Rejects unavailable, replaced, unsafe, or invalid effect material.
    pub fn load_drive_material(&self) -> Result<DriveMaterial, ActorConfigError> {
        Ok(DriveMaterial {
            claim_recovery_key: self.load_claim_recovery_key()?,
            zcash_secret_key: self.load_zcash_key()?,
            claim_preimage: self.load_claim_preimage()?,
            sidecar_capability: self.load_sidecar_capability()?,
            zebra_cookie: self.load_zebra_cookie()?,
            zebra_api_key: self.load_zebra_api_key()?,
        })
    }

    /// Fixed local role.
    #[must_use]
    pub const fn role(&self) -> ActorRole {
        self.role
    }

    /// Bounded run identity shared by the two actors.
    pub const fn run_id(&self) -> &RunId {
        &self.run_id
    }

    /// Exact application swap identity.
    #[must_use]
    pub const fn swap_id(&self) -> &SwapId {
        &self.swap_id
    }

    /// Role-local durable lifecycle database path.
    #[must_use]
    pub fn role_state_db(&self) -> &Path {
        &self.role_state_db
    }

    /// Sidecar HTTP endpoint.
    #[must_use]
    pub const fn bridge_endpoint(&self) -> &Url {
        self.bridge.endpoint.as_url()
    }

    /// Role-local sidecar operation-journal path.
    #[must_use]
    pub fn bridge_journal_db(&self) -> &Path {
        &self.bridge.journal_db
    }

    /// Role-local sidecar bearer-capability file.
    pub(crate) fn bridge_capability_file(&self) -> &Path {
        &self.bridge.capability_file
    }

    /// Full immutable sidecar runtime identity.
    pub const fn bridge_runtime(&self) -> &RuntimeDescriptor {
        &self.bridge.runtime
    }

    /// Maximum duration of one sidecar request.
    #[must_use]
    pub const fn bridge_request_timeout(&self) -> Duration {
        Duration::from_millis(self.bridge.request_timeout_millis)
    }

    /// Zebra HTTP endpoint.
    #[must_use]
    pub const fn zebra_endpoint(&self) -> &Url {
        self.zebra.route.endpoint()
    }

    /// Immutable Zcash consensus network.
    #[must_use]
    pub const fn zcash_network(&self) -> ZcashNetworkConfig {
        self.zebra.identity.network
    }

    /// Immutable Zebra RPC chain identity.
    #[must_use]
    pub const fn zebra_rpc_chain(&self) -> ZebraRpcChainConfig {
        self.zebra.identity.rpc_chain
    }

    /// Exact known consensus branch identifier.
    #[must_use]
    pub const fn zcash_consensus_branch_id(&self) -> &'static str {
        self.zebra.identity.consensus_branch_id.as_str()
    }

    /// RPC-display-order genesis block hash.
    pub const fn zcash_genesis_hash(&self) -> Hex32 {
        self.zebra.identity.genesis_hash
    }

    /// Finite maximum counterparty scan horizon.
    #[must_use]
    pub const fn counterparty_scan_blocks(&self) -> u32 {
        self.zebra.counterparty_scan_blocks
    }

    /// Fixed bounded LEZ discovery window.
    pub const fn lez_discovery_window(&self) -> DiscoveryWindow {
        self.lez_discovery_window
    }

    /// Agreement-committed candidate Zcash funding outpoints.
    #[must_use]
    pub fn zcash_funding_outpoints(&self) -> &[CandidateOutpoint] {
        &self.zcash_funding_outpoints
    }

    /// Whether this actor owns the private claim preimage and Zcash candidates.
    #[must_use]
    pub const fn is_local_zcash_funder(&self) -> bool {
        self.claim_preimage_file.is_some()
    }

    fn validate(&mut self) -> Result<(), ActorConfigError> {
        if self.schema_version != CONFIG_SCHEMA_VERSION
            || SwapId::new(self.swap_id.as_str()).is_err()
            || is_zero(self.signed_agreement_sha256)
            || self.bridge.runtime.sidecar_role != self.role.bridge_participant()
            || self.bridge.endpoint.as_url() == self.zebra.route.endpoint()
            || self.bridge.request_timeout_millis == 0
            || self.bridge.request_timeout_millis > MAX_REQUEST_TIMEOUT_MILLIS
            || !runtime_identity_is_nonzero(&self.bridge.runtime)
            || !zebra_chain_matches_network(&self.zebra.identity)
            || !self
                .zebra
                .route
                .matches_network(self.zebra.identity.network)
            || is_zero(self.zebra.identity.genesis_hash)
            || self.zebra.counterparty_scan_blocks == 0
            || self.zebra.counterparty_scan_blocks > MAX_COUNTERPARTY_SCAN_BLOCKS
            || !valid_key_id(&self.claim_recovery.key_id)
        {
            return Err(ActorConfigError::InvalidConfiguration);
        }

        let owns_preimage = self.claim_preimage_file.is_some();
        if owns_preimage
            != (1..=MAX_ZEC_FUNDING_INPUTS).contains(&self.zcash_funding_outpoints.len())
            || self
                .zcash_funding_outpoints
                .iter()
                .any(|candidate| is_zero(candidate.transaction_id))
            || self
                .zcash_funding_outpoints
                .iter()
                .collect::<HashSet<_>>()
                .len()
                != self.zcash_funding_outpoints.len()
        {
            return Err(ActorConfigError::InvalidConfiguration);
        }

        self.bindings = self.path_bindings()?;
        let local = self.bindings.local();
        if contains_path_alias(&local)
            || contains_location(&local, &self.source_identity)
            || contains_location(&local, &self.bindings.agreement)
            || self.source_identity.aliases(&self.bindings.agreement)
        {
            return Err(ActorConfigError::InvalidConfiguration);
        }
        Ok(())
    }

    fn path_bindings(&self) -> Result<PathBindings, ActorConfigError> {
        Ok(PathBindings {
            agreement: canonical_config_path(&self.signed_agreement_file)?,
            role_state: canonical_config_path(&self.role_state_db)?,
            claim_key: canonical_config_path(&self.claim_recovery.key_file)?,
            preimage: self
                .claim_preimage_file
                .as_ref()
                .map(|path| canonical_config_path(path))
                .transpose()?,
            zcash_key: canonical_config_path(&self.zcash_key_file)?,
            bridge_journal: canonical_config_path(&self.bridge.journal_db)?,
            capability: canonical_config_path(&self.bridge.capability_file)?,
            cookie: self
                .zebra
                .route
                .cookie_file()
                .map(|path| canonical_config_path(path))
                .transpose()?,
            api_key: self
                .zebra
                .route
                .api_key_file()
                .map(|path| canonical_config_path(path))
                .transpose()?,
        })
    }

    fn load_claim_recovery_key(&self) -> Result<ProtectedClaimKey, ActorConfigError> {
        let material = self.read_exact_private::<CLAIM_KEY_BYTES>(
            &self.claim_recovery.key_file,
            &self.bindings.claim_key,
        )?;
        if material.iter().all(|byte| *byte == 0) {
            return Err(ActorConfigError::InvalidCommandMaterial);
        }
        ProtectedClaimKey::new(self.claim_recovery.key_id.clone(), *material)
            .map_err(|_| ActorConfigError::InvalidCommandMaterial)
    }

    fn load_zcash_key(&self) -> Result<Zeroizing<[u8; ZCASH_KEY_BYTES]>, ActorConfigError> {
        let material = self.read_exact_private::<ZCASH_KEY_BYTES>(
            &self.zcash_key_file,
            &self.bindings.zcash_key,
        )?;
        let mut parsed = SecretKey::from_slice(material.as_ref())
            .map_err(|_| ActorConfigError::InvalidCommandMaterial)?;
        parsed.non_secure_erase();
        Ok(material)
    }

    fn load_claim_preimage(&self) -> Result<Option<ClaimPreimage>, ActorConfigError> {
        self.claim_preimage_file
            .as_ref()
            .zip(self.bindings.preimage.as_ref())
            .map(|(path, expected)| {
                self.read_exact_private::<PREIMAGE_BYTES>(path, expected)
                    .map(|value| ClaimPreimage::new(*value))
            })
            .transpose()
    }

    fn load_sidecar_capability(&self) -> Result<SidecarCapability, ActorConfigError> {
        let mut bytes = self.read_command_file(
            &self.bridge.capability_file,
            MAX_CAPABILITY_FILE_BYTES,
            FilePrivacy::OwnerPrivate,
            &self.bindings.capability,
        )?;
        trim_line_ending(bytes.as_mut());
        if !bytes.is_ascii() {
            return Err(ActorConfigError::InvalidCommandMaterial);
        }
        let value = String::from_utf8(bytes.to_vec())
            .map_err(|_| ActorConfigError::InvalidCommandMaterial)?;
        SidecarCapability::new(value).map_err(|_| ActorConfigError::InvalidCommandMaterial)
    }

    fn load_zebra_cookie(&self) -> Result<Option<ZebraCookie>, ActorConfigError> {
        self.zebra
            .route
            .cookie_file()
            .zip(self.bindings.cookie.as_ref())
            .map(|(path, expected)| {
                let mut bytes = self.read_command_file(
                    path,
                    MAX_COOKIE_FILE_BYTES,
                    FilePrivacy::OwnerPrivate,
                    expected,
                )?;
                trim_line_ending(bytes.as_mut());
                let separator = bytes.iter().position(|byte| *byte == b':');
                if bytes.len() > MAX_COOKIE_BYTES
                    || bytes.iter().any(|byte| !byte.is_ascii_graphic())
                    || separator.is_none_or(|index| index == 0 || index + 1 == bytes.len())
                {
                    return Err(ActorConfigError::InvalidCommandMaterial);
                }
                Ok(ZebraCookie(bytes))
            })
            .transpose()
    }

    fn load_zebra_api_key(&self) -> Result<Option<ZebraApiKey>, ActorConfigError> {
        self.zebra
            .route
            .api_key_file()
            .zip(self.bindings.api_key.as_ref())
            .map(|(path, expected)| {
                let mut bytes = self.read_command_file(
                    path,
                    MAX_API_KEY_FILE_BYTES,
                    FilePrivacy::OwnerPrivate,
                    expected,
                )?;
                trim_line_ending(bytes.as_mut());
                if bytes.is_empty()
                    || bytes.len() > MAX_API_KEY_BYTES
                    || bytes.iter().any(|byte| !byte.is_ascii_graphic())
                {
                    return Err(ActorConfigError::InvalidCommandMaterial);
                }
                Ok(ZebraApiKey(bytes))
            })
            .transpose()
    }

    fn read_exact_private<const N: usize>(
        &self,
        path: &Path,
        expected: &FileLocation,
    ) -> Result<Zeroizing<[u8; N]>, ActorConfigError> {
        let bytes = self.read_command_file(path, N, FilePrivacy::OwnerPrivate, expected)?;
        if bytes.len() != N {
            return Err(ActorConfigError::InvalidCommandMaterial);
        }
        let mut value = Zeroizing::new([0_u8; N]);
        value.copy_from_slice(bytes.as_slice());
        Ok(value)
    }

    fn read_command_file(
        &self,
        path: &Path,
        maximum: usize,
        privacy: FilePrivacy,
        expected: &FileLocation,
    ) -> Result<Zeroizing<Vec<u8>>, ActorConfigError> {
        let (bytes, current) =
            read_bounded_identified(path, maximum, privacy).map_err(|error| match error {
                SecureFileError::Unavailable => ActorConfigError::CommandMaterialUnavailable,
                SecureFileError::Unsafe => ActorConfigError::UnsafeCommandMaterialFile,
            })?;
        let aliases_other_binding = self
            .bindings
            .all()
            .into_iter()
            .chain(std::iter::once(&self.source_identity))
            .filter(|binding| !std::ptr::eq(*binding, expected))
            .any(|binding| current.aliases(binding));
        let current_role_state =
            canonical_location(&self.role_state_db).map_err(map_command_location_error)?;
        let current_bridge_journal =
            canonical_location(&self.bridge.journal_db).map_err(map_command_location_error)?;
        let mutable_locations = [
            (&current_role_state, &self.bindings.role_state),
            (&current_bridge_journal, &self.bindings.bridge_journal),
        ];
        let unsafe_mutable_location =
            mutable_locations.iter().any(|(mutable, expected_mutable)| {
                !expected_mutable.unchanged(mutable)
                    || current.aliases(mutable)
                    || self
                        .bindings
                        .all()
                        .into_iter()
                        .chain(std::iter::once(&self.source_identity))
                        .filter(|binding| !std::ptr::eq(*binding, *expected_mutable))
                        .any(|binding| mutable.aliases(binding))
            }) || current_role_state.aliases(&current_bridge_journal);
        if !expected.unchanged(&current) || aliases_other_binding || unsafe_mutable_location {
            return Err(ActorConfigError::UnsafeCommandMaterialFile);
        }
        Ok(bytes)
    }
}

impl fmt::Debug for ActorConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ActorConfig")
            .field("schema_version", &self.schema_version)
            .field("role", &self.role)
            .field("run_id", &self.run_id)
            .field("swap_id", &"[REDACTED]")
            .field("paths", &"[REDACTED]")
            .field("credentials", &"[REDACTED]")
            .finish_non_exhaustive()
    }
}

/// Fresh recovery material required by offline status.
pub struct StatusMaterial {
    claim_recovery_key: ProtectedClaimKey,
}

impl StatusMaterial {
    /// Recovery key used to reopen protected durable claim submissions.
    #[must_use]
    pub const fn claim_recovery_key(&self) -> &ProtectedClaimKey {
        &self.claim_recovery_key
    }

    pub(crate) fn into_claim_recovery_key(self) -> ProtectedClaimKey {
        self.claim_recovery_key
    }
}

impl fmt::Debug for StatusMaterial {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("StatusMaterial")
            .field("claim_recovery_key", &"[REDACTED]")
            .finish()
    }
}

/// Fresh agreement and credentials required by activation.
pub struct ActivateMaterial {
    signed_agreement_wire: Zeroizing<Vec<u8>>,
    claim_recovery_key: ProtectedClaimKey,
    zcash_secret_key: Zeroizing<[u8; ZCASH_KEY_BYTES]>,
    claim_preimage: Option<ClaimPreimage>,
    sidecar_capability: SidecarCapability,
}

impl ActivateMaterial {
    /// Bounded signed agreement wire.
    #[must_use]
    pub fn signed_agreement_wire(&self) -> &[u8] {
        self.signed_agreement_wire.as_slice()
    }

    /// Recovery key used to protect durable claim material.
    #[must_use]
    pub const fn claim_recovery_key(&self) -> &ProtectedClaimKey {
        &self.claim_recovery_key
    }

    /// Validated role-local secp256k1 secret bytes.
    #[must_use]
    pub fn zcash_secret_key(&self) -> &[u8; ZCASH_KEY_BYTES] {
        &self.zcash_secret_key
    }

    /// Locally owned claim preimage, present only for the configured funder.
    #[must_use]
    pub const fn claim_preimage(&self) -> Option<&ClaimPreimage> {
        self.claim_preimage.as_ref()
    }

    /// Fresh sidecar bearer capability.
    #[must_use]
    pub const fn sidecar_capability(&self) -> &SidecarCapability {
        &self.sidecar_capability
    }

    pub(crate) fn into_claim_recovery_key(self) -> ProtectedClaimKey {
        self.claim_recovery_key
    }
}

impl fmt::Debug for ActivateMaterial {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ActivateMaterial")
            .field("signed_agreement_wire", &"[REDACTED]")
            .field("claim_recovery_key", &"[REDACTED]")
            .field("zcash_secret_key", &"[REDACTED]")
            .field("claim_preimage", &"[REDACTED]")
            .field("sidecar_capability", &"[REDACTED]")
            .finish()
    }
}

/// Fresh credentials required by one lifecycle-driving invocation.
pub struct DriveMaterial {
    claim_recovery_key: ProtectedClaimKey,
    zcash_secret_key: Zeroizing<[u8; ZCASH_KEY_BYTES]>,
    claim_preimage: Option<ClaimPreimage>,
    sidecar_capability: SidecarCapability,
    zebra_cookie: Option<ZebraCookie>,
    zebra_api_key: Option<ZebraApiKey>,
}

impl DriveMaterial {
    /// Recovery key used to reopen protected durable effects.
    #[must_use]
    pub const fn claim_recovery_key(&self) -> &ProtectedClaimKey {
        &self.claim_recovery_key
    }

    /// Validated role-local secp256k1 secret bytes.
    #[must_use]
    pub fn zcash_secret_key(&self) -> &[u8; ZCASH_KEY_BYTES] {
        &self.zcash_secret_key
    }

    /// Locally owned claim preimage, present only for the configured funder.
    #[must_use]
    pub const fn claim_preimage(&self) -> Option<&ClaimPreimage> {
        self.claim_preimage.as_ref()
    }

    /// Fresh sidecar bearer capability.
    #[must_use]
    pub const fn sidecar_capability(&self) -> &SidecarCapability {
        &self.sidecar_capability
    }

    /// Optional Zebra cookie credential without the trailing line ending.
    #[must_use]
    pub fn zebra_cookie(&self) -> Option<&[u8]> {
        self.zebra_cookie.as_ref().map(|cookie| cookie.0.as_slice())
    }

    /// Optional Tatum `x-api-key` credential without the trailing line ending.
    #[must_use]
    pub fn zebra_api_key(&self) -> Option<&[u8]> {
        self.zebra_api_key
            .as_ref()
            .map(|api_key| api_key.0.as_slice())
    }

    pub(crate) fn into_claim_recovery_key(self) -> ProtectedClaimKey {
        self.claim_recovery_key
    }
}

impl fmt::Debug for DriveMaterial {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DriveMaterial")
            .field("claim_recovery_key", &"[REDACTED]")
            .field("zcash_secret_key", &"[REDACTED]")
            .field("claim_preimage", &"[REDACTED]")
            .field("sidecar_capability", &"[REDACTED]")
            .field("zebra_cookie", &"[REDACTED]")
            .field("zebra_api_key", &"[REDACTED]")
            .finish()
    }
}

struct ZebraCookie(Zeroizing<Vec<u8>>);

struct ZebraApiKey(Zeroizing<Vec<u8>>);

/// A secret-safe configuration or actor-isolation failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Error)]
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
    /// A command-scoped credential or agreement was unavailable.
    #[error("actor command material is unavailable")]
    CommandMaterialUnavailable,
    /// A command-scoped credential or agreement file was unsafe.
    #[error("actor command material file is unsafe")]
    UnsafeCommandMaterialFile,
    /// Command-scoped material had an invalid shape or value.
    #[error("actor command material is invalid")]
    InvalidCommandMaterial,
}

/// Confirms that maker and taker represent distinct users in one isolated run.
///
/// # Errors
///
/// Rejects role, run, swap, chain, agreement, funder, endpoint, signer, config,
/// or private/mutable path sharing inconsistencies.
pub fn validate_actor_pair(
    left: &ActorConfig,
    right: &ActorConfig,
) -> Result<(), ActorConfigError> {
    if left.role == right.role
        || left.run_id != right.run_id
        || left.swap_id != right.swap_id
        || left.signed_agreement_sha256 != right.signed_agreement_sha256
        || !left.bindings.agreement.aliases(&right.bindings.agreement)
        || left.bridge.endpoint == right.bridge.endpoint
        || left.bridge.runtime.sidecar_role == right.bridge.runtime.sidecar_role
        || left.bridge.runtime.signer_account_id == right.bridge.runtime.signer_account_id
        || !same_runtime(&left.bridge.runtime, &right.bridge.runtime)
        || !left.zebra.route.same_route(&right.zebra.route)
        || left.zebra.identity != right.zebra.identity
        || left.lez_discovery_window != right.lez_discovery_window
        || left.is_local_zcash_funder() == right.is_local_zcash_funder()
        || left.source_identity.aliases(&right.source_identity)
        || left
            .bindings
            .local()
            .iter()
            .any(|path| contains_location(&right.bindings.local(), path))
        || contains_location(&left.bindings.local(), &right.source_identity)
        || contains_location(&right.bindings.local(), &left.source_identity)
        || contains_location(&left.bindings.local(), &right.bindings.agreement)
        || contains_location(&right.bindings.local(), &left.bindings.agreement)
    {
        return Err(ActorConfigError::InvalidActorPair);
    }
    Ok(())
}

fn same_runtime(left: &RuntimeDescriptor, right: &RuntimeDescriptor) -> bool {
    left.compatibility == right.compatibility
        && left.chain_id == right.chain_id
        && left.channel_id == right.channel_id
        && left.genesis_block_hash == right.genesis_block_hash
        && left.escrow_program_id == right.escrow_program_id
}

fn runtime_identity_is_nonzero(runtime: &RuntimeDescriptor) -> bool {
    [
        runtime.chain_id,
        runtime.channel_id,
        runtime.genesis_block_hash,
        runtime.escrow_program_id,
        runtime.signer_account_id,
    ]
    .into_iter()
    .all(|identity| !is_zero(identity))
}

fn zebra_chain_matches_network(identity: &ZebraIdentityConfig) -> bool {
    matches!(
        (identity.network, identity.rpc_chain),
        (ZcashNetworkConfig::Main, ZebraRpcChainConfig::Main)
            | (
                ZcashNetworkConfig::Test | ZcashNetworkConfig::Regtest,
                ZebraRpcChainConfig::Test
            )
    )
}

fn is_zero(value: Hex32) -> bool {
    value.as_bytes().iter().all(|byte| *byte == 0)
}

fn valid_key_id(value: &str) -> bool {
    (1..=128).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn canonical_config_path(path: &Path) -> Result<FileLocation, ActorConfigError> {
    canonical_location(path).map_err(|_| ActorConfigError::InvalidConfiguration)
}

fn contains_path_alias(locations: &[&FileLocation]) -> bool {
    locations.iter().enumerate().any(|(index, location)| {
        locations[index + 1..]
            .iter()
            .any(|other| location.aliases(other))
    })
}

fn contains_location(locations: &[&FileLocation], candidate: &FileLocation) -> bool {
    locations.iter().any(|location| location.aliases(candidate))
}

fn trim_line_ending(bytes: &mut Vec<u8>) {
    if bytes.ends_with(&[13, 10]) {
        bytes.truncate(bytes.len() - 2);
    } else if bytes.ends_with(&[10]) {
        bytes.truncate(bytes.len() - 1);
    }
}

fn map_config_file_error(error: SecureFileError) -> ActorConfigError {
    match error {
        SecureFileError::Unavailable => ActorConfigError::ConfigurationUnavailable,
        SecureFileError::Unsafe => ActorConfigError::UnsafeConfigurationFile,
    }
}

fn map_command_location_error(error: SecureFileError) -> ActorConfigError {
    match error {
        SecureFileError::Unavailable => ActorConfigError::CommandMaterialUnavailable,
        SecureFileError::Unsafe => ActorConfigError::UnsafeCommandMaterialFile,
    }
}
