//! Canonical LEZ revealing-claim evidence derived from one primitive node snapshot.

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use std::fmt;
use subtle::ConstantTimeEq;

use crate::{
    ClaimError, ClaimPreimage, ClaimStepV1, LezAssetV1, LezCustodySnapshotV1, LezEnvironmentV1,
    LezEscrowMetadataSnapshotV1, LezEscrowStatusV1, LezInclusionStatusV1, LezObservationError,
    LezStableTipV1, PreparedClaimSubmissionV1, RevealingClaimEvidenceV1, ZecAgreementV1,
    lez_observation::{
        ExpectedAccountBinding, custody_matches, expected_escrow_metadata,
        validate_lez_chain_position_parts,
    },
};

/// Stable schema for the secret-free primitive claim snapshot embedded in recovery records.
pub const CANONICAL_LEZ_CLAIM_SNAPSHOT_SCHEMA_V1: u16 = 1;

/// Exact generated escrow claim instruction decoded from the canonical public transaction.
pub enum LezClaimInstructionV1 {
    /// Native authenticated-transfer claim.
    Native {
        /// Exact on-chain swap identifier.
        swap_id: [u8; 32],
        /// Transient preimage decoded from the public instruction.
        preimage: ClaimPreimage,
    },
    /// Fungible-token claim.
    Token {
        /// Exact on-chain swap identifier.
        swap_id: [u8; 32],
        /// Transient preimage decoded from the public instruction.
        preimage: ClaimPreimage,
    },
}

impl fmt::Debug for LezClaimInstructionV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (kind, swap_id) = match self {
            Self::Native { swap_id, .. } => ("Native", swap_id),
            Self::Token { swap_id, .. } => ("Token", swap_id),
        };
        formatter
            .debug_struct(kind)
            .field("swap_id", swap_id)
            .field("preimage", &"[REDACTED]")
            .finish()
    }
}

impl LezClaimInstructionV1 {
    const fn swap_id(&self) -> &[u8; 32] {
        match self {
            Self::Native { swap_id, .. } | Self::Token { swap_id, .. } => swap_id,
        }
    }

    const fn preimage(&self) -> &ClaimPreimage {
        match self {
            Self::Native { preimage, .. } | Self::Token { preimage, .. } => preimage,
        }
    }

    fn into_preimage(self) -> ClaimPreimage {
        match self {
            Self::Native { preimage, .. } | Self::Token { preimage, .. } => preimage,
        }
    }
}

/// Primitive facts independently decoded from one canonical public claim transaction.
pub struct LezClaimTransactionSnapshotV1 {
    reported_transaction_id: [u8; 32],
    canonical_transaction_hash: [u8; 32],
    program_id: [u32; 8],
    signer: [u8; 32],
    accounts: Vec<[u8; 32]>,
    instruction: LezClaimInstructionV1,
    is_public: bool,
    signature_valid: bool,
    inclusion_height: u64,
    inclusion_block_hash: [u8; 32],
    canonical_block_hash: [u8; 32],
    inclusion_status: LezInclusionStatusV1,
}

impl LezClaimTransactionSnapshotV1 {
    /// Creates untrusted primitive RPC and official-decoder results.
    ///
    /// `reported_transaction_id` is the lookup identity returned by the node;
    /// `canonical_transaction_hash` must be recomputed from the decoded official LEZ
    /// transaction type by the adapter. Validation rejects disagreement between them.
    #[allow(clippy::too_many_arguments)]
    #[must_use]
    pub fn new(
        reported_transaction_id: [u8; 32],
        canonical_transaction_hash: [u8; 32],
        program_id: [u32; 8],
        signer: [u8; 32],
        accounts: Vec<[u8; 32]>,
        instruction: LezClaimInstructionV1,
        is_public: bool,
        signature_valid: bool,
        inclusion_height: u64,
        inclusion_block_hash: [u8; 32],
        canonical_block_hash: [u8; 32],
        inclusion_status: LezInclusionStatusV1,
    ) -> Self {
        Self {
            reported_transaction_id,
            canonical_transaction_hash,
            program_id,
            signer,
            accounts,
            instruction,
            is_public,
            signature_valid,
            inclusion_height,
            inclusion_block_hash,
            canonical_block_hash,
            inclusion_status,
        }
    }
}

