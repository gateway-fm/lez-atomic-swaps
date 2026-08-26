//! Fail-closed Monero output observation for the local M4 proof of concept.
//!
//! This adapter talks only to project-owned `monerod` and wallet RPC processes
//! through literal-loopback HTTP endpoints. It composes the maintained, typed
//! [`monero_rpc`] daemon and wallet calls; it does not duplicate JSON-RPC wire
//! schemas. A successful observation binds the configured network and genesis,
//! exact transaction, exact shared destination and amount, canonical containing
//! block, wallet-reported availability, ten confirmations, and one stable tip.
//! The result is deliberately not release authority: a durable actor must consume
//! it into a Stage-B-bound, at-most-once capability before publishing any secret.
//!
//! # Trust and transport boundary
//!
//! The wallet RPC is trusted to own the shared view key and report the hidden
//! destination and amount. The daemon and wallet endpoints must be distinct
//! project-owned local origins. Public and DNS endpoints are rejected at
//! construction. Credential configuration does not itself prove that a process
//! enforces authentication; the isolated topology gate must retain a run-bound
//! wrong-credential rejection.
//!
//! [`monero_rpc`] 0.5.1 applies a hard request timeout, but its private transport
//! buffers JSON before returning typed values and exposes no response-body limit.
//! Consequently, pre-decode body limiting is not possible without replacing its
//! typed transport. Selected decoded collection bounds fail closed after
//! decoding. See [`TRANSPORT_RESIDUAL`].

#![forbid(unsafe_code)]

mod topology;
mod wallet_effect;

use std::fmt;
use std::num::NonZeroU64;
use std::time::Duration;

use async_trait::async_trait;
use monero_rpc::monero::Network as AddressNetwork;
use monero_rpc::monero::blockdata::block::Block;
use monero_rpc::monero::cryptonote::subaddress::Index as SubaddressIndex;
use monero_rpc::monero::util::address::AddressType;
use monero_rpc::{
    BlockHeaderResponse, DaemonJsonRpcClient, DaemonRpcClient, GetBlockHeaderSelector,
    GetTransfersCategory, GotTransfer, IncomingTransfer, RpcAuthentication, RpcClient,
    RpcClientBuilder, Transaction, TransactionsResponse, TransferHeight, TransferType,
    WalletClient,
};
use thiserror::Error;
use url::{Host, Url};

pub use topology::{
    MoneroTopologyBindingError, MoneroTopologyError, MoneroTopologyVerifier,
    VerifiedMoneroTopologyAttestation,
};
pub use wallet_effect::{
    ConfirmedMoneroFunding, ConfirmedMoneroSweep, MoneroRegtestWalletEffects,
    MoneroWalletEffectError, SubmittedMoneroSweep,
};

/// Canonical Monero standard-address type used by exact output terms.
pub use monero_rpc::monero::Address as MoneroAddress;
/// Canonical `CryptoNote` transaction hash used by exact output terms.
pub use monero_rpc::monero::cryptonote::hash::Hash as MoneroTransactionId;

/// Exact confirmation policy committed by the M4 LEZ/XMR agreement.
pub const REQUIRED_MONERO_CONFIRMATIONS: u64 = 10;
/// Fixed deadline applied by `monero-rpc` to every daemon and wallet request.
pub const RPC_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// Maximum decoded available-output entries accepted from wallet RPC.
pub const MAX_AVAILABLE_OUTPUTS: usize = 4_096;
/// Maximum transaction hashes accepted from one decoded Monero block.
pub const MAX_BLOCK_TRANSACTION_HASHES: usize = 100_000;
/// Maximum decoded hexadecimal transaction field accepted after RPC decoding.
pub const MAX_TRANSACTION_HEX_BYTES: usize = 2_000_000;
/// Known transport residual retained for later upstream hardening.
///
/// `monero-rpc` 0.5.1 owns its `reqwest` response handling and calls
/// `Response::json` directly. It supports a request timeout but neither a
/// response-body limit nor an injected HTTP client. This `PoC` therefore confines
/// that decode boundary to credential-configured literal-loopback project processes;
/// public RPC is unsupported. Production must add an upstream byte limit or
/// move to an equivalently typed bounded transport.
pub const TRANSPORT_RESIDUAL: &str = "monero-rpc 0.5.1 has no pre-decode response-body limit; this PoC admits only credential-configured literal-loopback project processes and does not support public RPC";
/// Authentication residual at the typed RPC boundary.
///
/// Supplying Digest credentials does not prove the server challenges invalid
/// credentials. The actual-node topology gate must prove a run-bound 401 with
/// a foreign credential before this observation participates in release.
pub const AUTHENTICATION_RESIDUAL: &str = "configured RPC credentials require a run-bound wrong-credential rejection from the isolated topology; construction alone does not prove authentication enforcement";
/// Spend-status residual for a shared view-only wallet.
///
/// Without imported composite key images, a view-only wallet cannot reliably
/// prove that an old output remains unspent. M4 uses fresh activation/output
/// uniqueness and a durable one-shot release journal; this adapter result proves
/// canonical receipt and wallet-reported availability only.
pub const VIEW_ONLY_SPEND_RESIDUAL: &str = "view-only wallet spent=false is not unspent authority without composite key-image knowledge; use only fresh activation-bound output observation plus durable at-most-once release";
/// Header trust-flag residual in the upstream typed client.
///
/// The single-header method discards the daemon untrusted flag. The local proof
/// of concept
/// therefore requires the peerless offline Regtest topology; Stagenet release
/// must cross-check used headers through a typed method that preserves the flag.
pub const HEADER_TRUST_RESIDUAL: &str = "monero-rpc 0.5.1 single-header calls discard the daemon untrusted flag; accepted only for the attested peerless offline Regtest PoC";
/// Malformed-block availability residual in the upstream typed client.
///
/// The typed block method unwraps missing, malformed-hex, and undecodable blobs.
/// A faulty local service can panic its caller. This fails the actor closed but
/// requires upstream repair or process containment before production.
pub const BLOCK_DECODE_RESIDUAL: &str = "monero-rpc 0.5.1 get_block can panic on malformed block responses; isolate or repair this path before production";

const MAX_CREDENTIAL_BYTES: usize = 128;
const SHARED_WALLET_ACCOUNT_INDEX: u32 = 0;

/// Exact Monero deployment profile admitted by this adapter.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MoneroNetwork {
    /// Offline official `monerod --regtest`; wallet addresses use Mainnet bytes.
    Regtest,
    /// Self-hosted Stagenet reached through a literal-loopback RPC process.
    Stagenet,
}

impl MoneroNetwork {
    const fn address_network(self) -> AddressNetwork {
        match self {
            Self::Regtest => AddressNetwork::Mainnet,
            Self::Stagenet => AddressNetwork::Stagenet,
        }
    }
}

/// Network plus exact height-zero block identity committed before funding.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MoneroChainIdentity {
    network: MoneroNetwork,
    genesis_hash: [u8; 32],
}

/// Read-only typed attestor for an actual local daemon's height-zero identity.
///
/// This boundary owns no wallet, output, signing, or submission capability. It
/// exists so a new agreement can bind the chain it actually contacted instead
/// of accepting a caller-supplied genesis hash.
pub struct MoneroChainIdentityAttestor {
    network: MoneroNetwork,
    daemon: DaemonJsonRpcClient,
    daemon_origin: String,
}

