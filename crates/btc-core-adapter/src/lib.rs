//! Typed, fail-closed local Bitcoin Core 31.1 adapter.

mod evidence;
mod http;

pub use evidence::{
    BitcoinCoreEvidenceError, BitcoinCoreEvidenceKind, BitcoinCoreEvidenceV1,
    MAX_BITCOIN_CORE_EVIDENCE_BYTES,
};
pub use http::{HttpBitcoinCoreConfig, HttpBitcoinCoreError, HttpBitcoinCoreRpc};

use std::convert::Infallible;
use std::error::Error as StdError;
use std::str::FromStr as _;

use async_trait::async_trait;
use bitcoin::blockdata::constants::genesis_block;
use bitcoin::consensus::{deserialize, serialize};
use bitcoin::hashes::{Hash as _, sha256};
use bitcoin::{BlockHash, Network, OutPoint, Transaction, Txid, Witness, Wtxid};
use corepc_types::v31::{
    GetBlockHash, GetBlockHeaderVerbose, GetBlockchainInfo, GetIndexInfo, GetNetworkInfo,
    GetRawTransactionVerbose, GetTxSpendingPrevout, SendRawTransaction, TestMempoolAccept,
};
use lez_btc_swap_sdk::BtcAgreementV1;
use thiserror::Error;

/// Exact numeric identity returned by Bitcoin Core 31.1.
pub const BITCOIN_CORE_31_1_VERSION: usize = 310_100;
/// Exact subversion identity returned by an unmodified Bitcoin Core 31.1 node.
pub const BITCOIN_CORE_31_1_SUBVERSION: &str = "/Satoshi:31.1.0/";
/// Largest raw transaction accepted from the local RPC boundary.
pub const MAX_RAW_TRANSACTION_BYTES: usize = 1_000_000;

/// Typed RPC boundary used by the adapter and deterministic tests.
#[async_trait]
pub trait BitcoinCoreRpc: Send + Sync {
    /// Concrete transport error.
    type Error: StdError + Send + Sync + 'static;

    /// Calls `getnetworkinfo` and decodes the Core 31 response type.
    async fn get_network_info(&self) -> Result<GetNetworkInfo, Self::Error>;
    /// Calls `getblockchaininfo` and decodes the Core 31 response type.
    async fn get_blockchain_info(&self) -> Result<GetBlockchainInfo, Self::Error>;
    /// Calls `getblockhash 0` and decodes the Core 31 response type.
    async fn get_genesis_hash(&self) -> Result<GetBlockHash, Self::Error>;
    /// Calls `getindexinfo` and decodes the Core 31 response type.
    async fn get_index_info(&self) -> Result<GetIndexInfo, Self::Error>;
    /// Calls verbose `getrawtransaction` through the exact Core 31 type.
    async fn get_raw_transaction(
        &self,
        transaction_id: Txid,
    ) -> Result<Option<GetRawTransactionVerbose>, Self::Error>;
    /// Calls verbose `getblockheader` through the exact Core 31 type.
    async fn get_block_header(
        &self,
        block_hash: BlockHash,
    ) -> Result<GetBlockHeaderVerbose, Self::Error>;
    /// Calls Core 31 `gettxspendingprevout` with spender bytes requested.
    async fn get_tx_spending_prevout(
        &self,
        outpoint: OutPoint,
    ) -> Result<GetTxSpendingPrevout, Self::Error>;
    /// Calls `testmempoolaccept` once for one exact transaction.
    async fn test_mempool_accept(
        &self,
        transaction: &[u8],
    ) -> Result<TestMempoolAccept, Self::Error>;
    /// Calls `sendrawtransaction` once for one exact transaction.
    async fn send_raw_transaction(
        &self,
        transaction: &[u8],
    ) -> Result<SendRawTransaction, Self::Error>;
    /// Reports a concrete production transport route when the implementation has one.
    ///
    /// Deterministic doubles and alternate trusted ports may leave this unspecified.
    /// The production HTTP transport always reports an exact route, allowing the
    /// adapter to reject a public HTTPS gateway paired with a Regtest profile.
    fn deployment_route(&self) -> Option<CoreRpcRoute> {
        None
    }
    /// Distinguishes a definitive node rejection from an outcome that may have reached Core.
    fn classify_send_failure(error: &Self::Error) -> SendFailure;
}

/// Classification used only after the single broadcast call.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SendFailure {
    /// Core definitively rejected the transaction.
    DefinitiveRejection,
    /// The caller cannot prove whether Core accepted the transaction.
    Unknown,
}

/// Concrete production RPC route category used for profile composition.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CoreRpcRoute {
    /// Literal-loopback HTTP with file-backed Basic credentials.
    LiteralLoopback,
    /// One exact allowlisted HTTPS DNS origin with file-backed Basic credentials.
    ExactHttpsBasic,
}

/// Expected Bitcoin Core P2P connectivity for the selected deployment route.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CoreConnectivityPolicy {
    /// Private Regtest: networking disabled and every connection count exactly zero.
    IsolatedLocal,
    /// Explicit network-enabled Regtest route; peer readiness is external policy.
    Networked,
    /// Public Testnet4 through an explicitly network-enabled Core 31.1 route.
    ///
    /// This never admits legacy Testnet3. Readiness requires Core to report
    /// `chain=testnet4` and the exact Testnet4 genesis pinned by rust-bitcoin.
    Testnet4Networked,
}

impl CoreConnectivityPolicy {
    const fn network(self) -> Network {
        match self {
            Self::IsolatedLocal | Self::Networked => Network::Regtest,
            Self::Testnet4Networked => Network::Testnet4,
        }
    }

    const fn admits_route(self, route: CoreRpcRoute) -> bool {
        matches!(
            (self, route),
            (
                Self::IsolatedLocal | Self::Networked,
                CoreRpcRoute::LiteralLoopback
            ) | (
                Self::Testnet4Networked,
                CoreRpcRoute::LiteralLoopback | CoreRpcRoute::ExactHttpsBasic
            )
        )
    }
}

/// Stable chain position bracketing one observation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StableTip {
    block_hash: BlockHash,
    height: u32,
    median_time_unix_seconds: u64,
}

impl StableTip {
    /// Stable active-chain tip hash.
    #[must_use]
    pub const fn block_hash(&self) -> BlockHash {
        self.block_hash
    }

    /// Stable active-chain tip height.
    #[must_use]
    pub const fn height(&self) -> u32 {
        self.height
    }

    /// Median past time reported for this exact stable active-chain tip.
    #[must_use]
    pub const fn median_time_unix_seconds(&self) -> u64 {
        self.median_time_unix_seconds
    }
}

/// Reconstructed, sufficiently confirmed agreement funding output.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ObservedFunding {
    transaction: Transaction,
    confirmations: u32,
    block_hash: BlockHash,
    block_height: u32,
    block_median_time_unix_seconds: u64,
    stable_tip: StableTip,
}

impl ObservedFunding {
    /// Canonical consensus transaction reconstructed from RPC hex.
    #[must_use]
    pub const fn transaction(&self) -> &Transaction {
        &self.transaction
    }

    /// Active-chain confirmations at the stable observation tip.
    #[must_use]
    pub const fn confirmations(&self) -> u32 {
        self.confirmations
    }

    /// Block containing the funding transaction.
    #[must_use]
    pub const fn block_hash(&self) -> BlockHash {
        self.block_hash
    }

    /// Active-chain height of the block containing the funding transaction.
    #[must_use]
    pub const fn block_height(&self) -> u32 {
        self.block_height
    }

    /// Median past time of the canonical block containing the funding transaction.
    #[must_use]
    pub const fn block_median_time_unix_seconds(&self) -> u64 {
        self.block_median_time_unix_seconds
    }

    /// Stable tip bracketing this observation.
    #[must_use]
    pub const fn stable_tip(&self) -> StableTip {
        self.stable_tip
    }
}

/// Current state of the exact agreement funding output.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FundingObservation {
    /// The transaction is absent from the required index at one stable active-chain tip.
    Absent {
        /// Stable tip bracketing the affirmative absence.
        stable_tip: StableTip,
    },
    /// The exact output exists but has not reached the signed confirmation policy.
    Pending {
        /// Current confirmation count, including zero for mempool funding.
        confirmations: u32,
        /// Stable tip bracketing the pending exact transaction.
        stable_tip: StableTip,
    },
    /// The exact output exists with sufficient active-chain confirmations.
    Ready(ObservedFunding),
}

