//! Canonical durable evidence emitted only from validated Core observations.

use std::convert::Infallible;
use std::str::FromStr as _;

use bitcoin::consensus::{deserialize, serialize};
use bitcoin::hashes::Hash as _;
use bitcoin::{BlockHash, Transaction, Txid, Wtxid};
use lez_btc_swap_sdk::BtcAgreementV1;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{ClaimObservation, ObservedFunding, StableTip, validate_exact_claim};

const EVIDENCE_SCHEMA_VERSION: u16 = 1;

/// Maximum canonical evidence payload accepted by the Bitcoin recovery store.
///
/// This deliberately matches the store's 64 KiB raw chain-evidence bound. An
/// otherwise valid transaction whose canonical JSON evidence exceeds the bound
/// is not eligible for durable projection through this version of the contract.
pub const MAX_BITCOIN_CORE_EVIDENCE_BYTES: usize = 64 * 1024;

/// Semantic state proved by one adapter-validated evidence value.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BitcoinCoreEvidenceKind {
    /// The exact agreement funding output reached its signed confirmation policy.
    FundingReady,
    /// The exact claim is public in the mempool and exposes its witness.
    ClaimRevealed,
    /// The exact claim is confirmed below the signed confirmation policy.
    ClaimConfirming,
    /// The exact claim reached the signed confirmation policy.
    ClaimFinalized,
}

impl BitcoinCoreEvidenceKind {
    const fn is_claim(self) -> bool {
        matches!(
            self,
            Self::ClaimRevealed | Self::ClaimConfirming | Self::ClaimFinalized
        )
    }
}

/// Version-1 canonical public evidence from the typed Bitcoin Core adapter.
///
/// Construction accepts only the adapter's validated ready-funding or recognized
/// claim observations. Decoding repeats agreement, consensus-byte, transaction-ID,
/// witness, placement, and state-classification validation before returning a value.
#[derive(Clone, Debug, Eq, PartialEq)]
#[must_use]
pub struct BitcoinCoreEvidenceV1 {
    kind: BitcoinCoreEvidenceKind,
    agreement_commitment: [u8; 32],
    transaction: Transaction,
    confirmations: u32,
    block_hash: Option<BlockHash>,
    stable_tip: StableTip,
    public_claim_witness: Option<[u8; 64]>,
}

impl BitcoinCoreEvidenceV1 {
    /// Captures an exact funding observation that already reached signed policy.
    ///
    /// # Errors
    ///
    /// Rejects a state that no longer matches the exact agreement, signed
    /// confirmation policy, or canonical transaction identities.
    pub fn funding_ready(
        agreement: &BtcAgreementV1,
        observation: &ObservedFunding,
    ) -> Result<Self, BitcoinCoreEvidenceError> {
        let evidence = Self {
            kind: BitcoinCoreEvidenceKind::FundingReady,
            agreement_commitment: *agreement.agreement_commitment(),
            transaction: observation.transaction().clone(),
            confirmations: observation.confirmations(),
            block_hash: Some(observation.block_hash()),
            stable_tip: observation.stable_tip(),
            public_claim_witness: None,
        };
        evidence.validate(agreement)?;
        evidence.ensure_encodable()?;
        Ok(evidence)
    }