impl MoneroChainIdentityAttestor {
    /// Builds one Digest-authenticated, literal-loopback discovery boundary.
    ///
    /// # Errors
    ///
    /// Returns [`MoneroEvidenceError::RpcClientBuild`] if the maintained typed
    /// client cannot be constructed. Route and credential validation happens in
    /// [`LoopbackRpcEndpoint::new`] before this method.
    pub fn new(
        network: MoneroNetwork,
        daemon: &LoopbackRpcEndpoint,
    ) -> Result<Self, MoneroEvidenceError> {
        let client = daemon
            .client()
            .map_err(|source| MoneroEvidenceError::RpcClientBuild {
                endpoint: "daemon JSON",
                source,
            })?
            .daemon();
        Ok(Self {
            network,
            daemon: client,
            daemon_origin: daemon.base_url.clone(),
        })
    }

    /// Discovers the exact nonzero block hash at height zero.
    ///
    /// # Errors
    ///
    /// Fails closed if the typed RPC fails or returns an all-zero placeholder.
    pub async fn discover(&self) -> Result<MoneroChainIdentity, MoneroEvidenceError> {
        let hash =
            MoneroRpcPort::rpc("on_get_block_hash(0)", self.daemon.on_get_block_hash(0)).await?;
        identity_from_observed_hash(self.network, hash_bytes(hash.as_ref()))
    }

    /// Non-secret exact daemon origin used for discovery.
    #[must_use]
    pub fn daemon_origin(&self) -> &str {
        &self.daemon_origin
    }
}

impl fmt::Debug for MoneroChainIdentityAttestor {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MoneroChainIdentityAttestor")
            .field("network", &self.network)
            .field("daemon_origin", &self.daemon_origin)
            .field("credentials", &"[REDACTED]")
            .finish_non_exhaustive()
    }
}

impl MoneroChainIdentity {
    /// Creates one exact named chain identity.
    ///
    /// # Errors
    ///
    /// Rejects an all-zero placeholder genesis hash.
    pub fn new(
        network: MoneroNetwork,
        genesis_hash: [u8; 32],
    ) -> Result<Self, MoneroConfigurationError> {
        if genesis_hash == [0; 32] {
            return Err(MoneroConfigurationError::ZeroGenesisHash);
        }
        Ok(Self {
            network,
            genesis_hash,
        })
    }

    /// Named Monero network.
    #[must_use]
    pub const fn network(self) -> MoneroNetwork {
        self.network
    }

    /// Exact height-zero block hash.
    #[must_use]
    pub const fn genesis_hash(self) -> [u8; 32] {
        self.genesis_hash
    }
}

/// Authentication and route for one project-owned local RPC process.
pub struct LoopbackRpcEndpoint {
    base_url: String,
    username: String,
    password: String,
}

impl LoopbackRpcEndpoint {
    /// Validates one credential-configured literal-loopback HTTP endpoint.
    ///
    /// `localhost`, DNS, public IPs, embedded URL credentials, paths, query
    /// strings, fragments, missing ports, and empty credentials are rejected.
    ///
    /// # Errors
    ///
    /// Returns a typed configuration error when any route or credential bound
    /// is violated.
    pub fn new(
        endpoint: &str,
        username: impl Into<String>,
        password: impl Into<String>,
    ) -> Result<Self, MoneroConfigurationError> {
        let parsed = Url::parse(endpoint).map_err(|_| MoneroConfigurationError::InvalidRpcUrl)?;
        if parsed.scheme() != "http" {
            return Err(MoneroConfigurationError::RpcMustUseHttp);
        }
        if !parsed.username().is_empty() || parsed.password().is_some() {
            return Err(MoneroConfigurationError::EmbeddedCredentials);
        }
        if parsed.path() != "/" || parsed.query().is_some() || parsed.fragment().is_some() {
            return Err(MoneroConfigurationError::UnexpectedRpcUrlSuffix);
        }
        match parsed.host() {
            Some(Host::Ipv4(address)) if address.is_loopback() => {}
            Some(Host::Ipv6(address)) if address.is_loopback() => {}
            _ => return Err(MoneroConfigurationError::RpcMustBeLiteralLoopback),
        }
        if parsed.port().is_none() {
            return Err(MoneroConfigurationError::MissingRpcPort);
        }

        let username = username.into();
        let password = password.into();
        if !valid_credential(&username, false) || !valid_credential(&password, true) {
            return Err(MoneroConfigurationError::InvalidRpcCredentials);
        }

        Ok(Self {
            base_url: parsed.as_str().trim_end_matches('/').to_owned(),
            username,
            password,
        })
    }

    /// Non-secret exact RPC origin.
    #[must_use]
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    fn client(&self) -> Result<RpcClient, anyhow::Error> {
        RpcClientBuilder::new()
            .timeout(RPC_REQUEST_TIMEOUT)
            .rpc_authentication(RpcAuthentication::Credentials {
                username: self.username.clone(),
                password: self.password.clone(),
            })
            .build(&self.base_url)
    }
}

impl fmt::Debug for LoopbackRpcEndpoint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LoopbackRpcEndpoint")
            .field("base_url", &self.base_url)
            .field("username", &self.username)
            .field("password", &"[REDACTED]")
            .finish()
    }
}

/// Exact expected shared-wallet output. This input never authorizes release.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExpectedMoneroOutput {
    transaction_id: MoneroTransactionId,
    destination: MoneroAddress,
    amount_piconero: NonZeroU64,
}

impl ExpectedMoneroOutput {
    /// Creates exact output terms from the countersigned agreement.
    ///
    /// # Errors
    ///
    /// Rejects zero transaction IDs, non-standard destinations, and zero value.
    pub fn new(
        transaction_id: MoneroTransactionId,
        destination: MoneroAddress,
        amount_piconero: u64,
    ) -> Result<Self, MoneroConfigurationError> {
        if transaction_id.as_ref() == [0; 32] {
            return Err(MoneroConfigurationError::ZeroTransactionId);
        }
        if destination.addr_type != AddressType::Standard {
            return Err(MoneroConfigurationError::NonStandardSharedAddress);
        }
        let amount_piconero =
            NonZeroU64::new(amount_piconero).ok_or(MoneroConfigurationError::ZeroOutputAmount)?;
        Ok(Self {
            transaction_id,
            destination,
            amount_piconero,
        })
    }

    /// Exact transaction ID.
    #[must_use]
    pub const fn transaction_id(&self) -> MoneroTransactionId {
        self.transaction_id
    }

    /// Exact standard shared destination.
    #[must_use]
    pub const fn destination(&self) -> MoneroAddress {
        self.destination
    }

    /// Exact principal in piconero.
    #[must_use]
    pub const fn amount_piconero(&self) -> u64 {
        self.amount_piconero.get()
    }
}

/// Non-forgeable observation that one exact Monero output passed chain checks.
///
/// All fields are private and this crate exposes no public mock/status validation
/// port or constructor. Only [`MoneroOutputVerifier::verify`] can create it.
///
/// This type is intentionally not cloneable. It still is not swap authorization:
/// an actor must consume it into a durable Stage-B-bound, at-most-once release
/// capability after verifying the run RPC-authentication attestation.
#[derive(Debug, Eq, PartialEq)]
#[must_use]
pub struct VerifiedMoneroOutputObservation {
    network: MoneroNetwork,
    genesis_hash: [u8; 32],
    daemon_origin: String,
    wallet_origin: String,
    transaction_id: MoneroTransactionId,
    destination: MoneroAddress,
    amount_piconero: u64,
    containing_block_hash: [u8; 32],
    containing_block_height: u64,
    confirmations: u64,
    stable_tip_hash: [u8; 32],
    stable_tip_height: u64,
    _non_forgeable: private::EvidenceSeal,
}

