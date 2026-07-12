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

    /// Zcash network authorized by the negotiated terms.
    #[must_use]
    pub const fn network(&self) -> NetworkType {
        self.network
    }

    /// Consensus branch authorized by the negotiated terms.
    #[must_use]
    pub const fn consensus_branch_id(&self) -> BranchId {
        self.consensus_branch_id
    }

    /// Exact transparent output value authorized by the negotiated terms.
    #[must_use]
    pub const fn value(&self) -> Zatoshis {
        self.value
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
    tip: ZcashStableTip,
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
        tip: ZcashStableTip,
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
            tip,
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

    /// Replaces both untrusted tip heights during snapshot assembly/testing.
    pub const fn set_tip_height(&mut self, value: BlockHeight) {
        self.tip.before_height = value;
        self.tip.after_height = value;
    }

    /// Replaces the second untrusted tip sample during snapshot assembly/testing.
    pub const fn set_tip_after(&mut self, hash: BlockHash, height: BlockHeight) {
        self.tip.after_hash = hash;
        self.tip.after_height = height;
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

/// Best-chain tip sampled before and after a multi-query observation attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ZcashStableTip {
    before_hash: BlockHash,
    before_height: BlockHeight,
    after_hash: BlockHash,
    after_height: BlockHeight,
}

impl ZcashStableTip {
    /// Creates an untrusted pair of tip samples.
    #[must_use]
    pub const fn new(
        before_hash: BlockHash,
        before_height: BlockHeight,
        after_hash: BlockHash,
        after_height: BlockHeight,
    ) -> Self {
        Self {
            before_hash,
            before_height,
            after_hash,
            after_height,
        }
    }

    fn validated(self) -> Result<(BlockHash, BlockHeight), ObservationError> {
        if self.before_hash != self.after_hash || self.before_height != self.after_height {
            return Err(ObservationError::UnstableTip);
        }
        Ok((self.after_hash, self.after_height))
    }
}

/// Node evidence that a previously canonical output's inclusion block was detached.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ZcashNodeRemovalSnapshot {
    network: NetworkType,
    consensus_branch_id: BranchId,
    canonical_block_hash_at_removed_height: BlockHash,
    tip: ZcashStableTip,
}

impl ZcashNodeRemovalSnapshot {
    /// Creates untrusted affirmative removal evidence from stable node queries.
    #[must_use]
    pub const fn new(
        network: NetworkType,
        consensus_branch_id: BranchId,
        canonical_block_hash_at_removed_height: BlockHash,
        tip: ZcashStableTip,
    ) -> Self {
        Self {
            network,
            consensus_branch_id,
            canonical_block_hash_at_removed_height,
            tip,
        }
    }
}

/// A rejected node snapshot or expected output binding.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum ObservationError {
    /// The best-chain tip changed while the adapter assembled its multi-query snapshot.
    #[error("best-chain tip changed during Zcash observation")]
    UnstableTip,
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
    /// The alleged removal still has the same canonical block at its inclusion height.
    #[error("canonical block at the observed height has not changed")]
    InclusionStillCanonical,
}

/// Complete canonical Zcash output evidence retained before core projection.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CanonicalZcashOutputObservation {
    pub(crate) network: NetworkType,
    pub(crate) consensus_branch_id: BranchId,
    pub(crate) block_hash: BlockHash,
    pub(crate) block_height: BlockHeight,
    pub(crate) tip_block_hash: BlockHash,
    pub(crate) tip_height: BlockHeight,
    pub(crate) transaction_id: TxId,
    pub(crate) outpoint: OutPoint,
    pub(crate) output: TxOut,
    pub(crate) redeem_script: Box<[u8]>,
    pub(crate) p2sh_script_pubkey: Box<[u8]>,
    pub(crate) confirmations: NonZeroU32,
    pub(crate) raw_transaction: Box<[u8]>,
}

/// Affirmative, validated evidence that one canonical output was detached.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CanonicalZcashOutputRemoval {
    pub(crate) previous: CanonicalZcashOutputObservation,
    pub(crate) canonical_block_hash_at_removed_height: BlockHash,
    pub(crate) tip_block_hash: BlockHash,
    pub(crate) tip_height: BlockHeight,
}

