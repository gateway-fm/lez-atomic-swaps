//! Versioned primitive persistence records for validated Zcash observations.

use std::io::Cursor;

use serde::{Deserialize, Serialize};
use zcash_primitives::transaction::Transaction;
use zcash_protocol::{
    consensus::{BranchId, NetworkType},
    value::Zatoshis,
};

use crate::{CanonicalZcashOutputObservation, CanonicalZcashOutputRemoval, ZcashObservationEvent};

/// Stable primitive spelling of a Zcash network for persistence version 1.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ZcashNetworkRecordV1 {
    /// Zcash mainnet.
    Main,
    /// Public Zcash testnet.
    Test,
    /// Private regression-test network.
    Regtest,
}

impl From<NetworkType> for ZcashNetworkRecordV1 {
    fn from(value: NetworkType) -> Self {
        match value {
            NetworkType::Main => Self::Main,
            NetworkType::Test => Self::Test,
            NetworkType::Regtest => Self::Regtest,
        }
    }
}

/// Historical primitive record of one previously validated canonical output.
///
/// Deserializing this type does not recreate trusted or currently canonical
/// evidence. Call [`Self::validate`] and reconcile with a fresh node snapshot.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ZcashOutputObservationRecordV1 {
    network: ZcashNetworkRecordV1,
    consensus_branch_id: u32,
    block_hash: [u8; 32],
    block_height: u32,
    tip_block_hash: [u8; 32],
    tip_height: u32,
    transaction_id: [u8; 32],
    output_index: u32,
    value_zatoshis: u64,
    output_script_pubkey: Vec<u8>,
    redeem_script: Vec<u8>,
    p2sh_script_pubkey: Vec<u8>,
    confirmations: u32,
    raw_transaction: Vec<u8>,
}

impl From<&CanonicalZcashOutputObservation> for ZcashOutputObservationRecordV1 {
    fn from(value: &CanonicalZcashOutputObservation) -> Self {
        Self {
            network: value.network().into(),
            consensus_branch_id: value.consensus_branch_id().into(),
            block_hash: value.block_hash().0,
            block_height: value.block_height().into(),
            tip_block_hash: value.tip_block_hash().0,
            tip_height: value.tip_height().into(),
            transaction_id: *value.transaction_id().as_ref(),
            output_index: value.outpoint().n(),
            value_zatoshis: value.output().value().into(),
            output_script_pubkey: value.output().script_pubkey().0.0.clone(),
            redeem_script: value.redeem_script().to_vec(),
            p2sh_script_pubkey: value.p2sh_script_pubkey().to_vec(),
            confirmations: value.confirmations().get(),
            raw_transaction: value.raw_transaction().to_vec(),
        }
    }
}

impl ZcashOutputObservationRecordV1 {
    /// Revalidates internal record consistency without asserting fresh canonicality.
    ///
    /// # Errors
    ///
    /// Returns [`ObservationRecordError`] when any branch, depth, transaction,
    /// outpoint, value, or script field disagrees with the canonical raw bytes.
    pub fn validate(&self) -> Result<(), ObservationRecordError> {
        let branch = BranchId::try_from(self.consensus_branch_id)
            .map_err(|_| ObservationRecordError::UnknownBranch)?;
        let depth = self
            .tip_height
            .checked_sub(self.block_height)
            .and_then(|value| value.checked_add(1))
            .ok_or(ObservationRecordError::InvalidDepth)?;
        if self.confirmations == 0 || self.confirmations != depth {
            return Err(ObservationRecordError::InvalidDepth);
        }
        let expected_value = Zatoshis::from_u64(self.value_zatoshis)
            .map_err(|_| ObservationRecordError::InvalidValue)?;
        let mut cursor = Cursor::new(self.raw_transaction.as_slice());
        let transaction = Transaction::read(&mut cursor, branch)
            .map_err(|_| ObservationRecordError::MalformedTransaction)?;
        if cursor.position()
            != u64::try_from(self.raw_transaction.len())
                .map_err(|_| ObservationRecordError::MalformedTransaction)?
        {
            return Err(ObservationRecordError::MalformedTransaction);
        }
        if transaction.txid().as_ref() != &self.transaction_id {
            return Err(ObservationRecordError::TransactionIdMismatch);
        }
        let output = transaction
            .transparent_bundle()
            .and_then(|bundle| {
                usize::try_from(self.output_index)
                    .ok()
                    .and_then(|index| bundle.vout.get(index))
            })
            .ok_or(ObservationRecordError::OutputMismatch)?;
        if output.value() != expected_value
            || output.script_pubkey().0.0 != self.output_script_pubkey
            || self.output_script_pubkey != self.p2sh_script_pubkey
            || self.redeem_script.is_empty()
        {
            return Err(ObservationRecordError::OutputMismatch);
        }
        Ok(())
    }

