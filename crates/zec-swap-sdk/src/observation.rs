//! Validation of canonical Zcash output observations before coordinator use.

use std::{io::Cursor, num::NonZeroU32};

use lez_swap_core::ChainProof;
use zcash_primitives::{block::BlockHash, transaction::Transaction};
use zcash_protocol::{
    TxId,
    consensus::{BlockHeight, BranchId, NetworkType},
    value::Zatoshis,
};
use zcash_transparent::bundle::{OutPoint, TxOut};

use crate::Bip199Contract;

/// Immutable BIP-199 output terms expected from a selected Zcash network.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExpectedBip199Output {
    network: NetworkType,
    consensus_branch_id: BranchId,
    value: Zatoshis,
    contract: Bip199Contract,
}

impl ExpectedBip199Output {
    /// Creates expected output terms already authorized by signed swap negotiation.
    #[must_use]
    pub const fn new(
        network: NetworkType,
        consensus_branch_id: BranchId,
        value: Zatoshis,
        contract: Bip199Contract,
    ) -> Self {
        Self {
            network,
            consensus_branch_id,
            value,
            contract,
        }
    }

    /// Exact expected BIP-199 contract.
    #[must_use]
    pub const fn contract(&self) -> &Bip199Contract {
        &self.contract
    }
}

/// One internally consistent snapshot assembled from authoritative node RPC responses.
///
/// This type is intentionally untrusted and mutable while an adapter assembles a stable
/// snapshot. Only [`CanonicalZcashOutputObservation::validate`] produces coordinator evidence.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ZcashNodeSnapshot {
    network: NetworkType,
    consensus_branch_id: BranchId,
    in_active_chain: bool,
    transaction_block_hash: BlockHash,
    canonical_block_hash: BlockHash,
    block_height: BlockHeight,
    tip_height: BlockHeight,
    reported_transaction_id: TxId,
    raw_transaction: Vec<u8>,
    output_index: u32,
    reported_confirmations: u32,
}

impl ZcashNodeSnapshot {
    /// Creates an untrusted snapshot from a stable node-query attempt.
    #[allow(clippy::too_many_arguments)]
    #[must_use]
    pub fn new(
        network: NetworkType,
        consensus_branch_id: BranchId,
        in_active_chain: bool,
        transaction_block_hash: BlockHash,
        canonical_block_hash: BlockHash,
        block_height: BlockHeight,
        tip_height: BlockHeight,
        reported_transaction_id: TxId,
        raw_transaction: Vec<u8>,
        output_index: u32,
        reported_confirmations: u32,
    ) -> Self {
        Self {
            network,
            consensus_branch_id,
            in_active_chain,
            transaction_block_hash,
            canonical_block_hash,
            block_height,
            tip_height,
            reported_transaction_id,
            raw_transaction,
            output_index,
            reported_confirmations,
        }
    }

    /// Replaces the untrusted active-chain flag during snapshot assembly/testing.
    pub const fn set_in_active_chain(&mut self, value: bool) {
        self.in_active_chain = value;
    }

    /// Replaces the untrusted network during snapshot assembly/testing.
    pub const fn set_network(&mut self, value: NetworkType) {
        self.network = value;
    }

    /// Replaces the untrusted branch during snapshot assembly/testing.
    pub const fn set_consensus_branch_id(&mut self, value: BranchId) {
        self.consensus_branch_id = value;
    }

    /// Replaces the canonical height lookup hash during snapshot assembly/testing.
    pub const fn set_canonical_block_hash(&mut self, value: BlockHash) {
        self.canonical_block_hash = value;
    }

    /// Replaces the untrusted tip during snapshot assembly/testing.
    pub const fn set_tip_height(&mut self, value: BlockHeight) {
        self.tip_height = value;
    }

    /// Replaces the untrusted inclusion height during snapshot assembly/testing.
    pub const fn set_block_height(&mut self, value: BlockHeight) {
        self.block_height = value;
    }