impl CanonicalZcashOutputRemoval {
    /// Validates network, branch, stable-tip, height, and changed-block bindings.
    ///
    /// # Errors
    ///
    /// Returns a typed observation error when the removal uses a different
    /// network/branch, an unstable or too-short tip, or an unchanged inclusion block.
    pub fn validate(
        previous: &CanonicalZcashOutputObservation,
        snapshot: &ZcashNodeRemovalSnapshot,
    ) -> Result<Self, ObservationError> {
        if snapshot.network != previous.network {
            return Err(ObservationError::NetworkMismatch);
        }
        if snapshot.consensus_branch_id != previous.consensus_branch_id {
            return Err(ObservationError::ConsensusBranchMismatch);
        }
        let (tip_block_hash, tip_height) = snapshot.tip.validated()?;
        if tip_height < previous.block_height {
            return Err(ObservationError::BlockAboveTip);
        }
        if snapshot.canonical_block_hash_at_removed_height == previous.block_hash {
            return Err(ObservationError::InclusionStillCanonical);
        }
        Ok(Self {
            previous: previous.clone(),
            canonical_block_hash_at_removed_height: snapshot.canonical_block_hash_at_removed_height,
            tip_block_hash,
            tip_height,
        })
    }

    /// The exact validated observation that left the active chain.
    #[must_use]
    pub const fn previous(&self) -> &CanonicalZcashOutputObservation {
        &self.previous
    }

    /// Canonical replacement block at the detached observation's former height.
    #[must_use]
    pub const fn canonical_block_hash_at_removed_height(&self) -> BlockHash {
        self.canonical_block_hash_at_removed_height
    }

    /// Stable replacement best-chain tip hash.
    #[must_use]
    pub const fn tip_block_hash(&self) -> BlockHash {
        self.tip_block_hash
    }

    /// Stable replacement best-chain tip height.
    #[must_use]
    pub const fn tip_height(&self) -> BlockHeight {
        self.tip_height
    }
}

/// A canonicality change emitted by a Zcash output watcher.
///
/// Events retain the complete validated observation instead of the lossy generic
/// coordinator projection. A replacement is one atomic transition so durable
/// consumers cannot observe the old output as removed without also seeing the new
/// canonical evidence.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ZcashObservationEvent {
    /// An output became canonical or its canonical confirmation depth changed.
    Canonical(CanonicalZcashOutputObservation),
    /// A previously canonical output is no longer in the active chain.
    Removed(CanonicalZcashOutputRemoval),
    /// Canonical evidence changed without an intervening absent observation.
    Replaced {
        /// Evidence that left the active chain.
        removed: Box<CanonicalZcashOutputRemoval>,
        /// Replacement evidence now in the active chain.
        canonical: Box<CanonicalZcashOutputObservation>,
    },
}

/// One validated input to canonicality reconciliation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ZcashObservationReconciliation {
    /// A validated output is currently canonical.
    Canonical(CanonicalZcashOutputObservation),
    /// Affirmative evidence removed the current observation.
    Removed(CanonicalZcashOutputRemoval),
    /// One stable poll proved removal and its replacement atomically.
    Replaced {
        /// Affirmative evidence for the detached observation.
        removed: Box<CanonicalZcashOutputRemoval>,
        /// Newly canonical output evidence.
        canonical: Box<CanonicalZcashOutputObservation>,
    },
}

/// A stale or incomplete tracker transition.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum ObservationTrackerError {
    /// Different canonical evidence requires affirmative removal proof.
    #[error("different canonical evidence requires explicit replacement proof")]
    ReplacementProofRequired,
    /// Removal or commit evidence does not match the durable tracker head.
    #[error("observation event does not match the durable tracker head")]
    StaleEvidence,
}

/// Stateful canonicality reconciler for one expected Zcash output.
///
/// The tracker is deliberately independent of RPC transport. Production polling
/// assembles and validates a stable [`ZcashNodeSnapshot`], then persists each event
/// before projecting it into coordinator state. Restoring [`Self::current`] on restart
/// makes at-least-once polling idempotent.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ZcashObservationTracker {
    current: Option<CanonicalZcashOutputObservation>,
}

impl ZcashObservationTracker {
    /// Restores a tracker from its last durable canonical observation.
    #[must_use]
    pub const fn from_current(current: Option<CanonicalZcashOutputObservation>) -> Self {
        Self { current }
    }

    /// Returns the most recent canonical observation, if any.
    #[must_use]
    pub const fn current(&self) -> Option<&CanonicalZcashOutputObservation> {
        self.current.as_ref()
    }