impl VerifiedMoneroOutputObservation {
    /// Verified named network.
    #[must_use]
    pub const fn network(&self) -> MoneroNetwork {
        self.network
    }

    /// Verified height-zero identity.
    #[must_use]
    pub const fn genesis_hash(&self) -> [u8; 32] {
        self.genesis_hash
    }

    /// Exact daemon RPC origin used to create this observation.
    #[must_use]
    pub fn daemon_origin(&self) -> &str {
        &self.daemon_origin
    }

    /// Exact shared-wallet RPC origin used to create this observation.
    #[must_use]
    pub fn wallet_origin(&self) -> &str {
        &self.wallet_origin
    }

    /// Verified transaction ID.
    #[must_use]
    pub const fn transaction_id(&self) -> MoneroTransactionId {
        self.transaction_id
    }

    /// Verified shared destination.
    #[must_use]
    pub const fn destination(&self) -> MoneroAddress {
        self.destination
    }

    /// Verified principal in piconero.
    #[must_use]
    pub const fn amount_piconero(&self) -> u64 {
        self.amount_piconero
    }

    /// Canonical containing block hash.
    #[must_use]
    pub const fn containing_block_hash(&self) -> [u8; 32] {
        self.containing_block_hash
    }

    /// Canonical containing block height.
    #[must_use]
    pub const fn containing_block_height(&self) -> u64 {
        self.containing_block_height
    }

    /// Exact confirmations at the stable tip.
    #[must_use]
    pub const fn confirmations(&self) -> u64 {
        self.confirmations
    }

    /// Stable bracketing tip hash.
    #[must_use]
    pub const fn stable_tip_hash(&self) -> [u8; 32] {
        self.stable_tip_hash
    }

    /// Stable bracketing tip height.
    #[must_use]
    pub const fn stable_tip_height(&self) -> u64 {
        self.stable_tip_height
    }
}

/// Literal-loopback verifier backed by typed `monero-rpc` 0.5.1 clients.
pub struct MoneroOutputVerifier {
    identity: MoneroChainIdentity,
    rpc: MoneroRpcPort,
}

impl MoneroOutputVerifier {
    /// Builds the local daemon/wallet observation composite.
    ///
    /// # Errors
    ///
    /// Returns [`MoneroEvidenceError::RpcClientBuild`] when a typed client
    /// cannot be constructed or when service origins alias. Endpoint validation
    /// happens before this method.
    pub fn new(
        identity: MoneroChainIdentity,
        daemon: &LoopbackRpcEndpoint,
        wallet: &LoopbackRpcEndpoint,
    ) -> Result<Self, MoneroEvidenceError> {
        if daemon.base_url == wallet.base_url {
            return Err(MoneroEvidenceError::AliasedRpcOrigins);
        }
        Ok(Self {
            identity,
            rpc: MoneroRpcPort::new(identity, daemon, wallet)?,
        })
    }

    /// Observes and validates one exact canonical wallet-available output.
    ///
    /// # Errors
    ///
    /// Fails closed on RPC failure, network/genesis mismatch, any transaction,
    /// destination, amount, pool, reported-spend, unlock, block, confirmation,
    /// orphan, selected response-bound, or stable-tip mismatch.
    ///
    /// Success proves neither authentication enforcement nor durable one-shot
    /// release. Callers must satisfy the authentication residual and consume the
    /// result into an activation-bound journal capability.
    pub async fn verify(
        &self,
        expected: &ExpectedMoneroOutput,
    ) -> Result<VerifiedMoneroOutputObservation, MoneroEvidenceError> {
        verify_with_port(&self.rpc, self.identity, expected).await
    }
}

impl fmt::Debug for MoneroOutputVerifier {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MoneroOutputVerifier")
            .field("identity", &self.identity)
            .field("transport", &"credential-configured-literal-loopback")
            .field("credentials", &"[REDACTED]")
            .finish_non_exhaustive()
    }
}

/// Configuration failures that happen before any verified observation.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum MoneroConfigurationError {
    /// URL parsing failed.
    #[error("Monero RPC URL is invalid")]
    InvalidRpcUrl,
    /// TLS/public transports are outside this progressive `PoC`.
    #[error("Monero local PoC RPC must use HTTP")]
    RpcMustUseHttp,
    /// URL userinfo could leak credentials through diagnostics.
    #[error("Monero RPC credentials must not be embedded in the URL")]
    EmbeddedCredentials,
    /// `monero-rpc` expects one bare origin and appends its own paths.
    #[error("Monero RPC URL must not contain a path, query, or fragment")]
    UnexpectedRpcUrlSuffix,
    /// Only numeric loopback addresses are admitted.
    #[error("Monero RPC must use a literal loopback address")]
    RpcMustBeLiteralLoopback,
    /// Exact local service identity includes an explicit port.
    #[error("Monero RPC URL must include an explicit port")]
    MissingRpcPort,
    /// Credentials are empty, oversized, or contain control delimiters.
    #[error("Monero RPC credentials violate the local bounded credential policy")]
    InvalidRpcCredentials,
    /// Placeholder genesis identities are forbidden.
    #[error("Monero genesis hash must not be zero")]
    ZeroGenesisHash,
    /// Placeholder transaction identities are forbidden.
    #[error("Monero transaction ID must not be zero")]
    ZeroTransactionId,
    /// Shared two-party wallet uses one standard address, not an integrated or subaddress output.
    #[error("Monero shared destination must be a standard address")]
    NonStandardSharedAddress,
    /// Zero-value locks are forbidden.
    #[error("Monero output amount must be nonzero")]
    ZeroOutputAmount,
}