impl fmt::Debug for LezClaimTransactionSnapshotV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LezClaimTransactionSnapshotV1")
            .field("reported_transaction_id", &self.reported_transaction_id)
            .field(
                "canonical_transaction_hash",
                &self.canonical_transaction_hash,
            )
            .field("program_id", &self.program_id)
            .field("signer", &self.signer)
            .field("accounts", &self.accounts)
            .field("instruction", &self.instruction)
            .field("is_public", &self.is_public)
            .field("signature_valid", &self.signature_valid)
            .field("inclusion_height", &self.inclusion_height)
            .field("inclusion_block_hash", &self.inclusion_block_hash)
            .field("canonical_block_hash", &self.canonical_block_hash)
            .field("inclusion_status", &self.inclusion_status)
            .finish()
    }
}

/// One untrusted, stable-query LEZ revealing-claim snapshot.
pub struct LezClaimNodeSnapshotV1 {
    environment: LezEnvironmentV1,
    channel_id: [u8; 32],
    genesis_block_hash: [u8; 32],
    tip: LezStableTipV1,
    transaction: LezClaimTransactionSnapshotV1,
    metadata_program_owner: [u32; 8],
    metadata_account: [u8; 32],
    metadata: LezEscrowMetadataSnapshotV1,
    custody_account: [u8; 32],
    custody: LezCustodySnapshotV1,
}

impl LezClaimNodeSnapshotV1 {
    /// Creates primitive results from one bracketing LEZ claim query.
    #[allow(clippy::too_many_arguments)]
    #[must_use]
    pub const fn new(
        environment: LezEnvironmentV1,
        channel_id: [u8; 32],
        genesis_block_hash: [u8; 32],
        tip: LezStableTipV1,
        transaction: LezClaimTransactionSnapshotV1,
        metadata_program_owner: [u32; 8],
        metadata_account: [u8; 32],
        metadata: LezEscrowMetadataSnapshotV1,
        custody_account: [u8; 32],
        custody: LezCustodySnapshotV1,
    ) -> Self {
        Self {
            environment,
            channel_id,
            genesis_block_hash,
            tip,
            transaction,
            metadata_program_owner,
            metadata_account,
            metadata,
            custody_account,
            custody,
        }
    }
}

impl fmt::Debug for LezClaimNodeSnapshotV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LezClaimNodeSnapshotV1")
            .field("environment", &self.environment)
            .field("channel_id", &self.channel_id)
            .field("genesis_block_hash", &self.genesis_block_hash)
            .field("tip", &self.tip)
            .field("transaction", &self.transaction)
            .field("metadata_program_owner", &self.metadata_program_owner)
            .field("metadata_account", &self.metadata_account)
            .field("metadata", &self.metadata)
            .field("custody_account", &self.custody_account)
            .field("custody", &self.custody)
            .finish()
    }
}

/// Secret-free primitive copy of a previously validated canonical LEZ claim snapshot.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CanonicalLezClaimSnapshotRecordV1 {
    schema_version: u16,
    environment: LezEnvironmentV1,
    channel_id: [u8; 32],
    genesis_block_hash: [u8; 32],
    tip: LezStableTipV1,
    reported_transaction_id: [u8; 32],
    canonical_transaction_hash: [u8; 32],
    program_id: [u32; 8],
    signer: [u8; 32],
    accounts: Vec<[u8; 32]>,
    instruction_kind: LezClaimInstructionKindV1,
    instruction_swap_id: [u8; 32],
    preimage_digest: [u8; 32],
    is_public: bool,
    signature_valid: bool,
    inclusion_height: u64,
    inclusion_block_hash: [u8; 32],
    canonical_block_hash: [u8; 32],
    inclusion_status: LezInclusionStatusV1,
    metadata_program_owner: [u32; 8],
    metadata_account: [u8; 32],
    metadata: LezEscrowMetadataSnapshotV1,
    custody_account: [u8; 32],
    custody: LezCustodySnapshotV1,
}

/// Secret-free decoded LEZ claim instruction kind.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum LezClaimInstructionKindV1 {
    /// Native authenticated-transfer claim.
    Native,
    /// Fungible-token claim.
    Token,
}