    /// Proposes a meaningful event without advancing the durable tracker head.
    ///
    /// The caller must commit the event atomically before calling [`Self::apply_committed`].
    ///
    /// # Errors
    ///
    /// Returns [`ObservationTrackerError`] for stale removal evidence or a changed
    /// canonical inclusion that lacks explicit replacement proof.
    pub fn propose(
        &self,
        input: &ZcashObservationReconciliation,
    ) -> Result<Option<ZcashObservationEvent>, ObservationTrackerError> {
        match input {
            ZcashObservationReconciliation::Canonical(canonical) => match &self.current {
                None => Ok(Some(ZcashObservationEvent::Canonical(canonical.clone()))),
                Some(current) if current == canonical => Ok(None),
                Some(current) if same_canonical_inclusion(current, canonical) => {
                    Ok(Some(ZcashObservationEvent::Canonical(canonical.clone())))
                }
                Some(_) => Err(ObservationTrackerError::ReplacementProofRequired),
            },
            ZcashObservationReconciliation::Removed(removed) => match &self.current {
                None => Ok(None),
                Some(current) if current == removed.previous() => {
                    Ok(Some(ZcashObservationEvent::Removed(removed.clone())))
                }
                Some(_) => Err(ObservationTrackerError::StaleEvidence),
            },
            ZcashObservationReconciliation::Replaced { removed, canonical } => {
                match &self.current {
                    Some(current) if current == canonical.as_ref() => Ok(None),
                    Some(current) if current == removed.previous() => {
                        Ok(Some(ZcashObservationEvent::Replaced {
                            removed: removed.clone(),
                            canonical: canonical.clone(),
                        }))
                    }
                    _ => Err(ObservationTrackerError::StaleEvidence),
                }
            }
        }
    }

    /// Advances the tracker only after the exact proposed event is durably committed.
    ///
    /// # Errors
    ///
    /// Returns [`ObservationTrackerError`] if the event is stale or does not match
    /// the proposal implied by the current durable head.
    pub fn apply_committed(
        &mut self,
        event: &ZcashObservationEvent,
    ) -> Result<(), ObservationTrackerError> {
        let input = match event {
            ZcashObservationEvent::Canonical(canonical) => {
                ZcashObservationReconciliation::Canonical(canonical.clone())
            }
            ZcashObservationEvent::Removed(removed) => {
                ZcashObservationReconciliation::Removed(removed.clone())
            }
            ZcashObservationEvent::Replaced { removed, canonical } => {
                ZcashObservationReconciliation::Replaced {
                    removed: removed.clone(),
                    canonical: canonical.clone(),
                }
            }
        };
        if self.propose(&input)? != Some(event.clone()) {
            return Err(ObservationTrackerError::StaleEvidence);
        }
        self.current = match event {
            ZcashObservationEvent::Canonical(canonical) => Some(canonical.clone()),
            ZcashObservationEvent::Replaced { canonical, .. } => Some(canonical.as_ref().clone()),
            ZcashObservationEvent::Removed(_) => None,
        };
        Ok(())
    }
}

fn same_canonical_inclusion(
    left: &CanonicalZcashOutputObservation,
    right: &CanonicalZcashOutputObservation,
) -> bool {
    left.network == right.network
        && left.consensus_branch_id == right.consensus_branch_id
        && left.block_hash == right.block_hash
        && left.block_height == right.block_height
        && left.outpoint == right.outpoint
        && left.output == right.output
        && left.redeem_script == right.redeem_script
        && left.p2sh_script_pubkey == right.p2sh_script_pubkey
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
        let (tip_block_hash, tip_height) = snapshot.tip.validated()?;
        let depth = u32::from(tip_height)
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
            tip_block_hash,
            tip_height,
            transaction_id,
            outpoint,
            output,
            redeem_script: expected.contract.redeem_script().into(),
            p2sh_script_pubkey: expected.contract.p2sh_script_pubkey().into(),
            confirmations,
            raw_transaction: snapshot.raw_transaction.clone().into(),
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

    /// Stable best-chain tip hash used to derive confirmation depth.
    #[must_use]
    pub const fn tip_block_hash(&self) -> BlockHash {
        self.tip_block_hash
    }

    /// Stable best-chain tip height used to derive confirmation depth.
    #[must_use]
    pub const fn tip_height(&self) -> BlockHeight {
        self.tip_height
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

    /// Canonical transaction bytes that were independently decoded and identified.
    #[must_use]
    pub fn raw_transaction(&self) -> &[u8] {
        &self.raw_transaction
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
