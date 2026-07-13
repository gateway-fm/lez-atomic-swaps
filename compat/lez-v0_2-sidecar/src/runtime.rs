use std::{fmt, net::IpAddr, sync::Arc, time::Duration};

use async_trait::async_trait;
use common::transaction::LeeTransaction;
use lee::{AccountId, PublicTransaction};
use lez_bridge_protocol::{Hex32, Participant, RuntimeCompatibility, RuntimeDescriptor};
use sequencer_service_rpc::{RpcClient as _, SequencerClient, SequencerClientBuilder};
use url::{Host, Url};

const MAX_EXACT_TRANSACTION_BYTES: usize = 2_000_000;
const MAX_NODE_REQUEST_BYTES: u32 = 2_800_000;
const MAX_NODE_RESPONSE_BYTES: u32 = 8 * 1024 * 1024;
const NODE_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// Fail-closed errors at the official LEZ v0.2 runtime boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum RuntimeBoundaryError {
    /// The configured graph is not the separately locked LEZ v0.2 graph.
    #[error("runtime compatibility is not pinned LEE v0.2.0")]
    WrongCompatibility,
    /// The configured descriptor and isolated process roles differ.
    #[error("runtime role does not match the isolated sidecar role")]
    WrongRole,
    /// The descriptor does not identify the official isolated signer.
    #[error("runtime signer does not match the isolated official signer")]
    WrongSigner,
    /// A required runtime identity is the impossible all-zero value.
    #[error("runtime descriptor contains an invalid zero identity")]
    InvalidRuntimeIdentity,
    /// The official node endpoint is not an explicit loopback HTTP IP and port.
    #[error("official node endpoint must be an uncredentialed HTTP loopback IP and port")]
    InvalidNodeEndpoint,
    /// The official sequencer health or channel RPC was unavailable.
    #[error("official LEZ v0.2 node health is unavailable")]
    NodeUnavailable,
    /// The official sequencer returned a different configured channel.
    #[error("official LEZ v0.2 node channel does not match the runtime descriptor")]
    WrongChannel,
    /// Exact bytes were not one canonical, signed official LEE public transaction.
    #[error("bytes are not a canonical signed official LEE v0.2 public transaction")]
    InvalidOfficialTransaction,
}

/// Source-correct facts returned by the official v0.2 sequencer health boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RuntimeHealth {
    channel_id: [u8; 32],
}

impl RuntimeHealth {
    /// Creates a health result from the official sequencer `ChannelId` bytes.
    #[must_use]
    pub const fn new(channel_id: [u8; 32]) -> Self {
        Self { channel_id }
    }

    /// Returns the exact official channel identity observed with the health check.
    #[must_use]
    pub const fn channel_id(&self) -> &[u8; 32] {
        &self.channel_id
    }
}

/// Supplies official sequencer health and channel facts without synthetic fallback.
#[async_trait]
pub trait HealthProbe: Send + Sync {
    /// Calls the source-correct v0.2 health boundary.
    ///
    /// # Errors
    ///
    /// Returns an error when either required official RPC fact is unavailable.
    async fn check_health(&self) -> Result<RuntimeHealth, RuntimeBoundaryError>;
}

/// Exact descriptor, role, signer, and official-node health binding for one actor.
pub struct RuntimeBoundary {
    descriptor: RuntimeDescriptor,
    role: Participant,
    signer_account_id: AccountId,
    health_probe: Arc<dyn HealthProbe>,
}

impl fmt::Debug for RuntimeBoundary {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RuntimeBoundary")
            .field("descriptor", &self.descriptor)
            .field("role", &self.role)
            .field("signer_account_id", &self.signer_account_id)
            .finish_non_exhaustive()
    }
}

impl RuntimeBoundary {
    /// Binds a complete v0.2 descriptor to one official LEE account and role.
    ///
    /// # Errors
    ///
    /// Rejects a legacy/cross-wired graph, role, signer, or zero identity.
    pub fn new(
        descriptor: RuntimeDescriptor,
        role: Participant,
        signer_account_id: AccountId,
        health_probe: Arc<dyn HealthProbe>,
    ) -> Result<Self, RuntimeBoundaryError> {
        if descriptor.compatibility != RuntimeCompatibility::LeeV0_2_0 {
            return Err(RuntimeBoundaryError::WrongCompatibility);
        }
        if descriptor.sidecar_role != role {
            return Err(RuntimeBoundaryError::WrongRole);
        }
        if descriptor.signer_account_id != Hex32::from_bytes(signer_account_id.into_value()) {
            return Err(RuntimeBoundaryError::WrongSigner);
        }
        if [
            descriptor.chain_id,
            descriptor.channel_id,
            descriptor.genesis_block_hash,
            descriptor.escrow_program_id,
            descriptor.signer_account_id,
        ]
        .iter()
        .any(|identity| identity.as_bytes() == &[0; 32])
        {
            return Err(RuntimeBoundaryError::InvalidRuntimeIdentity);
        }
        Ok(Self {
            descriptor,
            role,
            signer_account_id,
            health_probe,
        })
    }