/// Fail-closed observation and typed-RPC errors.
#[derive(Debug, Error)]
pub enum MoneroEvidenceError {
    /// Daemon and wallet trust roles cannot be served by one RPC origin.
    #[error("Monero daemon and wallet RPC origins must be distinct")]
    AliasedRpcOrigins,
    /// Typed RPC client creation failed.
    #[error("failed to build typed Monero {endpoint} RPC client")]
    RpcClientBuild {
        /// Non-secret endpoint role.
        endpoint: &'static str,
        /// Underlying transport construction failure.
        #[source]
        source: anyhow::Error,
    },
    /// One exact typed RPC call failed or returned an undecodable response.
    #[error("typed Monero RPC operation `{operation}` failed")]
    Rpc {
        /// Stable non-secret operation name.
        operation: &'static str,
        /// Underlying typed client failure.
        #[source]
        source: anyhow::Error,
    },
    /// The actual daemon returned an unusable all-zero height-zero identity.
    #[error("Monero daemon returned an all-zero height-zero hash")]
    ZeroObservedGenesisHash,
    /// A decoded collection exceeded its post-decode `PoC` bound.
    #[error("decoded Monero `{resource}` exceeded the semantic bound {maximum}")]
    SemanticResponseBound {
        /// Bounded response field.
        resource: &'static str,
        /// Maximum accepted entries or bytes.
        maximum: usize,
    },
    /// Destination bytes do not belong to the named profile.
    #[error("shared Monero destination network does not match the named chain profile")]
    DestinationNetworkMismatch,
    /// Daemon height-zero identity differs from the agreement.
    #[error("Monero daemon genesis hash does not match the configured identity")]
    GenesisMismatch,
    /// Tip changed while the wallet/daemon composite was observed.
    #[error("Monero tip was not stable across the output observation")]
    UnstableTip,
    /// A tip or containing block header was reported orphaned.
    #[error("Monero observation references an orphan block")]
    OrphanBlock,
    /// A last-block response was not actually at depth zero.
    #[error("Monero last-block header reported a nonzero depth")]
    InvalidTipDepth,
    /// Wallet did not know the exact transaction.
    #[error("shared Monero wallet does not contain the exact transaction")]
    MissingWalletTransfer,
    /// Wallet transaction ID differed or was malformed.
    #[error("shared Monero wallet returned a different transaction ID")]
    TransactionIdMismatch,
    /// Wallet transfer was not incoming.
    #[error("shared Monero wallet transfer is not incoming")]
    TransferNotIncoming,
    /// Wallet destination differed.
    #[error("shared Monero wallet destination does not match the agreement")]
    DestinationMismatch,
    /// Wallet or available-output amount differed.
    #[error("shared Monero wallet amount does not match the agreement")]
    AmountMismatch,
    /// Wallet transfer remains in the pool.
    #[error("shared Monero wallet transfer is not confirmed in a block")]
    WalletTransferInPool,
    /// Daemon or wallet observed a double spend.
    #[error("Monero transaction has a double-spend observation")]
    DoubleSpendSeen,
    /// Wallet RPC did not classify exactly one matching output as available.
    #[error("exact Monero output is not uniquely wallet-available and unlocked")]
    OutputNotUnlocked,
    /// Wallet RPC affirmatively reports that the available output is spent.
    #[error("Monero wallet reports the exact available output as spent")]
    OutputAlreadySpent,
    /// Daemon response did not have trusted `OK` status.
    #[error("Monero daemon transaction response is not trusted OK")]
    UntrustedDaemonResponse,
    /// Daemon reports the requested transaction as missed.
    #[error("Monero daemon missed the exact transaction")]
    DaemonMissedTransaction,
    /// Daemon did not return exactly one transaction.
    #[error("Monero daemon did not return exactly one transaction")]
    AmbiguousDaemonTransaction,
    /// Daemon transaction remains in its pool.
    #[error("Monero daemon transaction is not in a block")]
    DaemonTransactionInPool,
    /// Wallet and daemon disagree on containing height.
    #[error("Monero wallet and daemon disagree on the containing block height")]
    ContainingHeightMismatch,
    /// Header-by-height and returned block do not share one identity.
    #[error("Monero containing block bytes do not match the canonical header")]
    ContainingBlockMismatch,
    /// Canonical block does not contain the exact transaction hash once.
    #[error("canonical Monero block does not contain the exact transaction exactly once")]
    TransactionMembershipMismatch,
    /// Tip is behind the purported containing block.
    #[error("stable Monero tip is behind the containing block")]
    TipBehindContainingBlock,
    /// Fewer than ten canonical confirmations exist.
    #[error("Monero output has fewer than ten canonical confirmations")]
    InsufficientConfirmations,
    /// Wallet confirmation count differs from the canonical height calculation.
    #[error("Monero wallet confirmations disagree with the canonical chain")]
    ConfirmationMismatch,
    /// Header depth differs from the canonical stable-tip calculation.
    #[error("Monero containing header depth disagrees with the stable tip")]
    HeaderDepthMismatch,
    /// Wallet output reports a nonzero remaining unlock distance.
    #[error("Monero wallet output still reports a nonzero unlock distance")]
    NonzeroUnlockDistance,
}

fn identity_from_observed_hash(
    network: MoneroNetwork,
    genesis_hash: [u8; 32],
) -> Result<MoneroChainIdentity, MoneroEvidenceError> {
    MoneroChainIdentity::new(network, genesis_hash)
        .map_err(|_| MoneroEvidenceError::ZeroObservedGenesisHash)
}

fn valid_credential(value: &str, colon_allowed: bool) -> bool {
    !value.is_empty()
        && value.len() <= MAX_CREDENTIAL_BYTES
        && value
            .bytes()
            .all(|byte| (0x21..=0x7e).contains(&byte) && (colon_allowed || byte != b':'))
}

#[derive(Clone, Debug)]
struct HeaderSnapshot {
    hash: [u8; 32],
    height: u64,
    depth: u64,
    orphan: bool,
}

impl From<BlockHeaderResponse> for HeaderSnapshot {
    fn from(header: BlockHeaderResponse) -> Self {
        Self {
            hash: hash_bytes(header.hash.as_ref()),
            height: header.height,
            depth: header.depth,
            orphan: header.orphan_status,
        }
    }
}

#[derive(Clone, Debug)]
struct WalletTransferSnapshot {
    transaction_id: Vec<u8>,
    destination: MoneroAddress,
    amount_piconero: u64,
    confirmations: Option<u64>,
    double_spend_seen: bool,
    height: Option<u64>,
    incoming: bool,
    in_pool: bool,
    subaddress: SubaddressIndex,
    unlock_distance: u64,
}

impl From<GotTransfer> for WalletTransferSnapshot {
    fn from(transfer: GotTransfer) -> Self {
        Self {
            transaction_id: transfer.txid.0,
            destination: transfer.address,
            amount_piconero: transfer.amount.as_pico(),
            confirmations: transfer.confirmations,
            double_spend_seen: transfer.double_spend_seen,
            height: match transfer.height {
                TransferHeight::Confirmed(height) => Some(height.get()),
                TransferHeight::InPool => None,
            },
            incoming: matches!(
                transfer.transfer_type,
                GetTransfersCategory::In | GetTransfersCategory::Pool
            ),
            in_pool: transfer.transfer_type == GetTransfersCategory::Pool,
            subaddress: transfer.subaddr_index,
            unlock_distance: transfer.unlock_time,
        }
    }
}

#[derive(Clone, Debug)]
struct AvailableOutputSnapshot {
    transaction_id: [u8; 32],
    amount_piconero: u64,
    spent: bool,
    subaddress: SubaddressIndex,
    block_height: Option<u64>,
}

impl From<IncomingTransfer> for AvailableOutputSnapshot {
    fn from(output: IncomingTransfer) -> Self {
        Self {
            transaction_id: hash_bytes(output.tx_hash.0.as_ref()),
            amount_piconero: output.amount.as_pico(),
            spent: output.spent,
            subaddress: output.subaddr_index,
            block_height: output.block_height,
        }
    }
}

#[derive(Clone, Debug)]
struct DaemonTransactionSnapshot {
    transaction_id: [u8; 32],
    block_height: Option<u64>,
    double_spend_seen: bool,
    in_pool: bool,
}

impl From<Transaction> for DaemonTransactionSnapshot {
    fn from(transaction: Transaction) -> Self {
        Self {
            transaction_id: hash_bytes(transaction.tx_hash.0.as_ref()),
            block_height: transaction.block_height,
            double_spend_seen: transaction.double_spend_seen,
            in_pool: transaction.in_pool,
        }
    }
}

#[derive(Clone, Debug)]
struct BlockSnapshot {
    header: HeaderSnapshot,
    decoded_block_hash: [u8; 32],
    transaction_ids: Vec<[u8; 32]>,
}

#[derive(Clone, Debug)]
struct ObservationSnapshot {
    daemon_origin: String,
    wallet_origin: String,
    genesis_hash: [u8; 32],
    tip_before: HeaderSnapshot,
    wallet_transfer: Option<WalletTransferSnapshot>,
    available_outputs: Vec<AvailableOutputSnapshot>,
    daemon_status_ok: bool,
    daemon_untrusted: bool,
    daemon_missed: usize,
    daemon_transactions: Vec<DaemonTransactionSnapshot>,
    containing_block: Option<BlockSnapshot>,
    tip_after: HeaderSnapshot,
}