    fn matches_canonical(&self, value: &CanonicalZcashOutputObservation) -> bool {
        self == &Self::from(value)
    }
}

/// Historical primitive record of affirmative detach evidence.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ZcashOutputRemovalRecordV1 {
    previous: ZcashOutputObservationRecordV1,
    canonical_block_hash_at_removed_height: [u8; 32],
    tip_block_hash: [u8; 32],
    tip_height: u32,
}

impl From<&CanonicalZcashOutputRemoval> for ZcashOutputRemovalRecordV1 {
    fn from(value: &CanonicalZcashOutputRemoval) -> Self {
        Self {
            previous: value.previous().into(),
            canonical_block_hash_at_removed_height: value
                .canonical_block_hash_at_removed_height()
                .0,
            tip_block_hash: value.tip_block_hash().0,
            tip_height: value.tip_height().into(),
        }
    }
}

impl ZcashOutputRemovalRecordV1 {
    fn validate(&self) -> Result<(), ObservationRecordError> {
        self.previous.validate()?;
        if self.canonical_block_hash_at_removed_height == self.previous.block_hash
            || self.tip_height < self.previous.block_height
        {
            return Err(ObservationRecordError::InvalidRemoval);
        }
        Ok(())
    }
}

/// Version-1 durable event payload over primitives only.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum ZcashObservationEventRecordV1 {
    /// A canonical observation or depth update.
    Canonical {
        /// Complete historical canonical evidence.
        canonical: ZcashOutputObservationRecordV1,
    },
    /// Affirmative canonical removal.
    Removed {
        /// Complete historical detach evidence.
        removal: ZcashOutputRemovalRecordV1,
    },
    /// Atomic detach and canonical replacement.
    Replaced {
        /// Complete historical detach evidence.
        removed: Box<ZcashOutputRemovalRecordV1>,
        /// Complete historical replacement evidence.
        canonical: Box<ZcashOutputObservationRecordV1>,
    },
}

impl ZcashObservationEventRecordV1 {
    /// Creates a version-1 canonical event record from trusted in-memory evidence.
    #[must_use]
    pub fn from_canonical(value: &CanonicalZcashOutputObservation) -> Self {
        Self::Canonical {
            canonical: value.into(),
        }
    }

    /// Creates a version-1 event record from a validated watcher event.
    #[must_use]
    pub fn from_event(value: &ZcashObservationEvent) -> Self {
        match value {
            ZcashObservationEvent::Canonical(canonical) => Self::from_canonical(canonical),
            ZcashObservationEvent::Removed(removal) => Self::Removed {
                removal: removal.into(),
            },
            ZcashObservationEvent::Replaced { removed, canonical } => Self::Replaced {
                removed: Box::new(removed.as_ref().into()),
                canonical: Box::new(canonical.as_ref().into()),
            },
        }
    }

    /// Revalidates every internal primitive binding in the historical event.
    ///
    /// # Errors
    ///
    /// Returns [`ObservationRecordError`] when any contained observation or
    /// removal proof is internally inconsistent.
    pub fn validate(&self) -> Result<(), ObservationRecordError> {
        match self {
            Self::Canonical { canonical } => canonical.validate(),
            Self::Removed { removal } => removal.validate(),
            Self::Replaced { removed, canonical } => {
                removed.validate()?;
                canonical.validate()
            }
        }
    }

    /// Compares a canonical record with freshly validated trusted evidence.
    #[must_use]
    pub fn matches_canonical(&self, value: &CanonicalZcashOutputObservation) -> bool {
        matches!(self, Self::Canonical { canonical } if canonical.matches_canonical(value))
    }
}

/// Corrupt or unsupported primitive event evidence.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum ObservationRecordError {
    /// The consensus branch identifier is unknown to the pinned implementation.
    #[error("unknown persisted Zcash consensus branch")]
    UnknownBranch,
    /// Confirmation depth is zero, overflowed, or inconsistent with heights.
    #[error("persisted Zcash confirmation depth is inconsistent")]
    InvalidDepth,
    /// The value exceeds the Zcash monetary range.
    #[error("persisted Zcash value is invalid")]
    InvalidValue,
    /// Raw transaction bytes cannot be decoded exactly.
    #[error("persisted Zcash transaction bytes are malformed")]
    MalformedTransaction,
    /// The stored transaction identifier differs from decoded bytes.
    #[error("persisted Zcash transaction identifier is inconsistent")]
    TransactionIdMismatch,
    /// The stored outpoint, value, or scripts differ from decoded bytes.
    #[error("persisted Zcash output evidence is inconsistent")]
    OutputMismatch,
    /// Detach evidence retained an unchanged block or an insufficient tip.
    #[error("persisted Zcash removal evidence is inconsistent")]
    InvalidRemoval,
}