impl CanonicalLezClaimSnapshotRecordV1 {
    fn from_snapshot(snapshot: &LezClaimNodeSnapshotV1) -> Self {
        let transaction = &snapshot.transaction;
        let instruction_kind = match transaction.instruction {
            LezClaimInstructionV1::Native { .. } => LezClaimInstructionKindV1::Native,
            LezClaimInstructionV1::Token { .. } => LezClaimInstructionKindV1::Token,
        };
        Self {
            schema_version: CANONICAL_LEZ_CLAIM_SNAPSHOT_SCHEMA_V1,
            environment: snapshot.environment,
            channel_id: snapshot.channel_id,
            genesis_block_hash: snapshot.genesis_block_hash,
            tip: snapshot.tip,
            reported_transaction_id: transaction.reported_transaction_id,
            canonical_transaction_hash: transaction.canonical_transaction_hash,
            program_id: transaction.program_id,
            signer: transaction.signer,
            accounts: transaction.accounts.clone(),
            instruction_kind,
            instruction_swap_id: *transaction.instruction.swap_id(),
            preimage_digest: Sha256::digest(transaction.instruction.preimage().expose_secret())
                .into(),
            is_public: transaction.is_public,
            signature_valid: transaction.signature_valid,
            inclusion_height: transaction.inclusion_height,
            inclusion_block_hash: transaction.inclusion_block_hash,
            canonical_block_hash: transaction.canonical_block_hash,
            inclusion_status: transaction.inclusion_status,
            metadata_program_owner: snapshot.metadata_program_owner,
            metadata_account: snapshot.metadata_account,
            metadata: snapshot.metadata.clone(),
            custody_account: snapshot.custody_account,
            custody: snapshot.custody,
        }
    }

    /// Primitive snapshot schema.
    #[must_use]
    pub const fn schema_version(&self) -> u16 {
        self.schema_version
    }

    fn reconstruct(
        &self,
        preimage: ClaimPreimage,
    ) -> Result<LezClaimNodeSnapshotV1, LezClaimObservationError> {
        if self.schema_version != CANONICAL_LEZ_CLAIM_SNAPSHOT_SCHEMA_V1 {
            return Err(LezClaimObservationError::UnsupportedSnapshotSchema(
                self.schema_version,
            ));
        }
        let actual_digest: [u8; 32] = Sha256::digest(preimage.expose_secret()).into();
        if actual_digest.ct_eq(&self.preimage_digest).unwrap_u8() != 1 {
            return Err(LezClaimObservationError::Claim(
                ClaimError::SecretDigestMismatch,
            ));
        }
        let instruction = match self.instruction_kind {
            LezClaimInstructionKindV1::Native => LezClaimInstructionV1::Native {
                swap_id: self.instruction_swap_id,
                preimage,
            },
            LezClaimInstructionKindV1::Token => LezClaimInstructionV1::Token {
                swap_id: self.instruction_swap_id,
                preimage,
            },
        };
        Ok(LezClaimNodeSnapshotV1::new(
            self.environment,
            self.channel_id,
            self.genesis_block_hash,
            self.tip,
            LezClaimTransactionSnapshotV1::new(
                self.reported_transaction_id,
                self.canonical_transaction_hash,
                self.program_id,
                self.signer,
                self.accounts.clone(),
                instruction,
                self.is_public,
                self.signature_valid,
                self.inclusion_height,
                self.inclusion_block_hash,
                self.canonical_block_hash,
                self.inclusion_status,
            ),
            self.metadata_program_owner,
            self.metadata_account,
            self.metadata.clone(),
            self.custody_account,
            self.custody,
        ))
    }
}

impl RevealingClaimEvidenceV1 {
    /// Validates a counterparty LEZ reveal from independently decoded primitive node facts.
    ///
    /// # Errors
    ///
    /// Rejects transaction-hash disagreement, chain/tip drift, unstable or noncanonical
    /// inclusion, wrong agreement/program/actor/instruction/accounts, nonterminal escrow state,
    /// nonempty custody, insufficient depth, or the wrong revealed preimage.
    pub fn from_lez_claim_snapshot(
        agreement: &ZecAgreementV1,
        snapshot: LezClaimNodeSnapshotV1,
    ) -> Result<Self, LezClaimObservationError> {
        validate_snapshot(agreement, None, snapshot)
    }

    /// Validates an owned LEZ reveal and binds it to the durable prepared identity.
    ///
    /// # Errors
    ///
    /// In addition to [`Self::from_lez_claim_snapshot`], rejects a non-LEZ plan or a
    /// canonical transaction hash different from the protected prepared submission.
    pub fn from_prepared_lez_claim_snapshot(
        agreement: &ZecAgreementV1,
        prepared: &PreparedClaimSubmissionV1,
        snapshot: LezClaimNodeSnapshotV1,
    ) -> Result<Self, LezClaimObservationError> {
        if prepared.step() != ClaimStepV1::RevealingLez {
            return Err(LezClaimObservationError::PreparedStepMismatch);
        }
        validate_snapshot(agreement, Some(prepared.expected_submission_id()), snapshot)
    }