#[async_trait]
trait ObservationPort: Send + Sync {
    async fn observe(
        &self,
        expected: &ExpectedMoneroOutput,
    ) -> Result<ObservationSnapshot, MoneroEvidenceError>;
}

struct MoneroRpcPort {
    identity: MoneroChainIdentity,
    daemon: DaemonJsonRpcClient,
    daemon_rpc: DaemonRpcClient,
    wallet: WalletClient,
    daemon_origin: String,
    wallet_origin: String,
}

impl MoneroRpcPort {
    fn new(
        identity: MoneroChainIdentity,
        daemon_endpoint: &LoopbackRpcEndpoint,
        wallet_endpoint: &LoopbackRpcEndpoint,
    ) -> Result<Self, MoneroEvidenceError> {
        let daemon = daemon_endpoint
            .client()
            .map_err(|source| MoneroEvidenceError::RpcClientBuild {
                endpoint: "daemon JSON",
                source,
            })?
            .daemon();
        let daemon_rpc = daemon_endpoint
            .client()
            .map_err(|source| MoneroEvidenceError::RpcClientBuild {
                endpoint: "daemon binary",
                source,
            })?
            .daemon_rpc();
        let wallet = wallet_endpoint
            .client()
            .map_err(|source| MoneroEvidenceError::RpcClientBuild {
                endpoint: "wallet",
                source,
            })?
            .wallet();
        Ok(Self {
            identity,
            daemon,
            daemon_rpc,
            wallet,
            daemon_origin: daemon_endpoint.base_url.clone(),
            wallet_origin: wallet_endpoint.base_url.clone(),
        })
    }

    async fn rpc<T>(
        operation: &'static str,
        future: impl Future<Output = Result<T, anyhow::Error>>,
    ) -> Result<T, MoneroEvidenceError> {
        future
            .await
            .map_err(|source| MoneroEvidenceError::Rpc { operation, source })
    }

    fn bound_transactions(response: &TransactionsResponse) -> Result<(), MoneroEvidenceError> {
        if response
            .missed_tx
            .as_ref()
            .is_some_and(|items| items.len() > 1)
        {
            return Err(MoneroEvidenceError::SemanticResponseBound {
                resource: "missed transactions",
                maximum: 1,
            });
        }
        if response.txs.as_ref().is_some_and(|items| items.len() > 1) {
            return Err(MoneroEvidenceError::SemanticResponseBound {
                resource: "transactions",
                maximum: 1,
            });
        }
        for value in response
            .txs
            .iter()
            .flatten()
            .map(|transaction| transaction.as_hex.as_str())
            .chain(response.txs_as_hex.iter().flatten().map(String::as_str))
            .chain(response.txs_as_json.iter().flatten().map(String::as_str))
        {
            if value.len() > MAX_TRANSACTION_HEX_BYTES {
                return Err(MoneroEvidenceError::SemanticResponseBound {
                    resource: "transaction text bytes",
                    maximum: MAX_TRANSACTION_HEX_BYTES,
                });
            }
        }
        Ok(())
    }

    async fn available_outputs(
        &self,
        transfer: Option<&WalletTransferSnapshot>,
    ) -> Result<Vec<AvailableOutputSnapshot>, MoneroEvidenceError> {
        let Some(transfer) = transfer else {
            return Ok(Vec::new());
        };
        let response = Self::rpc(
            "incoming_transfers(available)",
            self.wallet.incoming_transfers(
                TransferType::Available,
                Some(transfer.subaddress.major),
                Some(vec![transfer.subaddress.minor]),
            ),
        )
        .await?;
        let outputs = response.transfers.unwrap_or_default();
        if outputs.len() > MAX_AVAILABLE_OUTPUTS {
            return Err(MoneroEvidenceError::SemanticResponseBound {
                resource: "available outputs",
                maximum: MAX_AVAILABLE_OUTPUTS,
            });
        }
        Ok(outputs
            .into_iter()
            .map(AvailableOutputSnapshot::from)
            .collect())
    }
}

#[async_trait]
impl ObservationPort for MoneroRpcPort {
    async fn observe(
        &self,
        expected: &ExpectedMoneroOutput,
    ) -> Result<ObservationSnapshot, MoneroEvidenceError> {
        let genesis_hash =
            Self::rpc("on_get_block_hash(0)", self.daemon.on_get_block_hash(0)).await?;
        if hash_bytes(genesis_hash.as_ref()) != self.identity.genesis_hash() {
            return Err(MoneroEvidenceError::GenesisMismatch);
        }
        let wallet_transfer = Self::rpc(
            "get_transfer_by_txid",
            self.wallet
                .get_transfer(expected.transaction_id, Some(SHARED_WALLET_ACCOUNT_INDEX)),
        )
        .await?
        .map(WalletTransferSnapshot::from);
        let Some(transfer) = wallet_transfer.as_ref() else {
            return Err(MoneroEvidenceError::MissingWalletTransfer);
        };
        validate_wallet_transfer_identity(transfer, expected)?;
        if transfer.in_pool || transfer.height.is_none() {
            return Err(MoneroEvidenceError::WalletTransferInPool);
        }
        let tip_before = Self::rpc(
            "get_last_block_header(before)",
            self.daemon.get_block_header(GetBlockHeaderSelector::Last),
        )
        .await?;
        let available_outputs = self.available_outputs(wallet_transfer.as_ref()).await?;
        let transaction_response = Self::rpc(
            "get_transactions(pruned)",
            self.daemon_rpc.get_transactions(
                vec![expected.transaction_id],
                Some(false),
                Some(true),
            ),
        )
        .await?;
        Self::bound_transactions(&transaction_response)?;
        let containing_height = transaction_response
            .txs
            .as_ref()
            .and_then(|transactions| transactions.first())
            .and_then(|transaction| transaction.block_height);
        let containing_block = if let Some(height) = containing_height {
            let header = Self::rpc(
                "get_block_header_by_height",
                self.daemon
                    .get_block_header(GetBlockHeaderSelector::Height(height)),
            )
            .await?;
            let block = Self::rpc(
                "get_block_by_hash",
                self.daemon
                    .get_block(GetBlockHeaderSelector::Hash(header.hash)),
            )
            .await?;
            if block.tx_hashes.len() > MAX_BLOCK_TRANSACTION_HASHES {
                return Err(MoneroEvidenceError::SemanticResponseBound {
                    resource: "block transaction hashes",
                    maximum: MAX_BLOCK_TRANSACTION_HASHES,
                });
            }
            Some(block_snapshot(header, block))
        } else {
            None
        };

        let tip_after = Self::rpc(
            "get_last_block_header(after)",
            self.daemon.get_block_header(GetBlockHeaderSelector::Last),
        )
        .await?;

        Ok(ObservationSnapshot {
            daemon_origin: self.daemon_origin.clone(),
            wallet_origin: self.wallet_origin.clone(),
            genesis_hash: hash_bytes(genesis_hash.as_ref()),
            tip_before: tip_before.into(),
            wallet_transfer,
            available_outputs,
            daemon_status_ok: transaction_response.status == "OK",
            daemon_untrusted: transaction_response.untrusted,
            daemon_missed: transaction_response
                .missed_tx
                .map_or(0, |items| items.len()),
            daemon_transactions: transaction_response
                .txs
                .unwrap_or_default()
                .into_iter()
                .map(DaemonTransactionSnapshot::from)
                .collect(),
            containing_block,
            tip_after: tip_after.into(),
        })
    }
}