    /// Captures one recognized exact claim observation and its public witness.
    ///
    /// # Errors
    ///
    /// Rejects an unspent observation, a mismatched state classification, or any
    /// transaction, witness, agreement, placement, confirmation, or size mismatch.
    pub fn claim(
        agreement: &BtcAgreementV1,
        observation: &ClaimObservation,
    ) -> Result<Self, BitcoinCoreEvidenceError> {
        let (kind, observation) = match observation {
            ClaimObservation::Unspent => {
                return Err(BitcoinCoreEvidenceError::UnsupportedObservation);
            }
            ClaimObservation::Revealed(observation) => {
                (BitcoinCoreEvidenceKind::ClaimRevealed, observation)
            }
            ClaimObservation::Confirming(observation) => {
                (BitcoinCoreEvidenceKind::ClaimConfirming, observation)
            }
            ClaimObservation::Finalized(observation) => {
                (BitcoinCoreEvidenceKind::ClaimFinalized, observation)
            }
        };
        let public_claim_witness = exact_claim_witness(observation.transaction())?;
        let evidence = Self {
            kind,
            agreement_commitment: *agreement.agreement_commitment(),
            transaction: observation.transaction().clone(),
            confirmations: observation.confirmations(),
            block_hash: observation.block_hash(),
            stable_tip: observation.stable_tip(),
            public_claim_witness: Some(public_claim_witness),
        };
        evidence.validate(agreement)?;
        evidence.ensure_encodable()?;
        Ok(evidence)
    }

    /// Decodes canonical bounded JSON and revalidates it against one agreement.
    ///
    /// # Errors
    ///
    /// Rejects empty, oversized, malformed, unknown-field, noncanonical,
    /// unsupported-version, cross-kind, agreement, consensus-byte, ID, witness,
    /// confirmation, or placement mutations.
    pub fn decode(
        agreement: &BtcAgreementV1,
        bytes: &[u8],
    ) -> Result<Self, BitcoinCoreEvidenceError> {
        if bytes.len() > MAX_BITCOIN_CORE_EVIDENCE_BYTES {
            return Err(BitcoinCoreEvidenceError::Oversized {
                actual: bytes.len(),
                maximum: MAX_BITCOIN_CORE_EVIDENCE_BYTES,
            });
        }
        if bytes.is_empty() {
            return Err(BitcoinCoreEvidenceError::Malformed);
        }
        let wire: EvidenceWireV1 =
            serde_json::from_slice(bytes).map_err(|_| BitcoinCoreEvidenceError::Malformed)?;
        if wire.schema_version != EVIDENCE_SCHEMA_VERSION {
            return Err(BitcoinCoreEvidenceError::UnsupportedSchema(
                wire.schema_version,
            ));
        }
        let evidence = Self::from_wire(agreement, &wire)?;
        let canonical =
            serde_json::to_vec(&wire).map_err(|_| BitcoinCoreEvidenceError::Encoding)?;
        if canonical != bytes {
            return Err(BitcoinCoreEvidenceError::Noncanonical);
        }
        Ok(evidence)
    }

    /// Encodes the only canonical version-1 JSON representation.
    ///
    /// # Errors
    ///
    /// Fails if serialization fails or the encoded DTO exceeds the 64 KiB
    /// recovery-store chain-evidence bound.
    pub fn encode(&self) -> Result<Vec<u8>, BitcoinCoreEvidenceError> {
        let encoded =
            serde_json::to_vec(&self.to_wire()).map_err(|_| BitcoinCoreEvidenceError::Encoding)?;
        require_bounded(&encoded)?;
        Ok(encoded)
    }

    /// Semantic state proved by this record.
    #[must_use]
    pub const fn kind(&self) -> BitcoinCoreEvidenceKind {
        self.kind
    }

    /// Exact countersigned agreement commitment owning this evidence.
    #[must_use]
    pub const fn agreement_commitment(&self) -> &[u8; 32] {
        &self.agreement_commitment
    }

    /// Canonical consensus transaction reconstructed and revalidated from the DTO.
    #[must_use]
    pub const fn transaction(&self) -> &Transaction {
        &self.transaction
    }

    /// Canonical non-witness transaction identifier.
    #[must_use]
    pub fn transaction_id(&self) -> Txid {
        self.transaction.compute_txid()
    }

    /// Canonical witness transaction identifier.
    #[must_use]
    pub fn witness_transaction_id(&self) -> Wtxid {
        self.transaction.compute_wtxid()
    }