    pub(crate) fn from_lez_claim_snapshot_record(
        agreement: &ZecAgreementV1,
        expected_submission_id: Option<&[u8; 32]>,
        record: &CanonicalLezClaimSnapshotRecordV1,
        preimage: ClaimPreimage,
    ) -> Result<Self, LezClaimObservationError> {
        validate_snapshot(
            agreement,
            expected_submission_id,
            record.reconstruct(preimage)?,
        )
    }
}

fn validate_snapshot(
    agreement: &ZecAgreementV1,
    expected_submission_id: Option<&[u8; 32]>,
    snapshot: LezClaimNodeSnapshotV1,
) -> Result<RevealingClaimEvidenceV1, LezClaimObservationError> {
    let confirmations = validate_lez_chain_position_parts(
        agreement,
        snapshot.environment,
        &snapshot.channel_id,
        &snapshot.genesis_block_hash,
        &snapshot.tip,
        snapshot.transaction.inclusion_height,
        &snapshot.transaction.inclusion_block_hash,
        &snapshot.transaction.canonical_block_hash,
    )?;
    let transaction = &snapshot.transaction;
    if transaction.reported_transaction_id != transaction.canonical_transaction_hash {
        return Err(LezClaimObservationError::TransactionIdentityMismatch);
    }
    if expected_submission_id
        .is_some_and(|expected| expected != &transaction.canonical_transaction_hash)
    {
        return Err(LezClaimObservationError::PreparedIdentityMismatch);
    }
    validate_claim_inclusion_policy(snapshot.environment, transaction.inclusion_status)?;
    let expected = ExpectedAccountBinding::from_agreement(agreement);
    validate_transaction(agreement, transaction, &expected)?;
    validate_accounts(agreement, &snapshot, &expected)?;
    let record = CanonicalLezClaimSnapshotRecordV1::from_snapshot(&snapshot);
    let preimage = snapshot.transaction.instruction.into_preimage();
    RevealingClaimEvidenceV1::from_validated_lez_snapshot_parts(
        agreement,
        record.reported_transaction_id,
        canonical_hash_string(&record.canonical_transaction_hash),
        confirmations.get(),
        preimage,
        record,
    )
    .map_err(LezClaimObservationError::Claim)
}

fn validate_claim_inclusion_policy(
    environment: LezEnvironmentV1,
    status: LezInclusionStatusV1,
) -> Result<(), LezClaimObservationError> {
    match (environment, status) {
        (LezEnvironmentV1::DeterministicLocalV0_2, _)
        | (LezEnvironmentV1::PublicTestnetV0_2, LezInclusionStatusV1::Finalized) => Ok(()),
        (LezEnvironmentV1::PublicTestnetV0_2, _) => {
            Err(LezClaimObservationError::UnstableInclusionStatus)
        }
    }
}

fn validate_transaction(
    agreement: &ZecAgreementV1,
    transaction: &LezClaimTransactionSnapshotV1,
    expected: &ExpectedAccountBinding,
) -> Result<(), LezClaimObservationError> {
    let instruction_kind_matches = matches!(
        (agreement.lez_terms().asset(), &transaction.instruction),
        (
            LezAssetV1::Native { .. },
            LezClaimInstructionV1::Native { .. }
        ) | (
            LezAssetV1::FungibleToken { .. },
            LezClaimInstructionV1::Token { .. }
        )
    );
    if !transaction.is_public
        || !transaction.signature_valid
        || transaction.program_id != *agreement.lez_terms().escrow_program_id()
        || transaction.signer != expected.claimant
        || !instruction_kind_matches
        || transaction.instruction.swap_id() != agreement.onchain_swap_id()
    {
        return Err(LezClaimObservationError::TransactionBindingMismatch);
    }
    crate::claim::validate_preimage(agreement, transaction.instruction.preimage())
        .map_err(LezClaimObservationError::Claim)
}