fn block_snapshot(header: BlockHeaderResponse, block: Block) -> BlockSnapshot {
    BlockSnapshot {
        header: header.into(),
        decoded_block_hash: block.id().0,
        transaction_ids: block.tx_hashes.into_iter().map(|hash| hash.0).collect(),
    }
}

async fn verify_with_port<P: ObservationPort>(
    port: &P,
    identity: MoneroChainIdentity,
    expected: &ExpectedMoneroOutput,
) -> Result<VerifiedMoneroOutputObservation, MoneroEvidenceError> {
    if expected.destination.network != identity.network.address_network() {
        return Err(MoneroEvidenceError::DestinationNetworkMismatch);
    }
    let snapshot = port.observe(expected).await?;
    validate_snapshot(&snapshot, identity, expected)
}

fn validate_snapshot(
    snapshot: &ObservationSnapshot,
    identity: MoneroChainIdentity,
    expected: &ExpectedMoneroOutput,
) -> Result<VerifiedMoneroOutputObservation, MoneroEvidenceError> {
    validate_chain(snapshot, identity)?;
    let (transfer, wallet_height) = validate_wallet(snapshot, expected)?;
    let daemon_height = validate_daemon(snapshot, expected, wallet_height)?;
    let (containing, confirmations) =
        validate_containing_block(snapshot, expected, daemon_height, transfer)?;

    Ok(VerifiedMoneroOutputObservation {
        network: identity.network,
        genesis_hash: identity.genesis_hash,
        daemon_origin: snapshot.daemon_origin.clone(),
        wallet_origin: snapshot.wallet_origin.clone(),
        transaction_id: expected.transaction_id,
        destination: expected.destination,
        amount_piconero: expected.amount_piconero.get(),
        containing_block_hash: containing.header.hash,
        containing_block_height: daemon_height,
        confirmations,
        stable_tip_hash: snapshot.tip_after.hash,
        stable_tip_height: snapshot.tip_after.height,
        _non_forgeable: private::EvidenceSeal,
    })
}

fn validate_chain(
    snapshot: &ObservationSnapshot,
    identity: MoneroChainIdentity,
) -> Result<(), MoneroEvidenceError> {
    if snapshot.genesis_hash != identity.genesis_hash {
        return Err(MoneroEvidenceError::GenesisMismatch);
    }
    if snapshot.tip_before.hash != snapshot.tip_after.hash
        || snapshot.tip_before.height != snapshot.tip_after.height
    {
        return Err(MoneroEvidenceError::UnstableTip);
    }
    if snapshot.tip_before.orphan || snapshot.tip_after.orphan {
        return Err(MoneroEvidenceError::OrphanBlock);
    }
    if snapshot.tip_before.depth != 0 || snapshot.tip_after.depth != 0 {
        return Err(MoneroEvidenceError::InvalidTipDepth);
    }
    Ok(())
}

fn validate_wallet<'a>(
    snapshot: &'a ObservationSnapshot,
    expected: &ExpectedMoneroOutput,
) -> Result<(&'a WalletTransferSnapshot, u64), MoneroEvidenceError> {
    let transfer = snapshot
        .wallet_transfer
        .as_ref()
        .ok_or(MoneroEvidenceError::MissingWalletTransfer)?;
    validate_wallet_transfer_identity(transfer, expected)?;
    if transfer.in_pool {
        return Err(MoneroEvidenceError::WalletTransferInPool);
    }
    let wallet_height = transfer
        .height
        .ok_or(MoneroEvidenceError::WalletTransferInPool)?;
    if transfer.unlock_distance != 0 {
        return Err(MoneroEvidenceError::NonzeroUnlockDistance);
    }

    let mut matching_available = snapshot.available_outputs.iter().filter(|output| {
        output.transaction_id == expected.transaction_id.0
            && output.subaddress == transfer.subaddress
    });
    let available = matching_available
        .next()
        .ok_or(MoneroEvidenceError::OutputNotUnlocked)?;
    if matching_available.next().is_some() {
        return Err(MoneroEvidenceError::OutputNotUnlocked);
    }
    if available.amount_piconero != expected.amount_piconero.get() {
        return Err(MoneroEvidenceError::AmountMismatch);
    }
    if available.spent {
        return Err(MoneroEvidenceError::OutputAlreadySpent);
    }
    if available
        .block_height
        .is_some_and(|height| height != wallet_height)
    {
        return Err(MoneroEvidenceError::ContainingHeightMismatch);
    }
    Ok((transfer, wallet_height))
}

fn validate_wallet_transfer_identity(
    transfer: &WalletTransferSnapshot,
    expected: &ExpectedMoneroOutput,
) -> Result<(), MoneroEvidenceError> {
    if transfer.transaction_id.as_slice() != expected.transaction_id.as_ref() {
        return Err(MoneroEvidenceError::TransactionIdMismatch);
    }
    if !transfer.incoming {
        return Err(MoneroEvidenceError::TransferNotIncoming);
    }
    if transfer.destination != expected.destination {
        return Err(MoneroEvidenceError::DestinationMismatch);
    }
    if transfer.amount_piconero != expected.amount_piconero.get() {
        return Err(MoneroEvidenceError::AmountMismatch);
    }
    if transfer.double_spend_seen {
        return Err(MoneroEvidenceError::DoubleSpendSeen);
    }
    Ok(())
}

fn validate_daemon(
    snapshot: &ObservationSnapshot,
    expected: &ExpectedMoneroOutput,
    wallet_height: u64,
) -> Result<u64, MoneroEvidenceError> {
    if !snapshot.daemon_status_ok || snapshot.daemon_untrusted {
        return Err(MoneroEvidenceError::UntrustedDaemonResponse);
    }
    if snapshot.daemon_missed != 0 {
        return Err(MoneroEvidenceError::DaemonMissedTransaction);
    }
    if snapshot.daemon_transactions.len() != 1 {
        return Err(MoneroEvidenceError::AmbiguousDaemonTransaction);
    }
    let daemon_transaction = &snapshot.daemon_transactions[0];
    if daemon_transaction.transaction_id != expected.transaction_id.0 {
        return Err(MoneroEvidenceError::TransactionIdMismatch);
    }
    if daemon_transaction.double_spend_seen {
        return Err(MoneroEvidenceError::DoubleSpendSeen);
    }
    if daemon_transaction.in_pool {
        return Err(MoneroEvidenceError::DaemonTransactionInPool);
    }
    let daemon_height = daemon_transaction
        .block_height
        .ok_or(MoneroEvidenceError::DaemonTransactionInPool)?;
    if daemon_height != wallet_height {
        return Err(MoneroEvidenceError::ContainingHeightMismatch);
    }
    Ok(daemon_height)
}

fn validate_containing_block<'a>(
    snapshot: &'a ObservationSnapshot,
    expected: &ExpectedMoneroOutput,
    daemon_height: u64,
    transfer: &WalletTransferSnapshot,
) -> Result<(&'a BlockSnapshot, u64), MoneroEvidenceError> {
    let containing = snapshot
        .containing_block
        .as_ref()
        .ok_or(MoneroEvidenceError::ContainingBlockMismatch)?;
    if containing.header.orphan {
        return Err(MoneroEvidenceError::OrphanBlock);
    }
    if containing.header.height != daemon_height
        || containing.header.hash != containing.decoded_block_hash
    {
        return Err(MoneroEvidenceError::ContainingBlockMismatch);
    }
    if containing
        .transaction_ids
        .iter()
        .filter(|transaction_id| **transaction_id == expected.transaction_id.0)
        .count()
        != 1
    {
        return Err(MoneroEvidenceError::TransactionMembershipMismatch);
    }

    let confirmations = snapshot
        .tip_after
        .height
        .checked_sub(daemon_height)
        .and_then(|depth| depth.checked_add(1))
        .ok_or(MoneroEvidenceError::TipBehindContainingBlock)?;
    if confirmations < REQUIRED_MONERO_CONFIRMATIONS {
        return Err(MoneroEvidenceError::InsufficientConfirmations);
    }
    if transfer.confirmations != Some(confirmations) {
        return Err(MoneroEvidenceError::ConfirmationMismatch);
    }
    if containing.header.depth != confirmations - 1 {
        return Err(MoneroEvidenceError::HeaderDepthMismatch);
    }
    Ok((containing, confirmations))
}