    /// Replaces the RPC-reported transaction identifier during snapshot assembly/testing.
    pub const fn set_reported_transaction_id(&mut self, value: TxId) {
        self.reported_transaction_id = value;
    }

    /// Replaces the explicit output index during snapshot assembly/testing.
    pub const fn set_output_index(&mut self, value: u32) {
        self.output_index = value;
    }

    /// Replaces the RPC-reported confirmation count during snapshot assembly/testing.
    pub const fn set_reported_confirmations(&mut self, value: u32) {
        self.reported_confirmations = value;
    }
}

/// A rejected node snapshot or expected output binding.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum ObservationError {
    /// The transaction is not in the node's active best chain.
    #[error("transaction is not in the active chain")]
    InactiveChain,
    /// The actual node network differs from signed terms.
    #[error("Zcash observation network mismatch")]
    NetworkMismatch,
    /// The transaction branch differs from signed terms.
    #[error("Zcash observation consensus branch mismatch")]
    ConsensusBranchMismatch,
    /// Transaction context and the canonical height lookup returned different blocks.
    #[error("transaction block hash is not canonical at its height")]
    BlockHashMismatch,
    /// The claimed inclusion height is above the observed canonical tip.
    #[error("transaction block height is above the canonical tip")]
    BlockAboveTip,
    /// Canonical transaction bytes could not be decoded for the selected branch.
    #[error("canonical transaction decoding failed")]
    MalformedTransaction,
    /// Bytes remained after decoding one canonical transaction.
    #[error("raw transaction contains trailing bytes")]
    TrailingTransactionBytes,
    /// The RPC-reported transaction ID differs from the canonical bytes.
    #[error("reported transaction ID differs from canonical bytes")]
    TransactionIdMismatch,
    /// The committed transparent output index does not exist.
    #[error("transparent output index is out of range")]
    OutputIndexOutOfRange,
    /// The observed output value differs from signed terms.
    #[error("transparent output value differs from signed terms")]
    ValueMismatch,
    /// The observed script is not the exact expected BIP-199 P2SH commitment.
    #[error("transparent output script differs from signed BIP-199 terms")]
    ScriptMismatch,
    /// Confirmation depth arithmetic overflowed.
    #[error("canonical confirmation depth overflowed")]
    ConfirmationOverflow,
    /// RPC confirmation depth differs from the block/tip-derived depth.
    #[error("reported confirmations differ from canonical block depth")]
    ConfirmationMismatch,
}

/// Complete canonical Zcash output evidence retained before core projection.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CanonicalZcashOutputObservation {
    network: NetworkType,
    consensus_branch_id: BranchId,
    block_hash: BlockHash,
    block_height: BlockHeight,
    transaction_id: TxId,
    outpoint: OutPoint,
    output: TxOut,
    redeem_script: Box<[u8]>,
    p2sh_script_pubkey: Box<[u8]>,
    confirmations: NonZeroU32,
}