/// Exact agreement funding state used at a fresh public-effect eligibility boundary.
///
/// Unlike [`FundingObservation`], this type retains pending transaction bytes and
/// distinguishes a confirmed unspent output from a confirmed output that already
/// has a spender. Only [`ExactFundingObservation::Unspent`] grants fresh-action
/// eligibility; every other variant is observe-only.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExactFundingObservation {
    /// The transaction is absent from the required index at one stable active-chain tip.
    Absent {
        /// Stable tip bracketing the affirmative absence.
        stable_tip: StableTip,
    },
    /// The exact output exists but has not reached the signed confirmation policy.
    Pending {
        /// Canonical consensus transaction reconstructed from RPC hex.
        transaction: Transaction,
        /// Current confirmation count, including zero for mempool funding.
        confirmations: u32,
        /// Stable tip bracketing the pending exact transaction.
        stable_tip: StableTip,
    },
    /// The exact output is sufficiently confirmed and currently unspent.
    Unspent(ObservedFunding),
    /// The exact output is sufficiently confirmed but already spent.
    Spent {
        /// Canonical confirmed agreement funding.
        funding: ObservedFunding,
        /// Non-witness identifier of the transaction currently spending the output.
        spender_transaction_id: Txid,
    },
}

/// Exact successful key-path claim observed through the spender index.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ObservedClaim {
    transaction: Transaction,
    transaction_id: Txid,
    confirmations: u32,
    block_hash: Option<BlockHash>,
    stable_tip: StableTip,
}

impl ObservedClaim {
    /// Canonical signed claim transaction.
    #[must_use]
    pub const fn transaction(&self) -> &Transaction {
        &self.transaction
    }

    /// Non-witness transaction identifier of the exact claim.
    #[must_use]
    pub const fn transaction_id(&self) -> Txid {
        self.transaction_id
    }

    /// Active-chain confirmations, or zero while only present in the mempool.
    #[must_use]
    pub const fn confirmations(&self) -> u32 {
        self.confirmations
    }

    /// Active-chain block containing the claim, absent while it is in the mempool.
    #[must_use]
    pub const fn block_hash(&self) -> Option<BlockHash> {
        self.block_hash
    }

    /// Stable tip bracketing this observation.
    #[must_use]
    pub const fn stable_tip(&self) -> StableTip {
        self.stable_tip
    }
}

/// Current spender state of the exact agreement funding outpoint.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ClaimObservation {
    /// No mempool or indexed active-chain spender exists.
    Unspent,
    /// The exact valid claim is visible in the mempool and reveals its witness.
    Revealed(ObservedClaim),
    /// The exact valid claim is active-chain confirmed below the signed policy.
    Confirming(ObservedClaim),
    /// The exact valid claim satisfies the signed active-chain confirmation policy.
    Finalized(ObservedClaim),
}

/// Signed-anchor maturity facts for the exact unilateral refund.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RefundEligibility {
    stable_tip: StableTip,
    funding_block_height: u32,
    first_valid_block_height: u32,
}

impl RefundEligibility {
    /// Stable active-chain tip used for the maturity decision.
    #[must_use]
    pub const fn stable_tip(&self) -> StableTip {
        self.stable_tip
    }

    /// Active-chain height containing the exact agreement funding transaction.
    #[must_use]
    pub const fn funding_block_height(&self) -> u32 {
        self.funding_block_height
    }

    /// First block that may contain the BIP-68 refund.
    #[must_use]
    pub const fn first_valid_block_height(&self) -> u32 {
        self.first_valid_block_height
    }
}

/// Exact agreement refund observed through the spender index.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ObservedRefund {
    transaction: Transaction,
    transaction_id: Txid,
    confirmations: u32,
    block_hash: Option<BlockHash>,
    block_height: Option<u32>,
    stable_tip: StableTip,
}

impl ObservedRefund {
    /// Canonical signed BIP-342 refund transaction.
    #[must_use]
    pub const fn transaction(&self) -> &Transaction {
        &self.transaction
    }

    /// Canonical non-witness transaction identifier.
    #[must_use]
    pub const fn transaction_id(&self) -> Txid {
        self.transaction_id
    }

    /// Active-chain confirmations, or zero while in the mempool.
    #[must_use]
    pub const fn confirmations(&self) -> u32 {
        self.confirmations
    }

    /// Active-chain block hash, absent only for a mempool refund.
    #[must_use]
    pub const fn block_hash(&self) -> Option<BlockHash> {
        self.block_hash
    }

    /// Active-chain containing-block height, absent only for a mempool refund.
    #[must_use]
    pub const fn block_height(&self) -> Option<u32> {
        self.block_height
    }

    /// Stable active-chain tip bracketing the observation.
    #[must_use]
    pub const fn stable_tip(&self) -> StableTip {
        self.stable_tip
    }
}

/// Maturity and spender state of the exact agreement refund path.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RefundObservation {
    /// The funding anchor is exact but the next block is too early for BIP-68.
    Immature(RefundEligibility),
    /// The next block may contain the refund and the outpoint remains unspent.
    Eligible(RefundEligibility),
    /// The exact refund is visible in the mempool.
    Revealed(ObservedRefund),
    /// The exact refund is confirmed below signed policy.
    Confirming(ObservedRefund),
    /// The exact refund satisfies signed confirmation policy.
    Finalized(ObservedRefund),
    /// Another transaction or witness spends the agreement outpoint.
    ConflictingSpend,
}

/// Result of one caller-authorized exact refund submission.
#[derive(Clone, Debug, Eq, PartialEq)]
#[must_use]
pub enum AuthorizedRefundSubmission {
    /// Exact post-send spender bytes prove Core holds the prepared witness.
    Accepted {
        /// Canonical non-witness transaction identifier.
        transaction_id: Txid,
        /// Canonical witness transaction identifier.
        witness_transaction_id: Wtxid,
    },
    /// Core definitively rejected the exact transaction.
    Rejected,
    /// Exact acceptance cannot be proved and no retry is permitted.
    Unknown,
}

/// Result of one caller-authorized exact funding submission.
///
/// The caller must durably consume its single-send authority before invoking
/// [`BitcoinCoreAdapter::submit_authorized_funding`]. `Unknown` is terminal for
/// that authority and must never be retried.
#[derive(Clone, Debug, Eq, PartialEq)]
#[must_use]
pub enum AuthorizedFundingSubmission {
    /// Core returned the expected txid and an exact post-send byte readback succeeded.
    Accepted {
        /// Canonical non-witness transaction identifier.
        transaction_id: Txid,
        /// Canonical witness transaction identifier.
        witness_transaction_id: Wtxid,
    },
    /// Core definitively rejected the exact transaction.
    Rejected,
    /// Exact acceptance cannot be proved and no retry is permitted.
    Unknown,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ParsedSpender {
    transaction_id: Txid,
    transaction_bytes: Vec<u8>,
    block_hash: Option<BlockHash>,
}

enum ExactSubmissionOutcome {
    Accepted {
        transaction_id: Txid,
        witness_transaction_id: Wtxid,
    },
    Rejected,
    Unknown,
}

enum FundingState {
    Absent {
        stable_tip: StableTip,
    },
    Pending {
        transaction: Transaction,
        confirmations: u32,
        stable_tip: StableTip,
    },
    Ready {
        funding: ObservedFunding,
        spender: Option<ParsedSpender>,
    },
}

/// Durable identity of one permitted claim broadcast attempt.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClaimSubmissionAttempt {
    agreement_commitment: [u8; 32],
    transaction_id: Txid,
    witness_transaction_id: Wtxid,
    raw_transaction_digest: [u8; 32],
}

impl ClaimSubmissionAttempt {
    /// Exact agreement commitment owning the attempt.
    #[must_use]
    pub const fn agreement_commitment(&self) -> &[u8; 32] {
        &self.agreement_commitment
    }

    /// Exact claim transaction identifier owning the attempt.
    #[must_use]
    pub const fn transaction_id(&self) -> Txid {
        self.transaction_id
    }

    /// Exact witness transaction identifier owning the attempt.
    #[must_use]
    pub const fn witness_transaction_id(&self) -> Wtxid {
        self.witness_transaction_id
    }

    /// SHA-256 of the exact canonical raw transaction bytes owning the attempt.
    #[must_use]
    pub const fn raw_transaction_digest(&self) -> &[u8; 32] {
        &self.raw_transaction_digest
    }
}

/// Durable state and terminal result of the single claim broadcast attempt.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ClaimSubmissionState {
    /// The CAS was persisted, but no terminal result was durably recorded.
    Started,
    /// Core returned the exact expected transaction identifier.
    Accepted {
        /// Accepted transaction identifier.
        transaction_id: Txid,
    },
    /// Core definitively rejected the transaction.
    Rejected,
    /// The attempt outcome cannot be proven and must never be retried.
    Unknown,
}

/// Chain result of one caller-authorized claim submission.
///
/// This type deliberately has no `Started` state: the caller must consume its
/// durable single-send authority before invoking [`BitcoinCoreAdapter::submit_authorized_claim`]
/// and durably record this result afterwards.
#[derive(Clone, Debug, Eq, PartialEq)]
#[must_use]
pub enum AuthorizedClaimSubmission {
    /// Core returned the exact expected transaction identifier.
    Accepted {
        /// Accepted transaction identifier.
        transaction_id: Txid,
    },
    /// Core definitively rejected the transaction.
    Rejected,
    /// The attempt outcome cannot be proven and must never be retried.
    Unknown,
}