fn hash_bytes(bytes: &[u8]) -> [u8; 32] {
    bytes
        .try_into()
        .expect("typed Monero hash is exactly 32 bytes")
}

mod private {
    #[derive(Clone, Debug, Eq, PartialEq)]
    pub(super) struct EvidenceSeal;
}

#[cfg(test)]
mod tests {
    use super::*;
    use monero_rpc::monero::PrivateKey;
    use monero_rpc::monero::PublicKey;

    #[derive(Clone)]
    struct FixturePort {
        snapshot: ObservationSnapshot,
    }

    #[async_trait]
    impl ObservationPort for FixturePort {
        async fn observe(
            &self,
            _expected: &ExpectedMoneroOutput,
        ) -> Result<ObservationSnapshot, MoneroEvidenceError> {
            Ok(self.snapshot.clone())
        }
    }

    fn hash(byte: u8) -> MoneroTransactionId {
        MoneroTransactionId([byte; 32])
    }

    fn address(network: AddressNetwork) -> MoneroAddress {
        let mut spend = [0; 32];
        spend[0] = 1;
        let mut view = [0; 32];
        view[0] = 2;
        let spend = PrivateKey::from_slice(&spend).expect("canonical private spend scalar");
        let view = PrivateKey::from_slice(&view).expect("canonical private view scalar");
        MoneroAddress::standard(
            network,
            PublicKey::from_private_key(&spend),
            PublicKey::from_private_key(&view),
        )
    }

    fn fixture() -> (
        MoneroChainIdentity,
        ExpectedMoneroOutput,
        ObservationSnapshot,
    ) {
        let identity =
            MoneroChainIdentity::new(MoneroNetwork::Regtest, [1; 32]).expect("fixture identity");
        let expected = ExpectedMoneroOutput::new(
            hash(2),
            address(AddressNetwork::Mainnet),
            10_000_000_000_000,
        )
        .expect("fixture output");
        let block_header = HeaderSnapshot {
            hash: [3; 32],
            height: 111,
            depth: 9,
            orphan: false,
        };
        let tip = HeaderSnapshot {
            hash: [4; 32],
            height: 120,
            depth: 0,
            orphan: false,
        };
        let subaddress = SubaddressIndex { major: 0, minor: 0 };
        let snapshot = ObservationSnapshot {
            daemon_origin: "http://127.0.0.1:18081".to_owned(),
            wallet_origin: "http://127.0.0.1:18083".to_owned(),
            genesis_hash: [1; 32],
            tip_before: tip.clone(),
            wallet_transfer: Some(WalletTransferSnapshot {
                transaction_id: vec![2; 32],
                destination: expected.destination,
                amount_piconero: expected.amount_piconero.get(),
                confirmations: Some(10),
                double_spend_seen: false,
                height: Some(111),
                incoming: true,
                in_pool: false,
                subaddress,
                unlock_distance: 0,
            }),
            available_outputs: vec![AvailableOutputSnapshot {
                transaction_id: [2; 32],
                amount_piconero: expected.amount_piconero.get(),
                spent: false,
                subaddress,
                block_height: Some(111),
            }],
            daemon_status_ok: true,
            daemon_untrusted: false,
            daemon_missed: 0,
            daemon_transactions: vec![DaemonTransactionSnapshot {
                transaction_id: [2; 32],
                block_height: Some(111),
                double_spend_seen: false,
                in_pool: false,
            }],
            containing_block: Some(BlockSnapshot {
                header: block_header,
                decoded_block_hash: [3; 32],
                transaction_ids: vec![[2; 32]],
            }),
            tip_after: tip,
        };
        (identity, expected, snapshot)
    }

    #[test]
    fn incoming_pool_transfer_is_a_pending_candidate() {
        let (_, expected, _) = fixture();
        let transfer: GotTransfer = serde_json::from_value(serde_json::json!({
            "address": expected.destination.to_string(),
            "amount": expected.amount_piconero.get(),
            "confirmations": 0,
            "double_spend_seen": false,
            "fee": 0,
            "height": 0,
            "note": "",
            "destinations": [],
            "payment_id": "0000000000000000",
            "subaddr_index": {"major": 0, "minor": 0},
            "suggested_confirmations_threshold": 10,
            "timestamp": 1_700_000_000,
            "txid": "02".repeat(32),
            "type": "pool",
            "unlock_time": 0
        }))
        .expect("Monero wallet pool transfer");
        let transfer = WalletTransferSnapshot::from(transfer);

        assert!(
            validate_wallet_transfer_identity(&transfer, &expected).is_ok(),
            "an exact incoming pool transfer is pending, not a wrong-direction transfer"
        );
        assert!(transfer.in_pool);
        assert!(transfer.height.is_none());
    }

    #[tokio::test]
    async fn exact_wallet_available_ten_confirmation_output_is_observed() {
        let (identity, expected, snapshot) = fixture();
        let evidence = verify_with_port(&FixturePort { snapshot }, identity, &expected)
            .await
            .expect("happy-path output evidence");

        assert_eq!(evidence.network(), MoneroNetwork::Regtest);
        assert_eq!(evidence.genesis_hash(), [1; 32]);
        assert_eq!(evidence.daemon_origin(), "http://127.0.0.1:18081");
        assert_eq!(evidence.wallet_origin(), "http://127.0.0.1:18083");
        assert_eq!(evidence.transaction_id(), hash(2));
        assert_eq!(evidence.destination(), expected.destination());
        assert_eq!(evidence.amount_piconero(), 10_000_000_000_000);
        assert_eq!(evidence.containing_block_hash(), [3; 32]);
        assert_eq!(evidence.containing_block_height(), 111);
        assert_eq!(evidence.confirmations(), 10);
        assert_eq!(evidence.stable_tip_hash(), [4; 32]);
        assert_eq!(evidence.stable_tip_height(), 120);
    }

    #[tokio::test]
    async fn exact_output_mismatches_fail_closed() {
        let (identity, expected, snapshot) = fixture();

        let mut wrong_amount = snapshot.clone();
        wrong_amount
            .wallet_transfer
            .as_mut()
            .expect("fixture transfer")
            .amount_piconero += 1;
        assert!(matches!(
            verify_with_port(
                &FixturePort {
                    snapshot: wrong_amount
                },
                identity,
                &expected
            )
            .await,
            Err(MoneroEvidenceError::AmountMismatch)
        ));

        let mut wrong_destination = snapshot.clone();
        wrong_destination
            .wallet_transfer
            .as_mut()
            .expect("fixture transfer")
            .destination = address(AddressNetwork::Mainnet);
        wrong_destination
            .wallet_transfer
            .as_mut()
            .expect("fixture transfer")
            .destination
            .public_spend = wrong_destination
            .wallet_transfer
            .as_ref()
            .expect("fixture transfer")
            .destination
            .public_view;
        assert!(matches!(
            verify_with_port(
                &FixturePort {
                    snapshot: wrong_destination,
                },
                identity,
                &expected,
            )
            .await,
            Err(MoneroEvidenceError::DestinationMismatch)
        ));

        let mut wrong_transaction = snapshot;
        wrong_transaction
            .wallet_transfer
            .as_mut()
            .expect("fixture transfer")
            .transaction_id[0] ^= 1;
        assert!(matches!(
            verify_with_port(
                &FixturePort {
                    snapshot: wrong_transaction,
                },
                identity,
                &expected,
            )
            .await,
            Err(MoneroEvidenceError::TransactionIdMismatch)
        ));
    }