    /// Active-chain confirmations at the bracketed stable tip.
    #[must_use]
    pub const fn confirmations(&self) -> u32 {
        self.confirmations
    }

    /// Containing active-chain block, absent only for a mempool-revealed claim.
    #[must_use]
    pub const fn block_hash(&self) -> Option<BlockHash> {
        self.block_hash
    }

    /// Stable active-chain tip bracketing the adapter observation.
    #[must_use]
    pub const fn stable_tip(&self) -> StableTip {
        self.stable_tip
    }

    /// Exact public 64-byte Bitcoin claim witness, never an adaptor scalar.
    #[must_use]
    pub const fn claim_public_witness(&self) -> Option<&[u8; 64]> {
        self.public_claim_witness.as_ref()
    }

    fn from_wire(
        agreement: &BtcAgreementV1,
        wire: &EvidenceWireV1,
    ) -> Result<Self, BitcoinCoreEvidenceError> {
        let agreement_commitment = decode_lower_hex::<32>(&wire.agreement_commitment)?;
        if agreement_commitment != *agreement.agreement_commitment() {
            return Err(BitcoinCoreEvidenceError::AgreementMismatch);
        }
        let transaction = decode_transaction(&wire.transaction)?;
        let block_hash = wire
            .block_hash
            .as_deref()
            .map(parse_block_hash)
            .transpose()?;
        let stable_tip = StableTip {
            block_hash: parse_block_hash(&wire.stable_tip.block_hash)?,
            height: wire.stable_tip.height,
        };
        let public_claim_witness = wire
            .public_claim_witness
            .as_deref()
            .map(decode_lower_hex::<64>)
            .transpose()?;
        let evidence = Self {
            kind: wire.kind,
            agreement_commitment,
            transaction,
            confirmations: wire.confirmations,
            block_hash,
            stable_tip,
            public_claim_witness,
        };
        evidence.validate(agreement)?;
        Ok(evidence)
    }

    fn to_wire(&self) -> EvidenceWireV1 {
        EvidenceWireV1 {
            schema_version: EVIDENCE_SCHEMA_VERSION,
            kind: self.kind,
            agreement_commitment: hex::encode(self.agreement_commitment),
            transaction: TransactionWireV1 {
                consensus_hex: hex::encode(serialize(&self.transaction)),
                transaction_id: self.transaction.compute_txid().to_string(),
                witness_transaction_id: self.transaction.compute_wtxid().to_string(),
            },
            confirmations: self.confirmations,
            block_hash: self.block_hash.map(|hash| hash.to_string()),
            stable_tip: StableTipWireV1 {
                block_hash: self.stable_tip.block_hash().to_string(),
                height: self.stable_tip.height(),
            },
            public_claim_witness: self.public_claim_witness.map(hex::encode),
        }
    }

    fn validate(&self, agreement: &BtcAgreementV1) -> Result<(), BitcoinCoreEvidenceError> {
        if self.agreement_commitment != *agreement.agreement_commitment() {
            return Err(BitcoinCoreEvidenceError::AgreementMismatch);
        }
        if self.confirmations > self.stable_tip.height().saturating_add(1) {
            return Err(BitcoinCoreEvidenceError::ObservationStateMismatch);
        }
        match self.kind {
            BitcoinCoreEvidenceKind::FundingReady => {
                if self.public_claim_witness.is_some()
                    || self.block_hash.is_none()
                    || self.confirmations < agreement.required_bitcoin_confirmations()
                {
                    return Err(BitcoinCoreEvidenceError::ObservationStateMismatch);
                }
                validate_funding(agreement, &self.transaction)?;
            }
            BitcoinCoreEvidenceKind::ClaimRevealed => {
                if self.confirmations != 0 || self.block_hash.is_some() {
                    return Err(BitcoinCoreEvidenceError::ObservationStateMismatch);
                }
                self.validate_claim(agreement)?;
            }
            BitcoinCoreEvidenceKind::ClaimConfirming => {
                if self.confirmations == 0
                    || self.confirmations >= agreement.required_bitcoin_confirmations()
                    || self.block_hash.is_none()
                {
                    return Err(BitcoinCoreEvidenceError::ObservationStateMismatch);
                }
                self.validate_claim(agreement)?;
            }
            BitcoinCoreEvidenceKind::ClaimFinalized => {
                if self.confirmations < agreement.required_bitcoin_confirmations()
                    || self.block_hash.is_none()
                {
                    return Err(BitcoinCoreEvidenceError::ObservationStateMismatch);
                }
                self.validate_claim(agreement)?;
            }
        }
        Ok(())
    }