impl From<AuthorizedClaimSubmission> for ClaimSubmissionState {
    fn from(value: AuthorizedClaimSubmission) -> Self {
        match value {
            AuthorizedClaimSubmission::Accepted { transaction_id } => {
                Self::Accepted { transaction_id }
            }
            AuthorizedClaimSubmission::Rejected => Self::Rejected,
            AuthorizedClaimSubmission::Unknown => Self::Unknown,
        }
    }
}

/// Result of the durable compare-and-set boundary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ClaimSubmissionAcquire {
    /// This caller exclusively acquired the single attempt.
    Acquired,
    /// An attempt already exists and no RPC mutation is permitted.
    Existing(ClaimSubmissionState),
}

/// Owner-provided durable compare-and-set port for one-attempt submission.
pub trait ClaimSubmissionStore: Send + Sync {
    /// Concrete persistence error.
    type Error: StdError + Send + Sync + 'static;

    /// Atomically inserts `Started` only when the logical agreement/txid key has no state.
    /// An implementation must compare the complete attempt, including wtxid and raw-byte
    /// digest, and return an error when an existing logical key has different payload bytes.
    ///
    /// # Errors
    ///
    /// Returns the concrete persistence error without permitting an RPC mutation.
    fn compare_and_mark_started(
        &self,
        attempt: ClaimSubmissionAttempt,
    ) -> Result<ClaimSubmissionAcquire, Self::Error>;

    /// Durably replaces `Started` with one terminal result.
    ///
    /// # Errors
    ///
    /// Returns the concrete persistence error; the existing `Started` record still
    /// prevents a later caller from acquiring another attempt.
    fn record_result(
        &self,
        attempt: &ClaimSubmissionAttempt,
        result: ClaimSubmissionState,
    ) -> Result<(), Self::Error>;
}

