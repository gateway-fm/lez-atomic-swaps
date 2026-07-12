//! Canonical LEZ escrow observation validated from one stable RPC snapshot.

use std::num::NonZeroU32;

use lez_swap_core::{Participant, SwapDirection};
use serde::{Deserialize, Serialize};

use crate::{LezAssetV1, LezEnvironmentV1, ZecAgreementV1};

/// Bedrock inclusion status returned for the observed transaction.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum LezInclusionStatusV1 {
    /// Included on the stable canonical sequencer chain.
    Safe,
    /// Included and finalized by Bedrock.
    Finalized,
}

/// Two tip reads bracketing all transaction, block, metadata, and custody RPCs.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LezStableTipV1 {
    before_hash: [u8; 32],
    before_height: u64,
    after_hash: [u8; 32],
    after_height: u64,
}

impl LezStableTipV1 {
    /// Creates primitive bracketing tip reads.
    #[must_use]
    pub const fn new(
        before_hash: [u8; 32],
        before_height: u64,
        after_hash: [u8; 32],
        after_height: u64,
    ) -> Self {
        Self {
            before_hash,
            before_height,
            after_hash,
            after_height,
        }
    }
}

/// Primitive facts decoded from the canonical public fund transaction.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LezFundTransactionSnapshotV1 {
    transaction_id: [u8; 32],
    program_id: [u32; 8],
    signer: [u8; 32],
    accounts: Vec<[u8; 32]>,
    swap_id: [u8; 32],
    is_public: bool,
    signature_valid: bool,
    inclusion_height: u64,
    inclusion_block_hash: [u8; 32],
    canonical_block_hash: [u8; 32],
    inclusion_status: LezInclusionStatusV1,
}

impl LezFundTransactionSnapshotV1 {
    /// Creates an untrusted decoded fund-transaction snapshot.
    #[allow(clippy::too_many_arguments)]
    #[must_use]
    pub fn new(
        transaction_id: [u8; 32],
        program_id: [u32; 8],
        signer: [u8; 32],
        accounts: Vec<[u8; 32]>,
        swap_id: [u8; 32],
        is_public: bool,
        signature_valid: bool,
        inclusion_height: u64,
        inclusion_block_hash: [u8; 32],
        canonical_block_hash: [u8; 32],
        inclusion_status: LezInclusionStatusV1,
    ) -> Self {
        Self {
            transaction_id,
            program_id,
            signer,
            accounts,
            swap_id,
            is_public,
            signature_valid,
            inclusion_height,
            inclusion_block_hash,
            canonical_block_hash,
            inclusion_status,
        }
    }
}

/// Exact decoded SPEL escrow status.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum LezEscrowStatusV1 {
    /// Initialized but empty.
    Empty,
    /// Holding the exact agreement amount.
    Funded,
    /// Claimed by the agreement claimant.
    Claimed,
    /// Refunded to the agreement depositor.
    Refunded,
}

/// Primitive decoded contents of the SPEL `EscrowMetadata` account.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LezEscrowMetadataSnapshotV1 {
    version: u8,
    swap_id: [u8; 32],
    terms_hash: [u8; 32],
    secret_digest: [u8; 32],
    depositor: [u8; 32],
    depositor_asset: [u8; 32],
    claimant: [u8; 32],
    claimant_asset: [u8; 32],
    custody: [u8; 32],
    asset_program: [u32; 8],
    custody_program: [u32; 8],
    asset_definition: [u8; 32],
    amount: u128,
    refund_at: u64,
    status: LezEscrowStatusV1,
}

impl LezEscrowMetadataSnapshotV1 {
    /// Creates an untrusted decoded SPEL metadata snapshot.
    #[allow(clippy::too_many_arguments)]
    #[must_use]
    pub const fn new(
        version: u8,
        swap_id: [u8; 32],
        terms_hash: [u8; 32],
        secret_digest: [u8; 32],
        depositor: [u8; 32],
        depositor_asset: [u8; 32],
        claimant: [u8; 32],
        claimant_asset: [u8; 32],
        custody: [u8; 32],
        asset_program: [u32; 8],
        custody_program: [u32; 8],
        asset_definition: [u8; 32],
        amount: u128,
        refund_at: u64,
        status: LezEscrowStatusV1,
    ) -> Self {
        Self {
            version,
            swap_id,
            terms_hash,
            secret_digest,
            depositor,
            depositor_asset,
            claimant,
            claimant_asset,
            custody,
            asset_program,
            custody_program,
            asset_definition,
            amount,
            refund_at,
            status,
        }
    }
}

