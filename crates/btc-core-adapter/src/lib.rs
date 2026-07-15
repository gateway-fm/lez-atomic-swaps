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
use bitcoin::consensus::{deserialize, serialize};
use bitcoin::hashes::{Hash as _, sha256};
use bitcoin::{BlockHash, OutPoint, Transaction, Txid, Witness, Wtxid};
use corepc_types::v31::{
    GetBlockHash, GetBlockchainInfo, GetIndexInfo, GetNetworkInfo, GetRawTransactionVerbose,
    GetTxSpendingPrevout, SendRawTransaction, TestMempoolAccept,
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

/// Expected Bitcoin Core P2P connectivity for the selected deployment route.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CoreConnectivityPolicy {
    /// Private Regtest: networking disabled and every connection count exactly zero.
    IsolatedLocal,
    /// Explicit network-enabled Regtest route; peer readiness is external policy.
    ///
    /// The current adapter still requires the agreement's Regtest genesis and Core's
    /// `chain=regtest`; Testnet4 admission is a later configuration-portability slice.
    Networked,
}

/// Stable chain position bracketing one observation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StableTip {
    block_hash: BlockHash,
    height: u32,
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
}

/// Reconstructed, sufficiently confirmed agreement funding output.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ObservedFunding {
    transaction: Transaction,
    confirmations: u32,
    block_hash: BlockHash,
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

    /// Stable tip bracketing this observation.
    #[must_use]
    pub const fn stable_tip(&self) -> StableTip {
        self.stable_tip
    }
}

/// Current state of the exact agreement funding output.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FundingObservation {
    /// The transaction is not available through the required transaction index.
    Absent,
    /// The exact output exists but has not reached the signed confirmation policy.
    Pending {
        /// Current confirmation count, including zero for mempool funding.
        confirmations: u32,
    },
    /// The exact output exists with sufficient active-chain confirmations.
    Ready(ObservedFunding),
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
    #[error("local node is not exact Bitcoin Core 31.1")]
    WrongCoreVersion,
    /// Node P2P connectivity contradicts the explicitly selected deployment route.
    #[error("Bitcoin Core connectivity contradicts the configured deployment policy")]
    ConnectivityPolicyMismatch,
    /// Node is not a ready, unpruned local regtest chain.
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

    /// Requires exact Core 31.1, regtest genesis, ready chain, and synced indexes.
    ///
    /// # Errors
    ///
    /// Rejects every identity, readiness, response, or RPC mismatch.
    pub async fn ensure_ready(
        &self,
        agreement: &BtcAgreementV1,
    ) -> Result<StableTip, CoreAdapterError<R::Error>> {
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
            CoreConnectivityPolicy::Networked => network.network_active,
        };
        if !connectivity_matches {
            return Err(CoreAdapterError::ConnectivityPolicyMismatch);
        }
        let chain = self
            .rpc
            .get_blockchain_info()
            .await
            .map_err(CoreAdapterError::Rpc)?;
        let tip = parse_ready_chain(&chain)?;
        let genesis = self
            .rpc
            .get_genesis_hash()
            .await
            .map_err(CoreAdapterError::Rpc)?;
        let observed_genesis = parse_block_hash(&genesis.0, "genesis hash")?;
        let expected_genesis = BlockHash::from_byte_array(*agreement.bitcoin_genesis_hash());
        if observed_genesis != expected_genesis {
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
        let before = self.ensure_ready(agreement).await?;
        let funding = agreement.funding_terms();
        let expected_txid = Txid::from_byte_array(*funding.transaction_id());
        let response = self
            .rpc
            .get_raw_transaction(expected_txid)
            .await
            .map_err(CoreAdapterError::Rpc)?;
        let after = self.current_ready_tip().await?;
        if before != after {
            return Err(CoreAdapterError::UnstableTip);
        }
        let Some(response) = response else {
            return Ok(FundingObservation::Absent);
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
            return Ok(FundingObservation::Pending { confirmations });
        }
        let block_hash = block_hash.ok_or(CoreAdapterError::InvalidConfirmationContext)?;
        Ok(FundingObservation::Ready(ObservedFunding {
            transaction,
            confirmations,
            block_hash,
            stable_tip: after,
        }))
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
        self.ensure_ready(agreement).await?;
        let transaction_id = transaction.compute_txid();
        let Ok(mempool) = self.rpc.test_mempool_accept(transaction_bytes).await else {
            return Ok(AuthorizedClaimSubmission::Unknown);
        };
        let [acceptance] = mempool.0.as_slice() else {
            return Err(CoreAdapterError::MempoolResponseMismatch);
        };
        let accepted_transaction_id = Txid::from_str(&acceptance.txid)
            .map_err(|_| CoreAdapterError::MempoolResponseMismatch)?;
        let accepted_witness_id = bitcoin::Wtxid::from_str(&acceptance.wtxid)
            .map_err(|_| CoreAdapterError::MempoolResponseMismatch)?;
        if accepted_transaction_id != transaction_id
            || accepted_witness_id != transaction.compute_wtxid()
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
                    AuthorizedClaimSubmission::Unknown
                } else {
                    AuthorizedClaimSubmission::Rejected
                },
            );
        }
        let state = match self.rpc.send_raw_transaction(transaction_bytes).await {
            Ok(response) => {
                let response_txid = Txid::from_str(&response.0)
                    .map_err(|_| CoreAdapterError::BroadcastIdentityMismatch)?;
                if response_txid != transaction_id {
                    return Err(CoreAdapterError::BroadcastIdentityMismatch);
                }
                AuthorizedClaimSubmission::Accepted { transaction_id }
            }
            Err(error) => match R::classify_send_failure(&error) {
                SendFailure::DefinitiveRejection => AuthorizedClaimSubmission::Rejected,
                SendFailure::Unknown => AuthorizedClaimSubmission::Unknown,
            },
        };
        Ok(state)
    }

    async fn current_ready_tip(&self) -> Result<StableTip, CoreAdapterError<R::Error>> {
        let chain = self
            .rpc
            .get_blockchain_info()
            .await
            .map_err(CoreAdapterError::Rpc)?;
        parse_ready_chain(&chain)
    }
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
        CoreAdapterError::InvalidConfirmationContext => {
            CoreAdapterError::InvalidConfirmationContext
        }
        CoreAdapterError::UnstableTip => CoreAdapterError::UnstableTip,
        CoreAdapterError::SpenderResponseMismatch => CoreAdapterError::SpenderResponseMismatch,
        CoreAdapterError::ClaimTransactionMismatch => CoreAdapterError::ClaimTransactionMismatch,
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

fn parse_ready_chain<R>(chain: &GetBlockchainInfo) -> Result<StableTip, CoreAdapterError<R>>
where
    R: StdError + 'static,
{
    if chain.chain != "regtest"
        || chain.initial_block_download
        || chain.pruned
        || chain.blocks < 0
        || chain.headers != chain.blocks
        || !chain.warnings.is_empty()
    {
        return Err(CoreAdapterError::ChainNotReady);
    }
    let height = u32::try_from(chain.blocks).map_err(|_| CoreAdapterError::ChainNotReady)?;
    Ok(StableTip {
        block_hash: parse_block_hash(&chain.best_block_hash, "best block hash")?,
        height,
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