impl CanonicalZcashOutputObservation {
    /// Validates exact transaction, output, block, network, branch, and depth bindings.
    ///
    /// # Errors
    ///
    /// Returns a typed error for any inconsistent, noncanonical, malformed, or
    /// terms-mismatched snapshot. Confirmation depth is recomputed from heights.
    pub fn validate(
        expected: &ExpectedBip199Output,
        snapshot: &ZcashNodeSnapshot,
    ) -> Result<Self, ObservationError> {
        if snapshot.network != expected.network {
            return Err(ObservationError::NetworkMismatch);
        }
        if snapshot.consensus_branch_id != expected.consensus_branch_id {
            return Err(ObservationError::ConsensusBranchMismatch);
        }
        if !snapshot.in_active_chain {
            return Err(ObservationError::InactiveChain);
        }
        if snapshot.transaction_block_hash != snapshot.canonical_block_hash {
            return Err(ObservationError::BlockHashMismatch);
        }
        let depth = u32::from(snapshot.tip_height)
            .checked_sub(u32::from(snapshot.block_height))
            .ok_or(ObservationError::BlockAboveTip)?
            .checked_add(1)
            .ok_or(ObservationError::ConfirmationOverflow)?;
        let confirmations = NonZeroU32::new(depth).ok_or(ObservationError::ConfirmationOverflow)?;
        if depth != snapshot.reported_confirmations {
            return Err(ObservationError::ConfirmationMismatch);
        }

        let mut cursor = Cursor::new(snapshot.raw_transaction.as_slice());
        let transaction = Transaction::read(&mut cursor, snapshot.consensus_branch_id)
            .map_err(|_| ObservationError::MalformedTransaction)?;
        if cursor.position()
            != u64::try_from(snapshot.raw_transaction.len())
                .map_err(|_| ObservationError::TrailingTransactionBytes)?
        {
            return Err(ObservationError::TrailingTransactionBytes);
        }
        let transaction_id = transaction.txid();
        if transaction_id != snapshot.reported_transaction_id {
            return Err(ObservationError::TransactionIdMismatch);
        }
        let index = usize::try_from(snapshot.output_index)
            .map_err(|_| ObservationError::OutputIndexOutOfRange)?;
        let output = transaction
            .transparent_bundle()
            .and_then(|bundle| bundle.vout.get(index))
            .cloned()
            .ok_or(ObservationError::OutputIndexOutOfRange)?;
        if output.value() != expected.value {
            return Err(ObservationError::ValueMismatch);
        }
        if output.script_pubkey().0.0 != expected.contract.p2sh_script_pubkey() {
            return Err(ObservationError::ScriptMismatch);
        }
        let outpoint = OutPoint::new(*transaction_id.as_ref(), snapshot.output_index);

        Ok(Self {
            network: snapshot.network,
            consensus_branch_id: snapshot.consensus_branch_id,
            block_hash: snapshot.transaction_block_hash,
            block_height: snapshot.block_height,
            transaction_id,
            outpoint,
            output,
            redeem_script: expected.contract.redeem_script().into(),
            p2sh_script_pubkey: expected.contract.p2sh_script_pubkey().into(),
            confirmations,
        })
    }

    /// Zcash network that produced the canonical observation.
    #[must_use]
    pub const fn network(&self) -> NetworkType {
        self.network
    }

    /// Consensus branch used to decode and identify the transaction.
    #[must_use]
    pub const fn consensus_branch_id(&self) -> BranchId {
        self.consensus_branch_id
    }

    /// Canonical inclusion block hash.
    #[must_use]
    pub const fn block_hash(&self) -> BlockHash {
        self.block_hash
    }

    /// Canonical inclusion height.
    #[must_use]
    pub const fn block_height(&self) -> BlockHeight {
        self.block_height
    }

    /// Canonically decoded transaction identifier.
    #[must_use]
    pub const fn transaction_id(&self) -> TxId {
        self.transaction_id
    }

    /// Exact committed transparent outpoint.
    #[must_use]
    pub const fn outpoint(&self) -> &OutPoint {
        &self.outpoint
    }

    /// Exact decoded transparent value and script.
    #[must_use]
    pub const fn output(&self) -> &TxOut {
        &self.output
    }

    /// Exact signed BIP-199 redeem script bytes.
    #[must_use]
    pub fn redeem_script(&self) -> &[u8] {
        &self.redeem_script
    }

    /// Exact P2SH output commitment derived from the redeem script.
    #[must_use]
    pub fn p2sh_script_pubkey(&self) -> &[u8] {
        &self.p2sh_script_pubkey
    }

    /// Canonical depth derived from inclusion and tip heights.
    #[must_use]
    pub const fn confirmations(&self) -> NonZeroU32 {
        self.confirmations
    }

    /// Projects validated chain-specific evidence into the generic coordinator input.
    ///
    /// The full observation must be persisted as source evidence; this lossy projection
    /// is suitable only for the existing chain-independent transition API.
    ///
    /// # Errors
    ///
    /// Returns a core identifier error if its transaction-ID rules change.
    pub fn chain_proof(&self) -> Result<ChainProof, lez_swap_core::Error> {
        ChainProof::new(self.transaction_id.to_string(), self.confirmations.get())
    }
}