/// Primitive funded custody account state.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum LezCustodySnapshotV1 {
    /// Native authenticated-transfer custody.
    Native {
        /// Actual account program owner.
        program_owner: [u32; 8],
        /// Exact native balance.
        balance: u128,
    },
    /// Fungible-token custody ATA.
    Token {
        /// Actual Token program owner.
        program_owner: [u32; 8],
        /// Decoded token definition.
        definition: [u8; 32],
        /// Exact token balance.
        balance: u128,
    },
}

/// One untrusted, stable-query LEZ RPC snapshot.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LezNodeSnapshotV1 {
    environment: LezEnvironmentV1,
    channel_id: [u8; 32],
    genesis_block_hash: [u8; 32],
    tip: LezStableTipV1,
    transaction: LezFundTransactionSnapshotV1,
    metadata_program_owner: [u32; 8],
    metadata_account: [u8; 32],
    metadata: LezEscrowMetadataSnapshotV1,
    custody_account: [u8; 32],
    custody: LezCustodySnapshotV1,
}

impl LezNodeSnapshotV1 {
    /// Creates primitive results from the bracketing LEZ RPC query.
    #[allow(clippy::too_many_arguments)]
    #[must_use]
    pub const fn new(
        environment: LezEnvironmentV1,
        channel_id: [u8; 32],
        genesis_block_hash: [u8; 32],
        tip: LezStableTipV1,
        transaction: LezFundTransactionSnapshotV1,
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

/// Agreement-bound canonical LEZ funded-escrow evidence.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CanonicalLezEscrowObservationV1 {
    transaction_id: [u8; 32],
    inclusion_height: u64,
    inclusion_block_hash: [u8; 32],
    tip_height: u64,
    tip_block_hash: [u8; 32],
    confirmations: NonZeroU32,
    snapshot: Box<LezNodeSnapshotV1>,
}

impl CanonicalLezEscrowObservationV1 {
    /// Validates a stable primitive RPC snapshot against every signed escrow term.
    ///
    /// # Errors
    ///
    /// Rejects the wrong direction, chain, unstable or noncanonical inclusion,
    /// malformed transaction, wrong program/accounts/actor, mismatched metadata
    /// or custody state, insufficient depth, or nonfinal public-testnet evidence.
    pub fn validate(
        agreement: &ZecAgreementV1,
        snapshot: &LezNodeSnapshotV1,
    ) -> Result<Self, LezObservationError> {
        if agreement.direction() != SwapDirection::TakerSellsLez {
            return Err(LezObservationError::WrongDirection);
        }
        let confirmations = validate_chain_position(agreement, snapshot)?;
        let expected = ExpectedAccountBinding::from_agreement(agreement);
        validate_fund_transaction(agreement, snapshot, &expected)?;
        validate_escrow_accounts(agreement, snapshot, &expected)?;
        let transaction = &snapshot.transaction;
        let tip = snapshot.tip;
        Ok(Self {
            transaction_id: transaction.transaction_id,
            inclusion_height: transaction.inclusion_height,
            inclusion_block_hash: transaction.inclusion_block_hash,
            tip_height: tip.after_height,
            tip_block_hash: tip.after_hash,
            confirmations,
            snapshot: Box::new(snapshot.clone()),
        })
    }

    /// Canonical LEZ transaction hash.
    #[must_use]
    pub const fn transaction_id(&self) -> &[u8; 32] {
        &self.transaction_id
    }

    /// Stable canonical confirmation count.
    #[must_use]
    pub const fn confirmations(&self) -> NonZeroU32 {
        self.confirmations
    }

    /// Canonical inclusion height.
    #[must_use]
    pub const fn inclusion_height(&self) -> u64 {
        self.inclusion_height
    }

    /// Canonical inclusion block hash.
    #[must_use]
    pub const fn inclusion_block_hash(&self) -> &[u8; 32] {
        &self.inclusion_block_hash
    }