/// Fail-closed typed Core, consensus, agreement, or durability error.
#[derive(Debug, Error)]
pub enum CoreAdapterError<RpcError: StdError + 'static, StoreError: StdError + 'static = Infallible>
{
    /// Typed RPC transport or remote call failed.
    #[error("Bitcoin Core RPC request failed")]
    Rpc(#[source] RpcError),
    /// Durable submission state failed before or after the single attempt.
    #[error("claim submission state persistence failed")]
    Store(#[source] StoreError),
    /// Node version or subversion is not exact Core 31.1.
    #[error("node is not exact Bitcoin Core 31.1")]
    WrongCoreVersion,
    /// Node P2P connectivity contradicts the explicitly selected deployment route.
    #[error("Bitcoin Core connectivity contradicts the configured deployment policy")]
    ConnectivityPolicyMismatch,
    /// Node is not ready and unpruned on the selected exact chain profile.
    #[error("Bitcoin Core active chain is not ready")]
    ChainNotReady,
    /// Node genesis differs from the countersigned agreement.
    #[error("Bitcoin Core genesis differs from agreement")]
    BitcoinGenesisMismatch,
    /// One required index is absent, unsynced, or at a different height.
    #[error("required Bitcoin Core index is not ready: {0}")]
    RequiredIndexNotReady(&'static str),
    /// Core returned a malformed hash or numeric field.
    #[error("Bitcoin Core returned malformed {0}")]
    MalformedResponse(&'static str),
    /// Raw transaction is malformed, non-canonical, or over the explicit bound.
    #[error("Bitcoin Core returned malformed raw transaction")]
    MalformedRawTransaction,
    /// Core raw transaction identity or metric fields disagree with consensus bytes.
    #[error("Bitcoin Core raw transaction identity metrics disagree with consensus bytes")]
    RawTransactionMetricsMismatch,
    /// Funding transaction does not contain the exact agreement output.
    #[error("observed funding output differs from agreement")]
    FundingOutputMismatch,
    /// Funding transaction identity differs from the signed agreement or caller expectation.
    #[error("funding transaction identity differs from agreement")]
    FundingTransactionMismatch,
    /// Confirmation context is partial or invalid.
    #[error("Bitcoin Core returned invalid transaction confirmation context")]
    InvalidConfirmationContext,
    /// Active chain tip changed during an observation.
    #[error("Bitcoin Core tip changed during observation")]
    UnstableTip,
    /// Spender response does not identify the exact requested outpoint and bytes.
    #[error("Bitcoin Core spender response is inconsistent")]
    SpenderResponseMismatch,
    /// Spending transaction differs from the exact signed agreement claim.
    #[error("observed spender is not the exact agreement claim")]
    ClaimTransactionMismatch,
    /// Confirmed funding height lies below the countersigned recovery anchor.
    #[error("observed funding height lies below the signed recovery anchor")]
    FundingAnchorMismatch,
    /// Spending transaction differs from the exact signed agreement refund.
    #[error("observed spender is not the exact agreement refund")]
    RefundTransactionMismatch,
    /// Mempool preflight response is malformed or identifies another transaction.
    #[error("Bitcoin Core mempool preflight response is inconsistent")]
    MempoolResponseMismatch,
    /// Broadcast success returned a different transaction identifier.
    #[error("Bitcoin Core broadcast returned a different transaction identifier")]
    BroadcastIdentityMismatch,
}

/// Agreement-aware typed Bitcoin Core adapter.
#[derive(Clone, Debug)]
pub struct BitcoinCoreAdapter<R> {
    rpc: R,
    connectivity: CoreConnectivityPolicy,
}

impl<R> BitcoinCoreAdapter<R>
where
    R: BitcoinCoreRpc,
{
    /// Wraps one typed RPC transport.
    #[must_use]
    pub const fn new(rpc: R, connectivity: CoreConnectivityPolicy) -> Self {
        Self { rpc, connectivity }
    }

    /// Validates the concrete transport route against the exact chain profile.
    ///
    /// This is side-effect free. Production HTTP transports always declare their
    /// route. Exact HTTPS is admitted only for Testnet4; literal loopback remains
    /// valid for existing Regtest and self-hosted Testnet4 nodes.
    ///
    /// # Errors
    ///
    /// Rejects an exact HTTPS route paired with either Regtest profile.
    pub fn ensure_route_compatible(&self) -> Result<(), CoreAdapterError<R::Error>> {
        if self
            .rpc
            .deployment_route()
            .is_some_and(|route| !self.connectivity.admits_route(route))
        {
            return Err(CoreAdapterError::ConnectivityPolicyMismatch);
        }
        Ok(())
    }

    /// Requires exact Core 31.1, profile-pinned genesis, ready chain, and synced indexes.
    ///
    /// # Errors
    ///
    /// Rejects every identity, readiness, response, or RPC mismatch.
    pub async fn ensure_ready(
        &self,
        agreement: &BtcAgreementV1,
    ) -> Result<StableTip, CoreAdapterError<R::Error>> {
        self.ensure_route_compatible()?;
        let network = self
            .rpc
            .get_network_info()
            .await
            .map_err(CoreAdapterError::Rpc)?;
        if network.version != BITCOIN_CORE_31_1_VERSION
            || network.subversion != BITCOIN_CORE_31_1_SUBVERSION
        {
            return Err(CoreAdapterError::WrongCoreVersion);
        }
        let connectivity_matches = match self.connectivity {
            CoreConnectivityPolicy::IsolatedLocal => {
                !network.network_active
                    && network.connections == 0
                    && network.connections_in == 0
                    && network.connections_out == 0
            }
            CoreConnectivityPolicy::Networked | CoreConnectivityPolicy::Testnet4Networked => {
                network.network_active
            }
        };
        if !connectivity_matches {
            return Err(CoreAdapterError::ConnectivityPolicyMismatch);
        }
        let chain = self
            .rpc
            .get_blockchain_info()
            .await
            .map_err(CoreAdapterError::Rpc)?;
        let profile_network = self.connectivity.network();
        let tip = parse_ready_chain(&chain, profile_network)?;
        let genesis = self
            .rpc
            .get_genesis_hash()
            .await
            .map_err(CoreAdapterError::Rpc)?;
        let observed_genesis = parse_block_hash(&genesis.0, "genesis hash")?;
        let agreement_genesis = BlockHash::from_byte_array(*agreement.bitcoin_genesis_hash());
        let profile_genesis = genesis_block(profile_network).block_hash();
        if agreement_genesis != profile_genesis || observed_genesis != profile_genesis {
            return Err(CoreAdapterError::BitcoinGenesisMismatch);
        }
        let indexes = self
            .rpc
            .get_index_info()
            .await
            .map_err(CoreAdapterError::Rpc)?;
        require_index(&indexes, "txindex", tip.height)?;
        require_index(&indexes, "txospenderindex", tip.height)?;
        Ok(tip)
    }

    /// Observes the exact funding transaction under a stable-tip bracket.
    ///
    /// # Errors
    ///
    /// Rejects readiness, raw consensus, RPC metric, agreement output, confirmation,
    /// or stable-tip mismatches.
    pub async fn observe_funding(
        &self,
        agreement: &BtcAgreementV1,
    ) -> Result<FundingObservation, CoreAdapterError<R::Error>> {
        Ok(match self.observe_funding_state(agreement, false).await? {
            FundingState::Absent { stable_tip } => FundingObservation::Absent { stable_tip },
            FundingState::Pending {
                confirmations,
                stable_tip,
                ..
            } => FundingObservation::Pending {
                confirmations,
                stable_tip,
            },
            FundingState::Ready { funding, .. } => FundingObservation::Ready(funding),
        })
    }

    /// Observes exact agreement funding and its current spender state under one
    /// stable-tip bracket.
    ///
    /// Pending bytes are retained so callers can distinguish exact plan presence
    /// from a conflicting transaction without granting fresh-send authority.
    /// Confirmed funding is `Unspent` only after `gettxspendingprevout` proves no
    /// spender at the same stable active-chain tip.
    ///
    /// # Errors
    ///
    /// Rejects readiness, raw consensus, RPC metric, agreement output,
    /// confirmation, spender-index, or stable-tip mismatches.
    pub async fn observe_exact_funding(
        &self,
        agreement: &BtcAgreementV1,
    ) -> Result<ExactFundingObservation, CoreAdapterError<R::Error>> {
        Ok(match self.observe_funding_state(agreement, true).await? {
            FundingState::Absent { stable_tip } => ExactFundingObservation::Absent { stable_tip },
            FundingState::Pending {
                transaction,
                confirmations,
                stable_tip,
            } => ExactFundingObservation::Pending {
                transaction,
                confirmations,
                stable_tip,
            },
            FundingState::Ready {
                funding,
                spender: None,
            } => ExactFundingObservation::Unspent(funding),
            FundingState::Ready {
                funding,
                spender: Some(spender),
            } => ExactFundingObservation::Spent {
                funding,
                spender_transaction_id: spender.transaction_id,
            },
        })
    }

    async fn observe_funding_state(
        &self,
        agreement: &BtcAgreementV1,
        inspect_spender: bool,
    ) -> Result<FundingState, CoreAdapterError<R::Error>> {
        let before = self.ensure_ready(agreement).await?;
        let funding = agreement.funding_terms();
        let expected_txid = Txid::from_byte_array(*funding.transaction_id());
        let response = self
            .rpc
            .get_raw_transaction(expected_txid)
            .await
            .map_err(CoreAdapterError::Rpc)?;
        let Some(response) = response else {
            let after = self.current_ready_tip().await?;
            if before != after {
                return Err(CoreAdapterError::UnstableTip);
            }
            return Ok(FundingState::Absent { stable_tip: after });
        };
        let transaction = parse_verbose_transaction(&response, expected_txid)?;
        let output = transaction
            .output
            .get(
                usize::try_from(funding.output_index())
                    .map_err(|_| CoreAdapterError::FundingOutputMismatch)?,
            )
            .ok_or(CoreAdapterError::FundingOutputMismatch)?;
        if output.value.to_sat() != funding.value_sat()
            || output.script_pubkey.as_bytes() != agreement.p2tr_contract().script_pubkey_bytes()
        {
            return Err(CoreAdapterError::FundingOutputMismatch);
        }
        let (confirmations, block_hash) = confirmation_context(&response)?;
        if confirmations < agreement.required_bitcoin_confirmations() {
            let after = self.current_ready_tip().await?;
            if before != after {
                return Err(CoreAdapterError::UnstableTip);
            }
            return Ok(FundingState::Pending {
                transaction,
                confirmations,
                stable_tip: after,
            });
        }
        let block_hash = block_hash.ok_or(CoreAdapterError::InvalidConfirmationContext)?;
        let header = self
            .rpc
            .get_block_header(block_hash)
            .await
            .map_err(CoreAdapterError::Rpc)?;
        let spender = if inspect_spender {
            let outpoint = OutPoint {
                txid: expected_txid,
                vout: funding.output_index(),
            };
            let response = self
                .rpc
                .get_tx_spending_prevout(outpoint)
                .await
                .map_err(CoreAdapterError::Rpc)?;
            let spender = parse_spender_response(&response, outpoint)?;
            if let Some(spender) = &spender {
                validate_funding_spender(spender, outpoint)?;
            }
            spender
        } else {
            None
        };
        let after = self.current_ready_tip().await?;
        if before != after {
            return Err(CoreAdapterError::UnstableTip);
        }
        let (block_height, block_median_time_unix_seconds) =
            validate_funding_block_header(&header, block_hash, confirmations, after)?;
        Ok(FundingState::Ready {
            funding: ObservedFunding {
                transaction,
                confirmations,
                block_hash,
                block_height,
                block_median_time_unix_seconds,
                stable_tip: after,
            },
            spender,
        })
    }

    /// Observes and validates the exact successful claim spender under a stable tip.
    ///
    /// # Errors
    ///
    /// Rejects readiness, spender-index, raw consensus, witness, agreement, or tip drift.
    pub async fn observe_claim(
        &self,
        agreement: &BtcAgreementV1,
    ) -> Result<ClaimObservation, CoreAdapterError<R::Error>> {
        let before = self.ensure_ready(agreement).await?;
        let outpoint = agreement.cooperative_claim().funding_outpoint();
        let response = self
            .rpc
            .get_tx_spending_prevout(outpoint)
            .await
            .map_err(CoreAdapterError::Rpc)?;
        let [item] = response.0.as_slice() else {
            return Err(CoreAdapterError::SpenderResponseMismatch);
        };
        let response_txid =
            Txid::from_str(&item.txid).map_err(|_| CoreAdapterError::SpenderResponseMismatch)?;
        if response_txid != outpoint.txid || item.vout != outpoint.vout {
            return Err(CoreAdapterError::SpenderResponseMismatch);
        }
        let Some(spending_txid_text) = &item.spending_txid else {
            if item.spending_tx.is_some() || item.block_hash.is_some() {
                return Err(CoreAdapterError::SpenderResponseMismatch);
            }
            let after = self.current_ready_tip().await?;
            if before != after {
                return Err(CoreAdapterError::UnstableTip);
            }
            return Ok(ClaimObservation::Unspent);
        };
        let spending_txid = Txid::from_str(spending_txid_text)
            .map_err(|_| CoreAdapterError::SpenderResponseMismatch)?;
        let spender_bytes = decode_raw_hex(
            item.spending_tx
                .as_deref()
                .ok_or(CoreAdapterError::SpenderResponseMismatch)?,
        )?;
        let response = self
            .rpc
            .get_raw_transaction(spending_txid)
            .await
            .map_err(CoreAdapterError::Rpc)?
            .ok_or(CoreAdapterError::SpenderResponseMismatch)?;
        let transaction = parse_verbose_transaction(&response, spending_txid)?;
        if serialize(&transaction) != spender_bytes {
            return Err(CoreAdapterError::SpenderResponseMismatch);
        }
        let (confirmations, block_hash) = confirmation_context(&response)?;
        let spender_block_hash = item
            .block_hash
            .as_deref()
            .map(|value| parse_block_hash(value, "spender block hash"))
            .transpose()?;
        if spender_block_hash != block_hash {
            return Err(CoreAdapterError::SpenderResponseMismatch);
        }
        validate_exact_claim(agreement, &transaction)?;
        let after = self.current_ready_tip().await?;
        if before != after {
            return Err(CoreAdapterError::UnstableTip);
        }
        let observed = ObservedClaim {
            transaction,
            transaction_id: spending_txid,
            confirmations,
            block_hash,
            stable_tip: after,
        };
        if confirmations == 0 {
            Ok(ClaimObservation::Revealed(observed))
        } else if confirmations < agreement.required_bitcoin_confirmations() {
            Ok(ClaimObservation::Confirming(observed))
        } else {
            Ok(ClaimObservation::Finalized(observed))
        }
    }

    /// Observes signed Bitcoin refund maturity and the exact outpoint spender.
    ///
    /// The funding containing height is derived from a stable tip and active-chain
    /// confirmations, then required to equal the countersigned recovery anchor.
    /// BIP-68 admission is eligible when the next block can be the signed refund
    /// height. A finalized result always records the refund containing-block height.
    ///
    /// # Errors
    ///
    /// Rejects readiness, funding-anchor, spender, consensus-byte, placement, or
    /// stable-tip contradictions.
    pub async fn observe_refund(
        &self,
        agreement: &BtcAgreementV1,
    ) -> Result<RefundObservation, CoreAdapterError<R::Error>> {
        let before = self.ensure_ready(agreement).await?;
        let eligibility = self.refund_eligibility(agreement, before).await?;
        self.observe_refund_spender(agreement, before, eligibility)
            .await
    }

    async fn refund_eligibility(
        &self,
        agreement: &BtcAgreementV1,
        stable_tip: StableTip,
    ) -> Result<RefundEligibility, CoreAdapterError<R::Error>> {
        let funding = agreement.funding_terms();
        let funding_txid = Txid::from_byte_array(*funding.transaction_id());
        let response = self
            .rpc
            .get_raw_transaction(funding_txid)
            .await
            .map_err(CoreAdapterError::Rpc)?
            .ok_or(CoreAdapterError::FundingOutputMismatch)?;
        let transaction = parse_verbose_transaction(&response, funding_txid)?;
        let output = transaction
            .output
            .get(
                usize::try_from(funding.output_index())
                    .map_err(|_| CoreAdapterError::FundingOutputMismatch)?,
            )
            .ok_or(CoreAdapterError::FundingOutputMismatch)?;
        if output.value.to_sat() != funding.value_sat()
            || output.script_pubkey.as_bytes() != agreement.p2tr_contract().script_pubkey_bytes()
        {
            return Err(CoreAdapterError::FundingOutputMismatch);
        }
        let (confirmations, block_hash) = confirmation_context(&response)?;
        if confirmations == 0 || block_hash.is_none() {
            return Err(CoreAdapterError::InvalidConfirmationContext);
        }
        let funding_block_height = stable_tip
            .height
            .checked_add(1)
            .and_then(|height| height.checked_sub(confirmations))
            .ok_or(CoreAdapterError::InvalidConfirmationContext)?;
        let recovery = agreement.body().recovery_plan();
        // The signed anchor is the tip when the funding was planned; the lock is
        // broadcast later and mined whenever the next block comes, so the
        // funding can only confirm above the anchor. BIP-68 counts from the
        // block that actually holds the funding, so a later confirmation only
        // delays the refund and never brings it forward. Below the anchor is
        // impossible for the planned transaction and stays a mismatch.
        let anchor_height = recovery.bitcoin_funding_anchor_height();
        if funding_block_height < anchor_height {
            return Err(CoreAdapterError::FundingAnchorMismatch);
        }
        let csv_delay = recovery
            .bitcoin_refund_height()
            .checked_sub(anchor_height)
            .ok_or(CoreAdapterError::InvalidConfirmationContext)?;
        let first_valid_block_height = funding_block_height
            .checked_add(csv_delay)
            .ok_or(CoreAdapterError::InvalidConfirmationContext)?;
        Ok(RefundEligibility {
            stable_tip,
            funding_block_height,
            first_valid_block_height,
        })
    }

    async fn observe_refund_spender(
        &self,
        agreement: &BtcAgreementV1,
        before: StableTip,
        eligibility: RefundEligibility,
    ) -> Result<RefundObservation, CoreAdapterError<R::Error>> {
        let outpoint = agreement.bitcoin_refund().funding_outpoint();
        let response = self
            .rpc
            .get_tx_spending_prevout(outpoint)
            .await
            .map_err(CoreAdapterError::Rpc)?;
        let Some(spender) = parse_spender_response(&response, outpoint)? else {
            let after = self.current_ready_tip().await?;
            if before != after {
                return Err(CoreAdapterError::UnstableTip);
            }
            return Ok(
                if before.height.saturating_add(1) >= eligibility.first_valid_block_height {
                    RefundObservation::Eligible(eligibility)
                } else {
                    RefundObservation::Immature(eligibility)
                },
            );
        };
        let response = self
            .rpc
            .get_raw_transaction(spender.transaction_id)
            .await
            .map_err(CoreAdapterError::Rpc)?
            .ok_or(CoreAdapterError::SpenderResponseMismatch)?;
        let transaction = parse_verbose_transaction(&response, spender.transaction_id)?;
        if serialize(&transaction) != spender.transaction_bytes {
            return Err(CoreAdapterError::SpenderResponseMismatch);
        }
        let (confirmations, block_hash) = confirmation_context(&response)?;
        if spender.block_hash != block_hash {
            return Err(CoreAdapterError::SpenderResponseMismatch);
        }
        let after = self.current_ready_tip().await?;
        if before != after {
            return Err(CoreAdapterError::UnstableTip);
        }
        if validate_exact_refund::<R::Error>(agreement, &transaction).is_err() {
            return Ok(RefundObservation::ConflictingSpend);
        }
        let block_height = refund_block_height(agreement, after, confirmations)?;
        let observed = ObservedRefund {
            transaction,
            transaction_id: spender.transaction_id,
            confirmations,
            block_hash,
            block_height,
            stable_tip: after,
        };
        Ok(classify_refund(
            observed,
            agreement.required_bitcoin_confirmations(),
        ))
    }

    /// Submits one exact claim after the caller has durably consumed send authority.
    ///
    /// This method does not acquire, persist, or retry submission authority. The
    /// caller must first durably bind the agreement, `expected_transaction_id`,
    /// and complete witness-bearing `transaction_bytes` to a single attempt. It
    /// must treat `Unknown` and every error after authority consumption as
    /// observe-only. One invocation makes at most one `sendrawtransaction` call.
    ///
    /// The canonical bytes and exact agreement claim are validated before any
    /// RPC. Core's mempool response must identify both the expected txid and the
    /// wtxid computed from the exact bytes; broadcast success must return the
    /// expected txid. Already-known and same-nonwitness-data results remain
    /// `Unknown`, because neither proves that Core holds this exact witness.
    ///
    /// # Errors
    ///
    /// Rejects malformed or non-canonical bytes, an agreement or expected txid
    /// mismatch, readiness failure, or a contradictory typed Core response.
    pub async fn submit_authorized_claim(
        &self,
        agreement: &BtcAgreementV1,
        transaction_bytes: &[u8],
        expected_transaction_id: Txid,
    ) -> Result<AuthorizedClaimSubmission, CoreAdapterError<R::Error>> {
        let transaction = validate_claim_submission(agreement, transaction_bytes)?;
        if transaction.compute_txid() != expected_transaction_id {
            return Err(CoreAdapterError::ClaimTransactionMismatch);
        }
        self.perform_claim_submission(agreement, &transaction, transaction_bytes)
            .await
    }

    /// Submits one exact agreement funding transaction after the actor journal
    /// has durably consumed send authority.
    ///
    /// Local consensus decoding, exact agreement output, and caller-provided
    /// txid are checked before any RPC. One invocation makes at most one
    /// `sendrawtransaction` call. A successful send becomes `Accepted` only
    /// after `getrawtransaction` returns the same canonical bytes; any ambiguous
    /// response is terminal `Unknown` and cannot authorize a retry.
    ///
    /// # Errors
    ///
    /// Rejects malformed or non-canonical bytes, agreement/identity drift,
    /// readiness failure, or a contradictory typed Core response.
    pub async fn submit_authorized_funding(
        &self,
        agreement: &BtcAgreementV1,
        transaction_bytes: &[u8],
        expected_transaction_id: Txid,
    ) -> Result<AuthorizedFundingSubmission, CoreAdapterError<R::Error>> {
        let transaction = validate_funding_submission(agreement, transaction_bytes)?;
        if transaction.compute_txid() != expected_transaction_id {
            return Err(CoreAdapterError::FundingTransactionMismatch);
        }
        Ok(
            match self
                .perform_single_broadcast(agreement, &transaction, transaction_bytes)
                .await?
            {
                ExactSubmissionOutcome::Accepted {
                    transaction_id,
                    witness_transaction_id,
                } => {
                    if self
                        .post_send_exact_funding(transaction_bytes, transaction_id)
                        .await
                    {
                        AuthorizedFundingSubmission::Accepted {
                            transaction_id,
                            witness_transaction_id,
                        }
                    } else {
                        AuthorizedFundingSubmission::Unknown
                    }
                }
                ExactSubmissionOutcome::Rejected => AuthorizedFundingSubmission::Rejected,
                ExactSubmissionOutcome::Unknown => AuthorizedFundingSubmission::Unknown,
            },
        )
    }

    /// Submits one exact BIP-342 refund after the actor journal has durably
    /// consumed send authority. A successful txid response is accepted only
    /// when a post-send spender read returns the same complete witness bytes.
    ///
    /// # Errors
    ///
    /// Rejects malformed or non-canonical bytes, agreement/identity drift,
    /// readiness failure, or a contradictory typed Core response.
    pub async fn submit_authorized_refund(
        &self,
        agreement: &BtcAgreementV1,
        transaction_bytes: &[u8],
        expected_transaction_id: Txid,
    ) -> Result<AuthorizedRefundSubmission, CoreAdapterError<R::Error>> {
        let transaction = decode_raw_transaction(transaction_bytes)?;
        validate_exact_refund(agreement, &transaction)?;
        if transaction.compute_txid() != expected_transaction_id {
            return Err(CoreAdapterError::RefundTransactionMismatch);
        }
        Ok(
            match self
                .perform_exact_submission(
                    agreement,
                    &transaction,
                    transaction_bytes,
                    agreement.bitcoin_refund().funding_outpoint(),
                )
                .await?
            {
                ExactSubmissionOutcome::Accepted {
                    transaction_id,
                    witness_transaction_id,
                } => AuthorizedRefundSubmission::Accepted {
                    transaction_id,
                    witness_transaction_id,
                },
                ExactSubmissionOutcome::Rejected => AuthorizedRefundSubmission::Rejected,
                ExactSubmissionOutcome::Unknown => AuthorizedRefundSubmission::Unknown,
            },
        )
    }

    /// Executes at most one durable claim submission attempt.
    ///
    /// The exact signed claim is validated locally before the durable CAS. Once
    /// acquired, every inability to prove a terminal result becomes `Unknown`.
    /// A repeated call returns durable state without another RPC mutation.
    ///
    /// # Errors
    ///
    /// Rejects an invalid claim, malformed typed response, readiness failure,
    /// mismatched broadcast identity, or durability error.
    pub async fn submit_claim<S>(
        &self,
        agreement: &BtcAgreementV1,
        transaction_bytes: &[u8],
        store: &S,
    ) -> Result<ClaimSubmissionState, CoreAdapterError<R::Error, S::Error>>
    where
        S: ClaimSubmissionStore,
    {
        let transaction = validate_claim_submission::<R::Error>(agreement, transaction_bytes)
            .map_err(widen_core_error::<R::Error, S::Error>)?;
        let transaction_id = transaction.compute_txid();
        let attempt = ClaimSubmissionAttempt {
            agreement_commitment: *agreement.agreement_commitment(),
            transaction_id,
            witness_transaction_id: transaction.compute_wtxid(),
            raw_transaction_digest: sha256::Hash::hash(transaction_bytes).to_byte_array(),
        };
        match store
            .compare_and_mark_started(attempt.clone())
            .map_err(CoreAdapterError::Store)?
        {
            ClaimSubmissionAcquire::Existing(state) => return Ok(state),
            ClaimSubmissionAcquire::Acquired => {}
        }
        let state = match self
            .perform_claim_submission(agreement, &transaction, transaction_bytes)
            .await
        {
            Ok(state) => ClaimSubmissionState::from(state),
            Err(error) => {
                record_result(store, &attempt, ClaimSubmissionState::Unknown)?;
                return Err(widen_core_error(error));
            }
        };
        record_result(store, &attempt, state.clone())?;
        Ok(state)
    }

    async fn perform_claim_submission(
        &self,
        agreement: &BtcAgreementV1,
        transaction: &Transaction,
        transaction_bytes: &[u8],
    ) -> Result<AuthorizedClaimSubmission, CoreAdapterError<R::Error>> {
        Ok(
            match self
                .perform_exact_submission(
                    agreement,
                    transaction,
                    transaction_bytes,
                    agreement.cooperative_claim().funding_outpoint(),
                )
                .await?
            {
                ExactSubmissionOutcome::Accepted { transaction_id, .. } => {
                    AuthorizedClaimSubmission::Accepted { transaction_id }
                }
                ExactSubmissionOutcome::Rejected => AuthorizedClaimSubmission::Rejected,
                ExactSubmissionOutcome::Unknown => AuthorizedClaimSubmission::Unknown,
            },
        )
    }

    async fn perform_exact_submission(
        &self,
        agreement: &BtcAgreementV1,
        transaction: &Transaction,
        transaction_bytes: &[u8],
        outpoint: OutPoint,
    ) -> Result<ExactSubmissionOutcome, CoreAdapterError<R::Error>> {
        match self
            .perform_single_broadcast(agreement, transaction, transaction_bytes)
            .await?
        {
            ExactSubmissionOutcome::Accepted {
                transaction_id,
                witness_transaction_id,
            } => {
                if self
                    .post_send_exact_spender(outpoint, transaction_bytes, transaction_id)
                    .await
                {
                    Ok(ExactSubmissionOutcome::Accepted {
                        transaction_id,
                        witness_transaction_id,
                    })
                } else {
                    Ok(ExactSubmissionOutcome::Unknown)
                }
            }
            outcome => Ok(outcome),
        }
    }

    async fn perform_single_broadcast(
        &self,
        agreement: &BtcAgreementV1,
        transaction: &Transaction,
        transaction_bytes: &[u8],
    ) -> Result<ExactSubmissionOutcome, CoreAdapterError<R::Error>> {
        self.ensure_ready(agreement).await?;
        let transaction_id = transaction.compute_txid();
        let witness_transaction_id = transaction.compute_wtxid();
        let Ok(mempool) = self.rpc.test_mempool_accept(transaction_bytes).await else {
            return Ok(ExactSubmissionOutcome::Unknown);
        };
        let [acceptance] = mempool.0.as_slice() else {
            return Err(CoreAdapterError::MempoolResponseMismatch);
        };
        let accepted_transaction_id = Txid::from_str(&acceptance.txid)
            .map_err(|_| CoreAdapterError::MempoolResponseMismatch)?;
        let accepted_witness_id = Wtxid::from_str(&acceptance.wtxid)
            .map_err(|_| CoreAdapterError::MempoolResponseMismatch)?;
        if accepted_transaction_id != transaction_id
            || accepted_witness_id != witness_transaction_id
        {
            return Err(CoreAdapterError::MempoolResponseMismatch);
        }
        let expected_vsize = i64::try_from(transaction.vsize())
            .map_err(|_| CoreAdapterError::MempoolResponseMismatch)?;
        if acceptance.allowed {
            if acceptance.vsize != Some(expected_vsize)
                || acceptance.reject_reason.is_some()
                || acceptance.reject_details.is_some()
            {
                return Err(CoreAdapterError::MempoolResponseMismatch);
            }
        } else {
            let reason = acceptance
                .reject_reason
                .as_deref()
                .filter(|reason| !reason.is_empty())
                .ok_or(CoreAdapterError::MempoolResponseMismatch)?;
            if acceptance.vsize.is_some() || acceptance.fees.is_some() {
                return Err(CoreAdapterError::MempoolResponseMismatch);
            }
            return Ok(
                if matches!(
                    reason,
                    "txn-already-in-mempool" | "txn-same-nonwitness-data-in-mempool"
                ) {
                    ExactSubmissionOutcome::Unknown
                } else {
                    ExactSubmissionOutcome::Rejected
                },
            );
        }
        match self.rpc.send_raw_transaction(transaction_bytes).await {
            Ok(response) => {
                let response_txid = Txid::from_str(&response.0)
                    .map_err(|_| CoreAdapterError::BroadcastIdentityMismatch)?;
                if response_txid != transaction_id {
                    return Err(CoreAdapterError::BroadcastIdentityMismatch);
                }
                Ok(ExactSubmissionOutcome::Accepted {
                    transaction_id,
                    witness_transaction_id,
                })
            }
            Err(error) => Ok(match R::classify_send_failure(&error) {
                SendFailure::DefinitiveRejection => ExactSubmissionOutcome::Rejected,
                SendFailure::Unknown => ExactSubmissionOutcome::Unknown,
            }),
        }
    }

    async fn post_send_exact_funding(
        &self,
        transaction_bytes: &[u8],
        transaction_id: Txid,
    ) -> bool {
        let Ok(Some(response)) = self.rpc.get_raw_transaction(transaction_id).await else {
            return false;
        };
        parse_verbose_transaction::<R::Error>(&response, transaction_id).is_ok_and(|transaction| {
            transaction.compute_txid() == transaction_id
                && serialize(&transaction) == transaction_bytes
        })
    }

    async fn post_send_exact_spender(
        &self,
        outpoint: OutPoint,
        transaction_bytes: &[u8],
        transaction_id: Txid,
    ) -> bool {
        let Ok(response) = self.rpc.get_tx_spending_prevout(outpoint).await else {
            return false;
        };
        let [item] = response.0.as_slice() else {
            return false;
        };
        let Ok(response_outpoint_txid) = Txid::from_str(&item.txid) else {
            return false;
        };
        let Some(spending_txid) = item
            .spending_txid
            .as_deref()
            .and_then(|value| Txid::from_str(value).ok())
        else {
            return false;
        };
        let Some(spending_bytes) = item
            .spending_tx
            .as_deref()
            .and_then(|value| decode_raw_hex::<R::Error>(value).ok())
        else {
            return false;
        };
        response_outpoint_txid == outpoint.txid
            && item.vout == outpoint.vout
            && spending_txid == transaction_id
            && spending_bytes == transaction_bytes
            && decode_raw_transaction::<R::Error>(&spending_bytes).is_ok_and(|transaction| {
                transaction.compute_txid() == transaction_id
                    && serialize(&transaction) == transaction_bytes
            })
    }

    async fn current_ready_tip(&self) -> Result<StableTip, CoreAdapterError<R::Error>> {
        let chain = self
            .rpc
            .get_blockchain_info()
            .await
            .map_err(CoreAdapterError::Rpc)?;
        parse_ready_chain(&chain, self.connectivity.network())
    }
}

fn parse_spender_response<R>(
    response: &GetTxSpendingPrevout,
    outpoint: OutPoint,
) -> Result<Option<ParsedSpender>, CoreAdapterError<R>>
where
    R: StdError + 'static,
{
    let [item] = response.0.as_slice() else {
        return Err(CoreAdapterError::SpenderResponseMismatch);
    };
    let response_txid =
        Txid::from_str(&item.txid).map_err(|_| CoreAdapterError::SpenderResponseMismatch)?;
    if response_txid != outpoint.txid || item.vout != outpoint.vout {
        return Err(CoreAdapterError::SpenderResponseMismatch);
    }
    let Some(spending_txid) = item.spending_txid.as_deref() else {
        if item.spending_tx.is_some() || item.block_hash.is_some() {
            return Err(CoreAdapterError::SpenderResponseMismatch);
        }
        return Ok(None);
    };
    let spending_txid =
        Txid::from_str(spending_txid).map_err(|_| CoreAdapterError::SpenderResponseMismatch)?;
    let spending_bytes = decode_raw_hex(
        item.spending_tx
            .as_deref()
            .ok_or(CoreAdapterError::SpenderResponseMismatch)?,
    )?;
    let block_hash = item
        .block_hash
        .as_deref()
        .map(|value| parse_block_hash(value, "spender block hash"))
        .transpose()?;
    Ok(Some(ParsedSpender {
        transaction_id: spending_txid,
        transaction_bytes: spending_bytes,
        block_hash,
    }))
}

fn validate_funding_spender<R>(
    spender: &ParsedSpender,
    outpoint: OutPoint,
) -> Result<(), CoreAdapterError<R>>
where
    R: StdError + 'static,
{
    let transaction = decode_raw_transaction(&spender.transaction_bytes)?;
    if transaction.compute_txid() != spender.transaction_id
        || !transaction
            .input
            .iter()
            .any(|input| input.previous_output == outpoint)
    {
        return Err(CoreAdapterError::SpenderResponseMismatch);
    }
    Ok(())
}

fn refund_block_height<R>(
    agreement: &BtcAgreementV1,
    stable_tip: StableTip,
    confirmations: u32,
) -> Result<Option<u32>, CoreAdapterError<R>>
where
    R: StdError + 'static,
{
    if confirmations == 0 {
        return Ok(None);
    }
    let height = stable_tip
        .height
        .checked_add(1)
        .and_then(|tip| tip.checked_sub(confirmations))
        .ok_or(CoreAdapterError::InvalidConfirmationContext)?;
    if height < agreement.body().recovery_plan().bitcoin_refund_height() {
        return Err(CoreAdapterError::RefundTransactionMismatch);
    }
    Ok(Some(height))
}

fn classify_refund(observed: ObservedRefund, required_confirmations: u32) -> RefundObservation {
    if observed.confirmations == 0 {
        RefundObservation::Revealed(observed)
    } else if observed.confirmations < required_confirmations {
        RefundObservation::Confirming(observed)
    } else {
        RefundObservation::Finalized(observed)
    }
}

fn validate_exact_refund<R>(
    agreement: &BtcAgreementV1,
    transaction: &Transaction,
) -> Result<(), CoreAdapterError<R>>
where
    R: StdError + 'static,
{
    let [input] = transaction.input.as_slice() else {
        return Err(CoreAdapterError::RefundTransactionMismatch);
    };
    let mut witness = input.witness.iter();
    let signature: [u8; 64] = witness
        .next()
        .and_then(|bytes| bytes.try_into().ok())
        .ok_or(CoreAdapterError::RefundTransactionMismatch)?;
    let script = witness
        .next()
        .ok_or(CoreAdapterError::RefundTransactionMismatch)?;
    let control_block = witness
        .next()
        .ok_or(CoreAdapterError::RefundTransactionMismatch)?;
    if witness.next().is_some()
        || script != agreement.p2tr_contract().refund_script_bytes()
        || control_block != agreement.p2tr_contract().refund_control_block_bytes()
    {
        return Err(CoreAdapterError::RefundTransactionMismatch);
    }
    let expected = agreement
        .bitcoin_refund()
        .clone()
        .finalize(signature)
        .map_err(|_| CoreAdapterError::RefundTransactionMismatch)?;
    if &expected != transaction {
        return Err(CoreAdapterError::RefundTransactionMismatch);
    }
    let mut unsigned = transaction.clone();
    unsigned.input[0].witness = Witness::new();
    if serialize(&unsigned) != agreement.bitcoin_refund().unsigned_transaction_bytes() {
        return Err(CoreAdapterError::RefundTransactionMismatch);
    }
    Ok(())
}

fn validate_claim_submission<R>(
    agreement: &BtcAgreementV1,
    transaction_bytes: &[u8],
) -> Result<Transaction, CoreAdapterError<R>>
where
    R: StdError + 'static,
{
    let transaction = decode_raw_transaction(transaction_bytes)?;
    validate_exact_claim(agreement, &transaction)?;
    Ok(transaction)
}

fn validate_funding_submission<R>(
    agreement: &BtcAgreementV1,
    transaction_bytes: &[u8],
) -> Result<Transaction, CoreAdapterError<R>>
where
    R: StdError + 'static,
{
    let transaction = decode_raw_transaction(transaction_bytes)?;
    let funding = agreement.funding_terms();
    let output = transaction
        .output
        .get(
            usize::try_from(funding.output_index())
                .map_err(|_| CoreAdapterError::FundingOutputMismatch)?,
        )
        .ok_or(CoreAdapterError::FundingOutputMismatch)?;
    if output.value.to_sat() != funding.value_sat()
        || output.script_pubkey.as_bytes() != agreement.p2tr_contract().script_pubkey_bytes()
    {
        return Err(CoreAdapterError::FundingOutputMismatch);
    }
    if transaction.compute_txid() != Txid::from_byte_array(*funding.transaction_id()) {
        return Err(CoreAdapterError::FundingTransactionMismatch);
    }
    Ok(transaction)
}

fn widen_core_error<R, S>(error: CoreAdapterError<R>) -> CoreAdapterError<R, S>
where
    R: StdError + 'static,
    S: StdError + 'static,
{
    match error {
        CoreAdapterError::Rpc(error) => CoreAdapterError::Rpc(error),
        CoreAdapterError::Store(never) => match never {},
        CoreAdapterError::WrongCoreVersion => CoreAdapterError::WrongCoreVersion,
        CoreAdapterError::ConnectivityPolicyMismatch => {
            CoreAdapterError::ConnectivityPolicyMismatch
        }
        CoreAdapterError::ChainNotReady => CoreAdapterError::ChainNotReady,
        CoreAdapterError::BitcoinGenesisMismatch => CoreAdapterError::BitcoinGenesisMismatch,
        CoreAdapterError::RequiredIndexNotReady(name) => {
            CoreAdapterError::RequiredIndexNotReady(name)
        }
        CoreAdapterError::MalformedResponse(field) => CoreAdapterError::MalformedResponse(field),
        CoreAdapterError::MalformedRawTransaction => CoreAdapterError::MalformedRawTransaction,
        CoreAdapterError::RawTransactionMetricsMismatch => {
            CoreAdapterError::RawTransactionMetricsMismatch
        }
        CoreAdapterError::FundingOutputMismatch => CoreAdapterError::FundingOutputMismatch,
        CoreAdapterError::FundingTransactionMismatch => {
            CoreAdapterError::FundingTransactionMismatch
        }
        CoreAdapterError::InvalidConfirmationContext => {
            CoreAdapterError::InvalidConfirmationContext
        }
        CoreAdapterError::UnstableTip => CoreAdapterError::UnstableTip,
        CoreAdapterError::SpenderResponseMismatch => CoreAdapterError::SpenderResponseMismatch,
        CoreAdapterError::ClaimTransactionMismatch => CoreAdapterError::ClaimTransactionMismatch,
        CoreAdapterError::FundingAnchorMismatch => CoreAdapterError::FundingAnchorMismatch,
        CoreAdapterError::RefundTransactionMismatch => CoreAdapterError::RefundTransactionMismatch,
        CoreAdapterError::MempoolResponseMismatch => CoreAdapterError::MempoolResponseMismatch,
        CoreAdapterError::BroadcastIdentityMismatch => CoreAdapterError::BroadcastIdentityMismatch,
    }
}

fn record_result<R, S>(
    store: &S,
    attempt: &ClaimSubmissionAttempt,
    state: ClaimSubmissionState,
) -> Result<(), CoreAdapterError<R, S::Error>>
where
    R: StdError + 'static,
    S: ClaimSubmissionStore,
{
    store
        .record_result(attempt, state)
        .map_err(CoreAdapterError::Store)
}

fn parse_ready_chain<R>(
    chain: &GetBlockchainInfo,
    network: Network,
) -> Result<StableTip, CoreAdapterError<R>>
where
    R: StdError + 'static,
{
    if chain.chain != network.to_core_arg()
        || chain.initial_block_download
        || chain.pruned
        || chain.blocks < 0
        || chain.headers != chain.blocks
        || !chain.warnings.is_empty()
    {
        return Err(CoreAdapterError::ChainNotReady);
    }
    let height = u32::try_from(chain.blocks).map_err(|_| CoreAdapterError::ChainNotReady)?;
    let median_time_unix_seconds =
        u64::try_from(chain.median_time).map_err(|_| CoreAdapterError::ChainNotReady)?;
    Ok(StableTip {
        block_hash: parse_block_hash(&chain.best_block_hash, "best block hash")?,
        height,
        median_time_unix_seconds,
    })
}

fn require_index<R>(
    indexes: &GetIndexInfo,
    name: &'static str,
    height: u32,
) -> Result<(), CoreAdapterError<R>>
where
    R: StdError + 'static,
{
    let index = indexes
        .0
        .get(name)
        .ok_or(CoreAdapterError::RequiredIndexNotReady(name))?;
    if !index.synced || index.best_block_height != height {
        return Err(CoreAdapterError::RequiredIndexNotReady(name));
    }
    Ok(())
}

fn parse_block_hash<R>(value: &str, field: &'static str) -> Result<BlockHash, CoreAdapterError<R>>
where
    R: StdError + 'static,
{
    if value.len() != 64 || !is_lower_hex(value) {
        return Err(CoreAdapterError::MalformedResponse(field));
    }
    BlockHash::from_str(value).map_err(|_| CoreAdapterError::MalformedResponse(field))
}

fn decode_raw_hex<R>(value: &str) -> Result<Vec<u8>, CoreAdapterError<R>>
where
    R: StdError + 'static,
{
    if value.is_empty()
        || value.len() > MAX_RAW_TRANSACTION_BYTES.saturating_mul(2)
        || !value.len().is_multiple_of(2)
        || !is_lower_hex(value)
    {
        return Err(CoreAdapterError::MalformedRawTransaction);
    }
    hex::decode(value).map_err(|_| CoreAdapterError::MalformedRawTransaction)
}

fn decode_raw_transaction<R>(bytes: &[u8]) -> Result<Transaction, CoreAdapterError<R>>
where
    R: StdError + 'static,
{
    if bytes.is_empty() || bytes.len() > MAX_RAW_TRANSACTION_BYTES {
        return Err(CoreAdapterError::MalformedRawTransaction);
    }
    let transaction: Transaction =
        deserialize(bytes).map_err(|_| CoreAdapterError::MalformedRawTransaction)?;
    if serialize(&transaction) != bytes {
        return Err(CoreAdapterError::MalformedRawTransaction);
    }
    Ok(transaction)
}

fn parse_verbose_transaction<R>(
    response: &GetRawTransactionVerbose,
    expected_txid: Txid,
) -> Result<Transaction, CoreAdapterError<R>>
where
    R: StdError + 'static,
{
    let bytes = decode_raw_hex(&response.hex)?;
    let transaction = decode_raw_transaction(&bytes)?;
    let response_transaction_id = Txid::from_str(&response.txid)
        .map_err(|_| CoreAdapterError::RawTransactionMetricsMismatch)?;
    let response_witness_id = bitcoin::Wtxid::from_str(&response.hash)
        .map_err(|_| CoreAdapterError::RawTransactionMetricsMismatch)?;
    let size = u64::try_from(transaction.total_size())
        .map_err(|_| CoreAdapterError::RawTransactionMetricsMismatch)?;
    let vsize = u64::try_from(transaction.vsize())
        .map_err(|_| CoreAdapterError::RawTransactionMetricsMismatch)?;
    if transaction.compute_txid() != expected_txid
        || response_transaction_id != expected_txid
        || response_witness_id != transaction.compute_wtxid()
        || response.size != size
        || response.vsize != vsize
        || response.weight != transaction.weight().to_wu()
        || response.version != transaction.version.0
        || response.lock_time != transaction.lock_time.to_consensus_u32()
        || response.inputs.len() != transaction.input.len()
        || response.outputs.len() != transaction.output.len()
    {
        return Err(CoreAdapterError::RawTransactionMetricsMismatch);
    }
    for (typed, consensus) in response.inputs.iter().zip(&transaction.input) {
        let typed = typed
            .to_input()
            .map_err(|_| CoreAdapterError::RawTransactionMetricsMismatch)?;
        if &typed != consensus {
            return Err(CoreAdapterError::RawTransactionMetricsMismatch);
        }
    }
    for (index, (typed, consensus)) in response.outputs.iter().zip(&transaction.output).enumerate()
    {
        if typed.index
            != u64::try_from(index).map_err(|_| CoreAdapterError::RawTransactionMetricsMismatch)?
        {
            return Err(CoreAdapterError::RawTransactionMetricsMismatch);
        }
        let typed = typed
            .to_output()
            .map_err(|_| CoreAdapterError::RawTransactionMetricsMismatch)?;
        if &typed != consensus {
            return Err(CoreAdapterError::RawTransactionMetricsMismatch);
        }
    }
    Ok(transaction)
}

fn confirmation_context<R>(
    response: &GetRawTransactionVerbose,
) -> Result<(u32, Option<BlockHash>), CoreAdapterError<R>>
where
    R: StdError + 'static,
{
    match (response.confirmations, response.block_hash.as_deref()) {
        (None, None) => Ok((0, None)),
        (Some(confirmations), Some(block_hash)) if confirmations > 0 => Ok((
            u32::try_from(confirmations)
                .map_err(|_| CoreAdapterError::InvalidConfirmationContext)?,
            Some(parse_block_hash(block_hash, "transaction block hash")?),
        )),
        _ => Err(CoreAdapterError::InvalidConfirmationContext),
    }
}

fn validate_funding_block_header<R>(
    header: &GetBlockHeaderVerbose,
    expected_hash: BlockHash,
    transaction_confirmations: u32,
    stable_tip: StableTip,
) -> Result<(u32, u64), CoreAdapterError<R>>
where
    R: StdError + 'static,
{
    let header_hash = parse_block_hash(&header.hash, "funding block header hash")?;
    let confirmations = u32::try_from(header.confirmations)
        .map_err(|_| CoreAdapterError::InvalidConfirmationContext)?;
    let height =
        u32::try_from(header.height).map_err(|_| CoreAdapterError::InvalidConfirmationContext)?;
    let median_time_unix_seconds = u64::try_from(header.median_time)
        .map_err(|_| CoreAdapterError::InvalidConfirmationContext)?;
    let expected_tip_height = height
        .checked_add(confirmations.saturating_sub(1))
        .ok_or(CoreAdapterError::InvalidConfirmationContext)?;
    if header_hash != expected_hash
        || confirmations == 0
        || confirmations != transaction_confirmations
        || expected_tip_height != stable_tip.height()
    {
        return Err(CoreAdapterError::InvalidConfirmationContext);
    }
    Ok((height, median_time_unix_seconds))
}

fn validate_exact_claim<R>(
    agreement: &BtcAgreementV1,
    transaction: &Transaction,
) -> Result<(), CoreAdapterError<R>>
where
    R: StdError + 'static,
{
    let [input] = transaction.input.as_slice() else {
        return Err(CoreAdapterError::ClaimTransactionMismatch);
    };
    let mut witness_items = input.witness.iter();
    let signature: [u8; 64] = witness_items
        .next()
        .and_then(|bytes| bytes.try_into().ok())
        .ok_or(CoreAdapterError::ClaimTransactionMismatch)?;
    if witness_items.next().is_some() {
        return Err(CoreAdapterError::ClaimTransactionMismatch);
    }
    let expected = agreement
        .cooperative_claim()
        .clone()
        .finalize(signature)
        .map_err(|_| CoreAdapterError::ClaimTransactionMismatch)?;
    if &expected != transaction {
        return Err(CoreAdapterError::ClaimTransactionMismatch);
    }
    let mut unsigned = transaction.clone();
    unsigned.input[0].witness = Witness::new();
    if serialize(&unsigned) != agreement.cooperative_claim().unsigned_transaction_bytes() {
        return Err(CoreAdapterError::ClaimTransactionMismatch);
    }
    Ok(())
}

fn is_lower_hex(value: &str) -> bool {
    value
        .bytes()
        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