fn validate_accounts(
    agreement: &ZecAgreementV1,
    snapshot: &LezClaimNodeSnapshotV1,
    expected: &ExpectedAccountBinding,
) -> Result<(), LezClaimObservationError> {
    let terms = agreement.lez_terms();
    let expected_accounts = match terms.asset() {
        LezAssetV1::Native { .. } => vec![
            *terms.metadata_account(),
            *terms.custody_account(),
            expected.claimant,
        ],
        LezAssetV1::FungibleToken { .. } => vec![
            *terms.metadata_account(),
            *terms.custody_account(),
            expected.claimant,
            expected.claimant_asset,
        ],
    };
    if snapshot.transaction.accounts != expected_accounts {
        return Err(LezClaimObservationError::TransactionAccountsMismatch);
    }
    let expected_metadata =
        expected_escrow_metadata(agreement, expected, LezEscrowStatusV1::Claimed);
    if snapshot.metadata_program_owner != *terms.escrow_program_id()
        || snapshot.metadata_account != *terms.metadata_account()
        || snapshot.metadata != expected_metadata
    {
        return Err(LezClaimObservationError::MetadataBindingMismatch);
    }
    if !custody_matches(agreement, &snapshot.custody_account, snapshot.custody, 0) {
        return Err(LezClaimObservationError::CustodyBindingMismatch);
    }
    Ok(())
}

fn canonical_hash_string(hash: &[u8; 32]) -> Box<str> {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(64);
    for byte in hash {
        output.push(char::from(HEX[usize::from(byte >> 4)]));
        output.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    output.into_boxed_str()
}

/// Failure validating or replaying a canonical LEZ revealing-claim snapshot.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum LezClaimObservationError {
    /// Shared chain identity, stable-tip, or canonical inclusion validation failed.
    #[error(transparent)]
    Chain(#[from] LezObservationError),
    /// The node lookup ID differs from the official decoded transaction hash.
    #[error("LEZ claim RPC identity differs from its canonical decoded transaction hash")]
    TransactionIdentityMismatch,
    /// The protected prepared plan names a different canonical transaction.
    #[error("canonical LEZ claim identity differs from the protected prepared submission")]
    PreparedIdentityMismatch,
    /// A Zcash follow-up plan was supplied to the LEZ claim validator.
    #[error("prepared claim step is not the revealing LEZ claim")]
    PreparedStepMismatch,
    /// Bedrock has not yet marked the canonical inclusion safe.
    #[error("LEZ claim inclusion is still pending")]
    UnstableInclusionStatus,
    /// Public kind, signature, program, claimant, instruction, or swap ID differs.
    #[error("LEZ claim transaction does not match the signed agreement")]
    TransactionBindingMismatch,
    /// Generated claim instruction account order differs.
    #[error("LEZ claim transaction accounts do not match the generated client")]
    TransactionAccountsMismatch,
    /// Terminal metadata address, owner, or decoded contents differ.
    #[error("claimed LEZ escrow metadata does not match the signed agreement")]
    MetadataBindingMismatch,
    /// Claimed custody identity, owner, definition, or empty balance differs.
    #[error("claimed LEZ escrow custody is not canonically empty")]
    CustodyBindingMismatch,
    /// Durable primitive snapshot schema is unsupported.
    #[error("unsupported canonical LEZ claim snapshot schema {0}")]
    UnsupportedSnapshotSchema(u16),
    /// Claim depth or preimage validation failed.
    #[error(transparent)]
    Claim(#[from] ClaimError),
}

#[cfg(test)]
mod tests {
    use super::{LezClaimObservationError, validate_claim_inclusion_policy};
    use crate::{LezEnvironmentV1, LezInclusionStatusV1};

    #[test]
    fn claim_finality_policy_separates_deterministic_standalone_from_public_bedrock() {
        for status in [
            LezInclusionStatusV1::Pending,
            LezInclusionStatusV1::Safe,
            LezInclusionStatusV1::Finalized,
        ] {
            assert_eq!(
                validate_claim_inclusion_policy(LezEnvironmentV1::DeterministicLocalV0_2, status),
                Ok(()),
                "deterministic status {status:?}",
            );
        }
        for status in [LezInclusionStatusV1::Pending, LezInclusionStatusV1::Safe] {
            assert_eq!(
                validate_claim_inclusion_policy(LezEnvironmentV1::PublicTestnetV0_2, status),
                Err(LezClaimObservationError::UnstableInclusionStatus),
                "public status {status:?}",
            );
        }
        assert_eq!(
            validate_claim_inclusion_policy(
                LezEnvironmentV1::PublicTestnetV0_2,
                LezInclusionStatusV1::Finalized,
            ),
            Ok(()),
        );
    }
}