    /// Stable tip height used for validation.
    #[must_use]
    pub const fn tip_height(&self) -> u64 {
        self.tip_height
    }

    /// Stable tip hash used for validation.
    #[must_use]
    pub const fn tip_block_hash(&self) -> &[u8; 32] {
        &self.tip_block_hash
    }

    pub(crate) const fn snapshot(&self) -> &LezNodeSnapshotV1 {
        &self.snapshot
    }
}

struct ExpectedAccountBinding {
    depositor: [u8; 32],
    claimant: [u8; 32],
    depositor_asset: [u8; 32],
    claimant_asset: [u8; 32],
    asset_program: [u32; 8],
    custody_program: [u32; 8],
    definition: [u8; 32],
    transaction_accounts: Vec<[u8; 32]>,
}

impl ExpectedAccountBinding {
    fn from_agreement(agreement: &ZecAgreementV1) -> Self {
        let terms = agreement.lez_terms();
        let depositor = *agreement.lez_account(agreement.lez_depositor());
        let claimant = *agreement.lez_account(agreement.lez_claimant());
        match terms.asset() {
            LezAssetV1::Native {
                authenticated_transfer_program_id,
            } => Self {
                depositor,
                claimant,
                depositor_asset: depositor,
                claimant_asset: claimant,
                asset_program: *authenticated_transfer_program_id,
                custody_program: *authenticated_transfer_program_id,
                definition: [0; 32],
                transaction_accounts: vec![
                    *terms.metadata_account(),
                    *terms.custody_account(),
                    depositor,
                ],
            },
            LezAssetV1::FungibleToken {
                definition_account,
                token_program_id,
                ata_program_id,
                depositor_ata,
                claimant_ata,
            } => Self {
                depositor,
                claimant,
                depositor_asset: *depositor_ata,
                claimant_asset: *claimant_ata,
                asset_program: *token_program_id,
                custody_program: *ata_program_id,
                definition: *definition_account,
                transaction_accounts: vec![
                    *terms.metadata_account(),
                    depositor,
                    *depositor_ata,
                    *terms.custody_account(),
                ],
            },
        }
    }
}

fn validate_chain_position(
    agreement: &ZecAgreementV1,
    snapshot: &LezNodeSnapshotV1,
) -> Result<NonZeroU32, LezObservationError> {
    let terms = agreement.lez_terms();
    if snapshot.environment != terms.chain().environment()
        || snapshot.channel_id != *terms.chain().channel_id()
        || snapshot.genesis_block_hash != *terms.chain().genesis_block_hash()
    {
        return Err(LezObservationError::ChainIdentityMismatch);
    }
    let tip = snapshot.tip;
    if tip.before_hash != tip.after_hash || tip.before_height != tip.after_height {
        return Err(LezObservationError::UnstableTip);
    }
    let transaction = &snapshot.transaction;
    if transaction.inclusion_block_hash != transaction.canonical_block_hash
        || transaction.inclusion_height > tip.after_height
    {
        return Err(LezObservationError::NoncanonicalInclusion);
    }
    let confirmations = tip
        .after_height
        .checked_sub(transaction.inclusion_height)
        .and_then(|depth| depth.checked_add(1))
        .and_then(|depth| u32::try_from(depth).ok())
        .and_then(NonZeroU32::new)
        .ok_or(LezObservationError::InvalidConfirmationDepth)?;
    let required = agreement
        .coordinator()
        .required_confirmations(Participant::Taker);
    if confirmations.get() < required {
        return Err(LezObservationError::InsufficientConfirmations {
            required,
            actual: confirmations.get(),
        });
    }
    if terms.chain().environment() == LezEnvironmentV1::PublicTestnetV0_2
        && transaction.inclusion_status != LezInclusionStatusV1::Finalized
    {
        return Err(LezObservationError::PublicFinalityRequired);
    }
    Ok(confirmations)
}

fn validate_fund_transaction(
    agreement: &ZecAgreementV1,
    snapshot: &LezNodeSnapshotV1,
    expected: &ExpectedAccountBinding,
) -> Result<(), LezObservationError> {
    let transaction = &snapshot.transaction;
    if !transaction.is_public
        || !transaction.signature_valid
        || transaction.program_id != *agreement.lez_terms().escrow_program_id()
        || transaction.signer != *agreement.lez_account(Participant::Taker)
        || transaction.swap_id != *agreement.onchain_swap_id()
    {
        return Err(LezObservationError::TransactionBindingMismatch);
    }
    if transaction.accounts != expected.transaction_accounts {
        return Err(LezObservationError::TransactionAccountsMismatch);
    }
    Ok(())
}

fn validate_escrow_accounts(
    agreement: &ZecAgreementV1,
    snapshot: &LezNodeSnapshotV1,
    expected: &ExpectedAccountBinding,
) -> Result<(), LezObservationError> {
    let terms = agreement.lez_terms();
    let expected_metadata = LezEscrowMetadataSnapshotV1::new(
        1,
        *agreement.onchain_swap_id(),
        *agreement.agreement_commitment(),
        *agreement.secret_digest(),
        expected.depositor,
        expected.depositor_asset,
        expected.claimant,
        expected.claimant_asset,
        *terms.custody_account(),
        expected.asset_program,
        expected.custody_program,
        expected.definition,
        terms.amount(),
        agreement.lez_refund_at_ms(),
        LezEscrowStatusV1::Funded,
    );
    if snapshot.metadata_program_owner != *terms.escrow_program_id()
        || snapshot.metadata_account != *terms.metadata_account()
        || snapshot.metadata != expected_metadata
    {
        return Err(LezObservationError::MetadataBindingMismatch);
    }
    let custody_matches = snapshot.custody_account == *terms.custody_account()
        && match (terms.asset(), snapshot.custody) {
            (
                LezAssetV1::Native {
                    authenticated_transfer_program_id,
                },
                LezCustodySnapshotV1::Native {
                    program_owner,
                    balance,
                },
            ) => program_owner == *authenticated_transfer_program_id && balance == terms.amount(),
            (
                LezAssetV1::FungibleToken {
                    definition_account,
                    token_program_id,
                    ..
                },
                LezCustodySnapshotV1::Token {
                    program_owner,
                    definition,
                    balance,
                },
            ) => {
                program_owner == *token_program_id
                    && definition == *definition_account
                    && balance == terms.amount()
            }
            _ => false,
        };
    if custody_matches {
        Ok(())
    } else {
        Err(LezObservationError::CustodyBindingMismatch)
    }
}

/// Failure validating canonical LEZ escrow evidence.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum LezObservationError {
    /// LEZ is not the taker's agreement-selected first lock.
    #[error("agreement direction does not select a taker-funded LEZ first lock")]
    WrongDirection,
    /// RPC chain environment or genesis differs from the signed agreement.
    #[error("LEZ chain identity does not match the signed agreement")]
    ChainIdentityMismatch,
    /// The bracketing tip reads differ.
    #[error("LEZ RPC snapshot changed while evidence was collected")]
    UnstableTip,
    /// Inclusion block is absent from the canonical chain.
    #[error("LEZ fund transaction is not in its alleged canonical block")]
    NoncanonicalInclusion,
    /// Tip/inclusion heights cannot produce a supported nonzero depth.
    #[error("LEZ confirmation depth is invalid")]
    InvalidConfirmationDepth,
    /// Evidence is below the signed policy.
    #[error("LEZ fund has {actual} confirmations; {required} required")]
    InsufficientConfirmations {
        /// Agreement threshold.
        required: u32,
        /// Canonical depth.
        actual: u32,
    },
    /// Public v0.2 evidence lacks Bedrock finality.
    #[error("public LEZ evidence must be finalized by Bedrock")]
    PublicFinalityRequired,
    /// Transaction kind, signature, program, signer, or swap ID differs.
    #[error("LEZ fund transaction does not match the signed agreement")]
    TransactionBindingMismatch,
    /// Generated fund instruction account order differs.
    #[error("LEZ fund transaction accounts do not match the generated client")]
    TransactionAccountsMismatch,
    /// Metadata address, owner, or decoded contents differ.
    #[error("LEZ escrow metadata does not match the signed agreement")]
    MetadataBindingMismatch,
    /// Custody owner, definition, or exact amount differs.
    #[error("LEZ escrow custody does not match the signed agreement")]
    CustodyBindingMismatch,
}