    /// Returns the exact immutable descriptor served by `describe_runtime`.
    pub const fn describe(&self) -> &RuntimeDescriptor {
        &self.descriptor
    }

    /// Proves official sequencer health and exact channel identity.
    ///
    /// # Errors
    ///
    /// Returns an error rather than treating an unavailable or different channel as healthy.
    pub async fn verify_health(&self) -> Result<RuntimeHealth, RuntimeBoundaryError> {
        let health = self.health_probe.check_health().await?;
        if health.channel_id != *self.descriptor.channel_id.as_bytes() {
            return Err(RuntimeBoundaryError::WrongChannel);
        }
        Ok(health)
    }
}

/// Direct, bounded client for the official v0.2 sequencer health RPCs.
#[derive(Clone)]
pub struct OfficialNodeRpc {
    client: SequencerClient,
}

impl fmt::Debug for OfficialNodeRpc {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OfficialNodeRpc")
            .finish_non_exhaustive()
    }
}

impl OfficialNodeRpc {
    /// Connects to an explicit local sequencer without retries or proxy indirection.
    ///
    /// # Errors
    ///
    /// Rejects any non-HTTP, non-loopback, credentialed, ambiguous, or portless URL.
    pub fn connect(endpoint: &str) -> Result<Self, RuntimeBoundaryError> {
        validate_node_endpoint(endpoint)?;
        let client = SequencerClientBuilder::default()
            .max_request_size(MAX_NODE_REQUEST_BYTES)
            .max_response_size(MAX_NODE_RESPONSE_BYTES)
            .request_timeout(NODE_REQUEST_TIMEOUT)
            .max_concurrent_requests(1)
            .build(endpoint)
            .map_err(|_| RuntimeBoundaryError::InvalidNodeEndpoint)?;
        Ok(Self { client })
    }
}

#[async_trait]
impl HealthProbe for OfficialNodeRpc {
    async fn check_health(&self) -> Result<RuntimeHealth, RuntimeBoundaryError> {
        self.client
            .check_health()
            .await
            .map_err(|_| RuntimeBoundaryError::NodeUnavailable)?;
        let channel = self
            .client
            .get_channel_id()
            .await
            .map_err(|_| RuntimeBoundaryError::NodeUnavailable)?;
        Ok(RuntimeHealth::new(channel.0))
    }
}

/// Decodes and statelessly validates exact bytes with official v0.2 LEE types.
///
/// This deliberately does not expose a bridge method yet. It establishes that
/// later planners consume the upstream `PublicTransaction` and `LeeTransaction`
/// representations rather than a hand-copied wire model.
///
/// # Errors
///
/// Rejects empty/oversized, noncanonical, or invalidly signed public transactions.
pub fn decode_official_public_transaction(
    exact_bytes: &[u8],
) -> Result<PublicTransaction, RuntimeBoundaryError> {
    if exact_bytes.is_empty() || exact_bytes.len() > MAX_EXACT_TRANSACTION_BYTES {
        return Err(RuntimeBoundaryError::InvalidOfficialTransaction);
    }
    let transaction = PublicTransaction::from_bytes(exact_bytes)
        .map_err(|_| RuntimeBoundaryError::InvalidOfficialTransaction)?;
    if transaction.to_bytes() != exact_bytes {
        return Err(RuntimeBoundaryError::InvalidOfficialTransaction);
    }
    LeeTransaction::Public(transaction.clone())
        .transaction_stateless_check()
        .map_err(|_| RuntimeBoundaryError::InvalidOfficialTransaction)?;
    Ok(transaction)
}

fn validate_node_endpoint(endpoint: &str) -> Result<(), RuntimeBoundaryError> {
    let parsed = Url::parse(endpoint).map_err(|_| RuntimeBoundaryError::InvalidNodeEndpoint)?;
    let loopback = match parsed.host() {
        Some(Host::Ipv4(address)) => IpAddr::V4(address).is_loopback(),
        Some(Host::Ipv6(address)) => IpAddr::V6(address).is_loopback(),
        Some(Host::Domain(_)) | None => false,
    };
    if parsed.scheme() != "http"
        || !loopback
        || parsed.port().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
        || parsed.path() != "/"
    {
        return Err(RuntimeBoundaryError::InvalidNodeEndpoint);
    }
    Ok(())
}