    #[tokio::test]
    async fn pool_double_spend_and_locked_outputs_fail_closed() {
        let (identity, expected, snapshot) = fixture();

        let mut in_pool = snapshot.clone();
        in_pool
            .daemon_transactions
            .first_mut()
            .expect("fixture transaction")
            .in_pool = true;
        assert!(matches!(
            verify_with_port(&FixturePort { snapshot: in_pool }, identity, &expected).await,
            Err(MoneroEvidenceError::DaemonTransactionInPool)
        ));

        let mut double_spend = snapshot.clone();
        double_spend
            .wallet_transfer
            .as_mut()
            .expect("fixture transfer")
            .double_spend_seen = true;
        assert!(matches!(
            verify_with_port(
                &FixturePort {
                    snapshot: double_spend,
                },
                identity,
                &expected,
            )
            .await,
            Err(MoneroEvidenceError::DoubleSpendSeen)
        ));

        let mut locked = snapshot;
        locked.available_outputs.clear();
        assert!(matches!(
            verify_with_port(&FixturePort { snapshot: locked }, identity, &expected).await,
            Err(MoneroEvidenceError::OutputNotUnlocked)
        ));
    }

    #[tokio::test]
    async fn unstable_or_orphaned_chain_and_bad_membership_fail_closed() {
        let (identity, expected, snapshot) = fixture();

        let mut unstable = snapshot.clone();
        unstable.tip_after.hash[0] ^= 1;
        assert!(matches!(
            verify_with_port(&FixturePort { snapshot: unstable }, identity, &expected).await,
            Err(MoneroEvidenceError::UnstableTip)
        ));

        let mut orphan = snapshot.clone();
        orphan
            .containing_block
            .as_mut()
            .expect("fixture block")
            .header
            .orphan = true;
        assert!(matches!(
            verify_with_port(&FixturePort { snapshot: orphan }, identity, &expected).await,
            Err(MoneroEvidenceError::OrphanBlock)
        ));

        let mut absent = snapshot;
        absent
            .containing_block
            .as_mut()
            .expect("fixture block")
            .transaction_ids
            .clear();
        assert!(matches!(
            verify_with_port(&FixturePort { snapshot: absent }, identity, &expected).await,
            Err(MoneroEvidenceError::TransactionMembershipMismatch)
        ));
    }

    #[tokio::test]
    async fn fewer_than_ten_or_inconsistent_confirmations_fail_closed() {
        let (identity, expected, snapshot) = fixture();

        let mut shallow = snapshot.clone();
        shallow.tip_before.height = 119;
        shallow.tip_after.height = 119;
        shallow
            .wallet_transfer
            .as_mut()
            .expect("fixture transfer")
            .confirmations = Some(9);
        shallow
            .containing_block
            .as_mut()
            .expect("fixture block")
            .header
            .depth = 8;
        assert!(matches!(
            verify_with_port(&FixturePort { snapshot: shallow }, identity, &expected).await,
            Err(MoneroEvidenceError::InsufficientConfirmations)
        ));

        let mut inconsistent = snapshot;
        inconsistent
            .wallet_transfer
            .as_mut()
            .expect("fixture transfer")
            .confirmations = Some(11);
        assert!(matches!(
            verify_with_port(
                &FixturePort {
                    snapshot: inconsistent,
                },
                identity,
                &expected,
            )
            .await,
            Err(MoneroEvidenceError::ConfirmationMismatch)
        ));
    }

    #[test]
    fn endpoint_is_literal_loopback_only_and_debug_is_secret_safe() {
        let endpoint = LoopbackRpcEndpoint::new("http://127.0.0.1:18081", "daemon", "super-secret")
            .expect("literal-loopback endpoint");
        assert_eq!(endpoint.base_url(), "http://127.0.0.1:18081");
        let debug = format!("{endpoint:?}");
        assert!(debug.contains("[REDACTED]"));
        assert!(!debug.contains("super-secret"));

        for rejected in [
            "https://127.0.0.1:18081",
            "http://localhost:18081",
            "http://192.0.2.1:18081",
            "http://daemon:secret@127.0.0.1:18081",
            "http://127.0.0.1",
            "http://127.0.0.1:18081/json_rpc",
        ] {
            assert!(LoopbackRpcEndpoint::new(rejected, "daemon", "secret").is_err());
        }

        let identity =
            MoneroChainIdentity::new(MoneroNetwork::Regtest, [1; 32]).expect("chain identity");
        let aliased_daemon = LoopbackRpcEndpoint::new("http://127.0.0.1:18081", "daemon", "secret")
            .expect("daemon endpoint");
        let aliased_wallet = LoopbackRpcEndpoint::new("http://127.0.0.1:18081", "wallet", "secret")
            .expect("wallet endpoint");
        assert!(matches!(
            MoneroOutputVerifier::new(identity, &aliased_daemon, &aliased_wallet),
            Err(MoneroEvidenceError::AliasedRpcOrigins)
        ));
    }

    #[test]
    fn observed_height_zero_identity_is_exact_and_nonzero() {
        let identity = identity_from_observed_hash(MoneroNetwork::Regtest, [7; 32])
            .expect("actual nonzero height-zero identity");
        assert_eq!(identity.network(), MoneroNetwork::Regtest);
        assert_eq!(identity.genesis_hash(), [7; 32]);
        assert!(matches!(
            identity_from_observed_hash(MoneroNetwork::Regtest, [0; 32]),
            Err(MoneroEvidenceError::ZeroObservedGenesisHash)
        ));

        let endpoint = LoopbackRpcEndpoint::new("http://127.0.0.1:18081", "daemon", "secret")
            .expect("literal-loopback authenticated endpoint");
        let attestor = MoneroChainIdentityAttestor::new(MoneroNetwork::Regtest, &endpoint)
            .expect("typed height-zero attestor");
        let debug = format!("{attestor:?}");
        assert!(debug.contains("127.0.0.1:18081"));
        assert!(!debug.contains("secret"));
    }

    #[test]
    fn transport_residual_and_network_domains_are_explicit() {
        assert!(TRANSPORT_RESIDUAL.contains("no pre-decode response-body limit"));
        assert!(TRANSPORT_RESIDUAL.contains("literal-loopback"));
        assert!(AUTHENTICATION_RESIDUAL.contains("wrong-credential rejection"));
        assert!(VIEW_ONLY_SPEND_RESIDUAL.contains("not unspent authority"));
        assert!(HEADER_TRUST_RESIDUAL.contains("untrusted flag"));
        assert!(BLOCK_DECODE_RESIDUAL.contains("can panic"));
        assert_eq!(
            MoneroNetwork::Regtest.address_network(),
            AddressNetwork::Mainnet
        );
        assert_eq!(
            MoneroNetwork::Stagenet.address_network(),
            AddressNetwork::Stagenet
        );
    }
}