    fn validate_claim(&self, agreement: &BtcAgreementV1) -> Result<(), BitcoinCoreEvidenceError> {
        if !self.kind.is_claim() {
            return Err(BitcoinCoreEvidenceError::ObservationStateMismatch);
        }
        validate_exact_claim::<Infallible>(agreement, &self.transaction)
            .map_err(|_| BitcoinCoreEvidenceError::TransactionMismatch)?;
        let witness = exact_claim_witness(&self.transaction)?;
        if self.public_claim_witness != Some(witness) {
            return Err(BitcoinCoreEvidenceError::TransactionMismatch);
        }
        Ok(())
    }

    fn ensure_encodable(&self) -> Result<(), BitcoinCoreEvidenceError> {
        let encoded =
            serde_json::to_vec(&self.to_wire()).map_err(|_| BitcoinCoreEvidenceError::Encoding)?;
        require_bounded(&encoded)
    }
}

/// Failure to construct, encode, decode, or revalidate durable Core evidence.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum BitcoinCoreEvidenceError {
    /// The observation does not contain a durable affirmative fact.
    #[error("Bitcoin Core observation has no encodable affirmative evidence")]
    UnsupportedObservation,
    /// The encoded record exceeds the recovery-store raw evidence bound.
    #[error("Bitcoin Core evidence is {actual} bytes; maximum is {maximum}")]
    Oversized {
        /// Actual encoded byte length.
        actual: usize,
        /// Maximum accepted encoded byte length.
        maximum: usize,
    },
    /// JSON syntax, shape, fields, or fixed-width values are malformed.
    #[error("Bitcoin Core evidence is malformed")]
    Malformed,
    /// Well-formed evidence differs from the one canonical JSON encoding.
    #[error("Bitcoin Core evidence is not canonically encoded")]
    Noncanonical,
    /// The record uses an unsupported schema version.
    #[error("unsupported Bitcoin Core evidence schema {0}")]
    UnsupportedSchema(u16),
    /// The record is bound to another countersigned agreement.
    #[error("Bitcoin Core evidence agreement commitment mismatch")]
    AgreementMismatch,
    /// Consensus bytes, transaction identities, agreement terms, or witness differ.
    #[error("Bitcoin Core evidence transaction mismatch")]
    TransactionMismatch,
    /// Kind, confirmation, block, tip, or witness shape is internally inconsistent.
    #[error("Bitcoin Core evidence observation state mismatch")]
    ObservationStateMismatch,
    /// Canonical serialization unexpectedly failed.
    #[error("Bitcoin Core evidence encoding failed")]
    Encoding,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct EvidenceWireV1 {
    schema_version: u16,
    kind: BitcoinCoreEvidenceKind,
    agreement_commitment: String,
    transaction: TransactionWireV1,
    confirmations: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    block_hash: Option<String>,
    stable_tip: StableTipWireV1,
    #[serde(skip_serializing_if = "Option::is_none")]
    public_claim_witness: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct TransactionWireV1 {
    consensus_hex: String,
    transaction_id: String,
    witness_transaction_id: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct StableTipWireV1 {
    block_hash: String,
    height: u32,
}

fn validate_funding(
    agreement: &BtcAgreementV1,
    transaction: &Transaction,
) -> Result<(), BitcoinCoreEvidenceError> {
    let funding = agreement.funding_terms();
    let expected_transaction_id = Txid::from_byte_array(*funding.transaction_id());
    let output = transaction
        .output
        .get(
            usize::try_from(funding.output_index())
                .map_err(|_| BitcoinCoreEvidenceError::TransactionMismatch)?,
        )
        .ok_or(BitcoinCoreEvidenceError::TransactionMismatch)?;
    if transaction.compute_txid() != expected_transaction_id
        || output.value.to_sat() != funding.value_sat()
        || output.script_pubkey.as_bytes() != agreement.p2tr_contract().script_pubkey_bytes()
    {
        return Err(BitcoinCoreEvidenceError::TransactionMismatch);
    }
    Ok(())
}

fn exact_claim_witness(transaction: &Transaction) -> Result<[u8; 64], BitcoinCoreEvidenceError> {
    let [input] = transaction.input.as_slice() else {
        return Err(BitcoinCoreEvidenceError::TransactionMismatch);
    };
    let mut witness = input.witness.iter();
    let signature = witness
        .next()
        .and_then(|bytes| bytes.try_into().ok())
        .ok_or(BitcoinCoreEvidenceError::TransactionMismatch)?;
    if witness.next().is_some() {
        return Err(BitcoinCoreEvidenceError::TransactionMismatch);
    }
    Ok(signature)
}

fn decode_transaction(wire: &TransactionWireV1) -> Result<Transaction, BitcoinCoreEvidenceError> {
    if wire.consensus_hex.is_empty()
        || !wire.consensus_hex.len().is_multiple_of(2)
        || !is_lower_hex(&wire.consensus_hex)
    {
        return Err(BitcoinCoreEvidenceError::TransactionMismatch);
    }
    let bytes = hex::decode(&wire.consensus_hex)
        .map_err(|_| BitcoinCoreEvidenceError::TransactionMismatch)?;
    let transaction: Transaction =
        deserialize(&bytes).map_err(|_| BitcoinCoreEvidenceError::TransactionMismatch)?;
    if serialize(&transaction) != bytes
        || transaction.compute_txid().to_string() != wire.transaction_id
        || transaction.compute_wtxid().to_string() != wire.witness_transaction_id
    {
        return Err(BitcoinCoreEvidenceError::TransactionMismatch);
    }
    Ok(transaction)
}

fn parse_block_hash(value: &str) -> Result<BlockHash, BitcoinCoreEvidenceError> {
    if value.len() != 64 || !is_lower_hex(value) {
        return Err(BitcoinCoreEvidenceError::Malformed);
    }
    let hash = BlockHash::from_str(value).map_err(|_| BitcoinCoreEvidenceError::Malformed)?;
    if hash.to_string() != value {
        return Err(BitcoinCoreEvidenceError::Malformed);
    }
    Ok(hash)
}

fn decode_lower_hex<const N: usize>(value: &str) -> Result<[u8; N], BitcoinCoreEvidenceError> {
    if value.len() != N.saturating_mul(2) || !is_lower_hex(value) {
        return Err(BitcoinCoreEvidenceError::Malformed);
    }
    hex::decode(value)
        .ok()
        .and_then(|bytes| bytes.try_into().ok())
        .ok_or(BitcoinCoreEvidenceError::Malformed)
}

fn require_bounded(bytes: &[u8]) -> Result<(), BitcoinCoreEvidenceError> {
    if bytes.len() > MAX_BITCOIN_CORE_EVIDENCE_BYTES {
        Err(BitcoinCoreEvidenceError::Oversized {
            actual: bytes.len(),
            maximum: MAX_BITCOIN_CORE_EVIDENCE_BYTES,
        })
    } else {
        Ok(())
    }
}

fn is_lower_hex(value: &str) -> bool {
    value
        .bytes()
        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
