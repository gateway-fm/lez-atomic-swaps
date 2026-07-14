//! Canonical, dual-signed version-1 LEZ/ZEC agreement records.

use borsh::{BorshDeserialize, BorshSerialize};
use lez_swap_core::{
    Chain, ConfirmationPolicy, Participant, SwapCoordinator, SwapDirection, SwapId, UnixSeconds,
};
use secp256k1::{Message, PublicKey, Secp256k1, ecdsa::Signature};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use zcash_protocol::{consensus::BlockHeight, value::Zatoshis};
use zcash_transparent::{
    address::TransparentAddress,
    bundle::{OutPoint, TxOut},
};

use crate::{
    FundingBuildError, TransactionBuildError, TransparentFundingRequest, TransparentSpendRequest,
    TransparentUtxo, ZcashNetworkRecordV1, ZecBindingRecordError, ZecProfileId, ZecProfileRecordV1,
    ZecRefundProfile, ZecSwapBinding, ZecSwapBindingRecordV1, derive_lez_metadata_account_v1,
    derive_lez_native_custody_account_v1, derive_lez_swap_id_v1, derive_lez_token_account_v1,
    derive_nssa_v0_1_2_metadata_account_v1, derive_nssa_v0_1_2_native_custody_account_v1,
    derive_nssa_v0_1_2_token_account_v1,
};

/// Domain separating version-1 agreement commitments from every other signature protocol.
pub const ZEC_AGREEMENT_V1_DOMAIN: &[u8] = b"logos.gateway.lez-zec.agreement.v1\0";

/// Legacy pre-channel schema recognized only for bounded typed rejection.
pub const ZEC_CONCRETE_AGREEMENT_SCHEMA_V1: u16 = 1;
/// Current schema binding the LEZ v0.2 execution channel.
pub const ZEC_CONCRETE_AGREEMENT_SCHEMA_V2: u16 = 2;

/// Maximum accepted canonical agreement record size.
pub const MAX_ZEC_AGREEMENT_RECORD_BYTES: usize = 16 * 1024;

/// Maximum application swap identifier length, preflighted before Borsh allocation.
pub const MAX_ZEC_APPLICATION_SWAP_ID_BYTES: usize = 128;

/// Maximum transparent inputs accepted by the canonical funding-input commitment helper.
pub const MAX_ZEC_FUNDING_INPUTS: usize = 64;

/// Maximum committed transparent previous-output script length.
pub const MAX_ZEC_FUNDING_SCRIPT_BYTES: usize = 520;

const ZEC_FUNDING_INPUT_SET_V1_DOMAIN: &[u8] = b"logos.gateway.zec-funding-input-set.v1\0";

/// Stable primitive spelling of a supported trade direction.
#[derive(BorshDeserialize, BorshSerialize, Clone, Copy, Debug, Eq, PartialEq)]
pub enum SwapDirectionRecordV1 {
    /// The taker funds ZEC first and the maker subsequently funds LEZ.
    TakerSellsForeign,
    /// The taker funds LEZ first and the maker subsequently funds ZEC.
    TakerSellsLez,
}

impl From<SwapDirection> for SwapDirectionRecordV1 {
    fn from(value: SwapDirection) -> Self {
        match value {
            SwapDirection::TakerSellsForeign => Self::TakerSellsForeign,
            SwapDirection::TakerSellsLez => Self::TakerSellsLez,
        }
    }
}

impl From<SwapDirectionRecordV1> for SwapDirection {
    fn from(value: SwapDirectionRecordV1) -> Self {
        match value {
            SwapDirectionRecordV1::TakerSellsForeign => Self::TakerSellsForeign,
            SwapDirectionRecordV1::TakerSellsLez => Self::TakerSellsLez,
        }
    }
}

/// LEZ deployment family committed by the named ZEC profile.
#[derive(
    BorshDeserialize, BorshSerialize, Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize,
)]
pub enum LezEnvironmentV1 {
    /// An isolated deterministic LEZ v0.2 chain.
    DeterministicLocalV0_2,
    /// The public LEZ testnet v0.2 chain.
    PublicTestnetV0_2,
    /// The pinned LEZ v0.1.2 NSSA runtime used only for deterministic compatibility evidence.
    DeterministicLocalV0_1_2Compatibility,
}

/// Exact LEZ chain identity used by both independent actors.
#[derive(BorshDeserialize, BorshSerialize, Clone, Copy, Debug, Eq, PartialEq)]
pub struct LezChainIdentityV1 {
    environment: LezEnvironmentV1,
    channel_id: [u8; 32],
    genesis_block_hash: [u8; 32],
}

impl LezChainIdentityV1 {
    /// Creates an exact environment/genesis identity.
    #[must_use]
    pub const fn new(
        environment: LezEnvironmentV1,
        channel_id: [u8; 32],
        genesis_block_hash: [u8; 32],
    ) -> Self {
        Self {
            environment,
            channel_id,
            genesis_block_hash,
        }
    }

    /// Deployment family.
    #[must_use]
    pub const fn environment(&self) -> LezEnvironmentV1 {
        self.environment
    }

    /// Exact execution channel reported by the selected runtime adapter.
    #[must_use]
    pub const fn channel_id(&self) -> &[u8; 32] {
        &self.channel_id
    }

    /// Exact genesis block hash that a chain adapter must re-query.
    #[must_use]
    pub const fn genesis_block_hash(&self) -> &[u8; 32] {
        &self.genesis_block_hash
    }
}

/// Public identities assigned to one immutable protocol role.
#[derive(BorshDeserialize, BorshSerialize, Clone, Debug, Eq, PartialEq)]
pub struct ZecParticipantIdentityV1 {
    lez_owner_account: [u8; 32],
    zcash_compressed_pubkey: [u8; 33],
}

impl ZecParticipantIdentityV1 {
    /// Creates primitive role identity terms; agreement validation parses the Zcash key.
    #[must_use]
    pub const fn new(lez_owner_account: [u8; 32], zcash_compressed_pubkey: [u8; 33]) -> Self {
        Self {
            lez_owner_account,
            zcash_compressed_pubkey,
        }
    }

    /// Exact LEZ owner account.
    #[must_use]
    pub const fn lez_owner_account(&self) -> &[u8; 32] {
        &self.lez_owner_account
    }

    /// Canonical compressed transparent Zcash public key bytes.
    #[must_use]
    pub const fn zcash_compressed_pubkey(&self) -> &[u8; 33] {
        &self.zcash_compressed_pubkey
    }
}

/// Maker and taker identities committed together.
#[derive(BorshDeserialize, BorshSerialize, Clone, Debug, Eq, PartialEq)]
pub struct ZecParticipantsV1 {
    maker: ZecParticipantIdentityV1,
    taker: ZecParticipantIdentityV1,
}

impl ZecParticipantsV1 {
    /// Creates the role-indexed identity set.
    #[must_use]
    pub const fn new(maker: ZecParticipantIdentityV1, taker: ZecParticipantIdentityV1) -> Self {
        Self { maker, taker }
    }

    /// Returns one role's immutable identity.
    #[must_use]
    pub const fn for_participant(&self, participant: Participant) -> &ZecParticipantIdentityV1 {
        match participant {
            Participant::Maker => &self.maker,
            Participant::Taker => &self.taker,
        }
    }
}

/// Exact LEZ asset construction used by the escrow.
#[derive(BorshDeserialize, BorshSerialize, Clone, Debug, Eq, PartialEq)]
pub enum LezAssetV1 {
    /// Native value moved through the authenticated-transfer program.
    Native {
        /// Exact authenticated-transfer program identifier.
        authenticated_transfer_program_id: [u32; 8],
    },
    /// Fungible token moved through Token and associated-token-account programs.
    FungibleToken {
        /// Exact token definition account.
        definition_account: [u8; 32],
        /// Exact Token program identifier.
        token_program_id: [u32; 8],
        /// Exact associated-token-account program identifier.
        ata_program_id: [u32; 8],
        /// Exact depositor associated-token-account destination.
        depositor_ata: [u8; 32],
        /// Exact claimant associated-token-account destination.
        claimant_ata: [u8; 32],
    },
}

/// Concrete LEZ escrow terms. Actor destinations and deadlines are derived from the body.
#[derive(BorshDeserialize, BorshSerialize, Clone, Debug, Eq, PartialEq)]
pub struct ZecLezTermsV1 {
    chain: LezChainIdentityV1,
    escrow_program_id: [u32; 8],
    asset: LezAssetV1,
    amount: u128,
    metadata_account: [u8; 32],
    custody_account: [u8; 32],
}

impl ZecLezTermsV1 {
    /// Creates exact LEZ chain, program, asset, and amount terms.
    #[must_use]
    pub const fn new(
        chain: LezChainIdentityV1,
        escrow_program_id: [u32; 8],
        asset: LezAssetV1,
        amount: u128,
        metadata_account: [u8; 32],
        custody_account: [u8; 32],
    ) -> Self {
        Self {
            chain,
            escrow_program_id,
            asset,
            amount,
            metadata_account,
            custody_account,
        }
    }

    /// Exact LEZ chain identity.
    #[must_use]
    pub const fn chain(&self) -> &LezChainIdentityV1 {
        &self.chain
    }

    /// Exact escrow program identifier.
    #[must_use]
    pub const fn escrow_program_id(&self) -> &[u32; 8] {
        &self.escrow_program_id
    }

    /// Exact native or token asset construction.
    #[must_use]
    pub const fn asset(&self) -> &LezAssetV1 {
        &self.asset
    }

    /// Exact LEZ amount.
    #[must_use]
    pub const fn amount(&self) -> u128 {
        self.amount
    }

    /// Exact metadata PDA account expected from the generated client.
    #[must_use]
    pub const fn metadata_account(&self) -> &[u8; 32] {
        &self.metadata_account
    }

    /// Exact native custody PDA or token custody ATA expected from the generated client.
    #[must_use]
    pub const fn custody_account(&self) -> &[u8; 32] {
        &self.custody_account
    }
}

/// Exact transparent P2PKH payout or change destination.
#[derive(BorshDeserialize, BorshSerialize, Clone, Copy, Debug, Eq, PartialEq)]
pub struct ZcashTransparentDestinationV1 {
    public_key_hash: [u8; 20],
}

impl ZcashTransparentDestinationV1 {
    /// Creates a transparent P2PKH destination from its canonical HASH160 payload.
    #[must_use]
    pub const fn p2pkh(public_key_hash: [u8; 20]) -> Self {
        Self { public_key_hash }
    }

    /// Exact P2PKH HASH160 payload.
    #[must_use]
    pub const fn public_key_hash(&self) -> &[u8; 20] {
        &self.public_key_hash
    }
}

/// Exact executable transparent-transaction policy signed by both roles.
#[derive(BorshDeserialize, BorshSerialize, Clone, Copy, Debug, Eq, PartialEq)]
pub struct ZecTransactionPolicyV1 {
    funding_input_set_commitment: [u8; 32],
    funding_change_destination: ZcashTransparentDestinationV1,
    funding_fee_zatoshis: u64,
    minimum_change_zatoshis: u64,
    claim_destination: ZcashTransparentDestinationV1,
    claim_fee_zatoshis: u64,
    refund_destination: ZcashTransparentDestinationV1,
    refund_fee_zatoshis: u64,
    expiry_delta_blocks: u32,
}

impl ZecTransactionPolicyV1 {
    /// Creates an untrusted exact transaction policy; agreement validation cross-binds it.
    #[allow(clippy::too_many_arguments)]
    #[must_use]
    pub const fn new(
        funding_input_set_commitment: [u8; 32],
        funding_change_destination: ZcashTransparentDestinationV1,
        funding_fee_zatoshis: u64,
        minimum_change_zatoshis: u64,
        claim_destination: ZcashTransparentDestinationV1,
        claim_fee_zatoshis: u64,
        refund_destination: ZcashTransparentDestinationV1,
        refund_fee_zatoshis: u64,
        expiry_delta_blocks: u32,
    ) -> Self {
        Self {
            funding_input_set_commitment,
            funding_change_destination,
            funding_fee_zatoshis,
            minimum_change_zatoshis,
            claim_destination,
            claim_fee_zatoshis,
            refund_destination,
            refund_fee_zatoshis,
            expiry_delta_blocks,
        }
    }

    /// Commitment to the canonical funding input set.
    #[must_use]
    pub const fn funding_input_set_commitment(&self) -> &[u8; 32] {
        &self.funding_input_set_commitment
    }

    /// Exact funding change destination controlled by the ZEC funder.
    #[must_use]
    pub const fn funding_change_destination(&self) -> ZcashTransparentDestinationV1 {
        self.funding_change_destination
    }

    /// Exact funding fee.
    #[must_use]
    pub const fn funding_fee_zatoshis(&self) -> u64 {
        self.funding_fee_zatoshis
    }

    /// Exact threshold below which funding change is absorbed into the fee.
    #[must_use]
    pub const fn minimum_change_zatoshis(&self) -> u64 {
        self.minimum_change_zatoshis
    }

    /// Exact successful-claim payout destination.
    #[must_use]
    pub const fn claim_destination(&self) -> ZcashTransparentDestinationV1 {
        self.claim_destination
    }

    /// Exact successful-claim fee.
    #[must_use]
    pub const fn claim_fee_zatoshis(&self) -> u64 {
        self.claim_fee_zatoshis
    }

    /// Exact timeout-refund payout destination.
    #[must_use]
    pub const fn refund_destination(&self) -> ZcashTransparentDestinationV1 {
        self.refund_destination
    }

    /// Exact timeout-refund fee.
    #[must_use]
    pub const fn refund_fee_zatoshis(&self) -> u64 {
        self.refund_fee_zatoshis
    }

    /// Exact named-profile transaction expiry delta.
    #[must_use]
    pub const fn expiry_delta_blocks(&self) -> u32 {
        self.expiry_delta_blocks
    }
}

/// Primitive fetched transparent input used only to derive a canonical signed commitment.
#[derive(BorshSerialize, Clone, Debug, Eq, PartialEq)]
pub struct ZcashFundingInputV1 {
    transaction_id: [u8; 32],
    output_index: u32,
    value_zatoshis: u64,
    script_pubkey: Vec<u8>,
}

impl ZcashFundingInputV1 {
    /// Creates an untrusted primitive funding input.
    #[must_use]
    pub fn new(
        transaction_id: [u8; 32],
        output_index: u32,
        value_zatoshis: u64,
        script_pubkey: Vec<u8>,
    ) -> Self {
        Self {
            transaction_id,
            output_index,
            value_zatoshis,
            script_pubkey,
        }
    }
}

/// Canonically ordered, bounded transparent funding input set.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ZcashFundingInputSetV1(Vec<ZcashFundingInputV1>);

impl ZcashFundingInputSetV1 {
    /// Validates, de-duplicates, and canonically orders an exact input set.
    ///
    /// # Errors
    ///
    /// Rejects an empty/oversized set, zero transaction/value, empty/oversized script, or a
    /// duplicate outpoint.
    pub fn new(mut inputs: Vec<ZcashFundingInputV1>) -> Result<Self, FundingInputSetError> {
        if inputs.is_empty() || inputs.len() > MAX_ZEC_FUNDING_INPUTS {
            return Err(FundingInputSetError::InvalidInputCount);
        }
        for input in &inputs {
            if input.transaction_id == [0; 32] || input.value_zatoshis == 0 {
                return Err(FundingInputSetError::InvalidInput);
            }
            if input.script_pubkey.is_empty()
                || input.script_pubkey.len() > MAX_ZEC_FUNDING_SCRIPT_BYTES
            {
                return Err(FundingInputSetError::InvalidScriptLength);
            }
        }
        inputs.sort_by_key(|input| (input.transaction_id, input.output_index));
        if inputs.windows(2).any(|pair| {
            pair[0].transaction_id == pair[1].transaction_id
                && pair[0].output_index == pair[1].output_index
        }) {
            return Err(FundingInputSetError::DuplicateOutpoint);
        }
        Ok(Self(inputs))
    }

    /// Computes the domain-separated commitment used by [`ZecTransactionPolicyV1`].
    ///
    /// # Panics
    ///
    /// Only if serialization into an in-memory `Vec` unexpectedly reports an I/O failure.
    #[must_use]
    pub fn commitment(&self) -> [u8; 32] {
        let encoded = borsh::to_vec(&self.0).expect("serializing into a Vec cannot fail");
        let mut hasher = Sha256::new();
        hasher.update(ZEC_FUNDING_INPUT_SET_V1_DOMAIN);
        hasher.update(encoded);
        hasher.finalize().into()
    }

    /// Canonically ordered exact input records.
    #[must_use]
    pub fn inputs(&self) -> &[ZcashFundingInputV1] {
        &self.0
    }
}

/// Invalid transparent funding input set.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum FundingInputSetError {
    /// Input count must be between one and the fixed maximum.
    #[error("funding input count is empty or exceeds the fixed maximum")]
    InvalidInputCount,
    /// Transaction ID and value must both be nonzero.
    #[error("funding input transaction ID or value is invalid")]
    InvalidInput,
    /// Previous-output script length is empty or exceeds the fixed maximum.
    #[error("funding input script length is invalid")]
    InvalidScriptLength,
    /// One outpoint was supplied more than once.
    #[error("funding input outpoint is duplicated")]
    DuplicateOutpoint,
}

/// Final role payout destination derived from direction and immutable terms.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ZecRolePayoutV1 {
    /// LEZ claimant account receives the escrowed LEZ asset.
    LezAccount([u8; 32]),
    /// Zcash claimant receives the transparent contract output minus its exact fee.
    ZcashTransparent(ZcashTransparentDestinationV1),
}

/// Signed anchors and calibrated conservative bounds used to derive both refund deadlines.
#[derive(BorshDeserialize, BorshSerialize, Clone, Copy, Debug, Eq, PartialEq)]
pub struct ZecRefundPlanV1 {
    lez_funding_anchor_unix_seconds: u64,
    zcash_funding_anchor_height: u32,
    earlier_refund_latest_lez_ms: u64,
    later_refund_earliest_unix_seconds: u64,
}

impl ZecRefundPlanV1 {
    /// Creates the exact signed inputs to the named refund profile.
    #[must_use]
    pub const fn new(
        lez_funding_anchor_unix_seconds: u64,
        zcash_funding_anchor_height: u32,
        earlier_refund_latest_lez_ms: u64,
        later_refund_earliest_unix_seconds: u64,
    ) -> Self {
        Self {
            lez_funding_anchor_unix_seconds,
            zcash_funding_anchor_height,
            earlier_refund_latest_lez_ms,
            later_refund_earliest_unix_seconds,
        }
    }
}

/// Mutually authenticated pre-lock transcript metadata.
#[derive(BorshDeserialize, BorshSerialize, Clone, Copy, Debug, Eq, PartialEq)]
pub struct NegotiationTranscriptV1 {
    session_id: [u8; 32],
    offer_commitment: [u8; 32],
    expires_at_unix_seconds: u64,
}

impl NegotiationTranscriptV1 {
    /// Creates transcript identity, authenticated offer commitment, and expiry.
    #[must_use]
    pub const fn new(
        session_id: [u8; 32],
        offer_commitment: [u8; 32],
        expires_at_unix_seconds: u64,
    ) -> Self {
        Self {
            session_id,
            offer_commitment,
            expires_at_unix_seconds,
        }
    }
}

/// Canonical version-1 agreement body signed by maker and taker.
#[derive(BorshSerialize, Clone, Eq, PartialEq)]
pub struct ZecAgreementBodyV1 {
    application_swap_id: String,
    direction: SwapDirectionRecordV1,
    profile: ZecProfileRecordV1,
    participants: ZecParticipantsV1,
    secret_digest: [u8; 32],
    lez: ZecLezTermsV1,
    zcash: ZecSwapBindingRecordV1,
    transaction_policy: ZecTransactionPolicyV1,
    refund_plan: ZecRefundPlanV1,
    transcript: NegotiationTranscriptV1,
}

impl ZecAgreementBodyV1 {
    /// Creates an untrusted canonical body; [`ZecAgreementV1::validate_at`] enforces it.
    #[allow(clippy::too_many_arguments)]
    #[must_use]
    pub fn new(
        application_swap_id: impl Into<String>,
        direction: SwapDirection,
        profile: ZecProfileRecordV1,
        participants: ZecParticipantsV1,
        secret_digest: [u8; 32],
        lez: ZecLezTermsV1,
        zcash: ZecSwapBindingRecordV1,
        transaction_policy: ZecTransactionPolicyV1,
        refund_plan: ZecRefundPlanV1,
        transcript: NegotiationTranscriptV1,
    ) -> Self {
        Self {
            application_swap_id: application_swap_id.into(),
            direction: direction.into(),
            profile,
            participants,
            secret_digest,
            lez,
            zcash,
            transaction_policy,
            refund_plan,
            transcript,
        }
    }

    /// Computes the fixed-domain SHA-256 commitment over canonical Borsh bytes.
    ///
    /// # Panics
    ///
    /// Only if serialization into an in-memory `Vec` unexpectedly reports an I/O failure.
    #[must_use]
    pub fn commitment(&self) -> [u8; 32] {
        let encoded = borsh::to_vec(self).expect("serializing into a Vec cannot fail");
        let mut hasher = Sha256::new();
        hasher.update(ZEC_AGREEMENT_V1_DOMAIN);
        hasher.update(encoded);
        hasher.finalize().into()
    }

    /// Stable application-level swap ID.
    #[must_use]
    pub fn application_swap_id(&self) -> &str {
        &self.application_swap_id
    }

    /// Exact direction.
    #[must_use]
    pub const fn direction(&self) -> SwapDirection {
        match self.direction {
            SwapDirectionRecordV1::TakerSellsForeign => SwapDirection::TakerSellsForeign,
            SwapDirectionRecordV1::TakerSellsLez => SwapDirection::TakerSellsLez,
        }
    }

    /// Exact LEZ terms.
    #[must_use]
    pub const fn lez_terms(&self) -> &ZecLezTermsV1 {
        &self.lez
    }
}

/// Primitive agreement record. It is untrusted until validated.
#[derive(BorshSerialize, Clone, Eq, PartialEq)]
pub struct ZecAgreementRecordV1 {
    schema_version: u16,
    body: ZecAgreementBodyV1,
    agreement_commitment: [u8; 32],
    maker_signature: [u8; 64],
    taker_signature: [u8; 64],
}

impl ZecAgreementRecordV1 {
    /// Assembles untrusted wire parts. Validation recomputes every derived value.
    #[must_use]
    pub const fn from_parts(
        schema_version: u16,
        body: ZecAgreementBodyV1,
        agreement_commitment: [u8; 32],
        maker_signature: [u8; 64],
        taker_signature: [u8; 64],
    ) -> Self {
        Self {
            schema_version,
            body,
            agreement_commitment,
            maker_signature,
            taker_signature,
        }
    }

    /// Canonical body that was purportedly signed.
    #[must_use]
    pub const fn body(&self) -> &ZecAgreementBodyV1 {
        &self.body
    }

    /// Encodes the exact canonical bounded wire record.
    ///
    /// # Errors
    ///
    /// Rejects a record that exceeds the fixed network limit.
    pub fn encode_wire(&self) -> Result<Vec<u8>, ZecAgreementV1Error> {
        let encoded = borsh::to_vec(self).map_err(|_| ZecAgreementV1Error::WireEncoding)?;
        if encoded.len() > MAX_ZEC_AGREEMENT_RECORD_BYTES {
            return Err(ZecAgreementV1Error::OversizedWireRecord {
                actual: encoded.len(),
                maximum: MAX_ZEC_AGREEMENT_RECORD_BYTES,
            });
        }
        Ok(encoded)
    }
}

/// Trusted, cross-bound version-1 agreement and its derived initial coordinator.
#[derive(Clone, Eq, PartialEq)]
pub struct ZecAgreementV1 {
    record: ZecAgreementRecordV1,
    binding: ZecSwapBinding,
    coordinator: SwapCoordinator,
    maker_zcash_key: PublicKey,
    taker_zcash_key: PublicKey,
    lez_refund_at_ms: u64,
    zcash_refund_at_height: u32,
    onchain_swap_id: [u8; 32],
}

impl ZecAgreementV1 {
    /// Revalidates an untrusted record and derives all executable terms.
    ///
    /// # Errors
    ///
    /// Returns a typed error for schema, identity, profile, role, deadline, commitment, expiry,
    /// signature, asset, amount, network, program, or BIP-199 binding violations.
    pub fn validate_at(
        record: ZecAgreementRecordV1,
        now: UnixSeconds,
    ) -> Result<Self, ZecAgreementV1Error> {
        let envelope = validate_envelope(&record, now)?;
        let binding = validate_binding(
            &record.body,
            &envelope.maker_zcash_key,
            &envelope.taker_zcash_key,
        )?;
        let protocol = derive_protocol(&record.body, &binding, envelope.swap_id)?;
        let onchain_swap_id = derive_lez_swap_id_v1(record.body.application_swap_id.as_bytes());
        Ok(Self {
            record,
            binding,
            coordinator: protocol.coordinator,
            maker_zcash_key: envelope.maker_zcash_key,
            taker_zcash_key: envelope.taker_zcash_key,
            lez_refund_at_ms: protocol.lez_refund_at_ms,
            zcash_refund_at_height: protocol.zcash_refund_at_height,
            onchain_swap_id,
        })
    }

    /// Decodes and validates the only supported network-entry representation.
    ///
    /// The Borsh string length is preflighted before deserialization, preventing a peer from
    /// declaring an attacker-sized application ID allocation.
    ///
    /// # Errors
    ///
    /// Rejects oversized, truncated, malformed, trailing, overlong-ID, or invalid records.
    pub fn from_wire_at(bytes: &[u8], now: UnixSeconds) -> Result<Self, ZecAgreementV1Error> {
        const APPLICATION_ID_LENGTH_OFFSET: usize = 2;
        const APPLICATION_ID_BYTES_OFFSET: usize = APPLICATION_ID_LENGTH_OFFSET + 4;
        if bytes.len() > MAX_ZEC_AGREEMENT_RECORD_BYTES {
            return Err(ZecAgreementV1Error::OversizedWireRecord {
                actual: bytes.len(),
                maximum: MAX_ZEC_AGREEMENT_RECORD_BYTES,
            });
        }
        let length_bytes: [u8; 4] = bytes
            .get(APPLICATION_ID_LENGTH_OFFSET..APPLICATION_ID_BYTES_OFFSET)
            .and_then(|value| value.try_into().ok())
            .ok_or(ZecAgreementV1Error::MalformedWireRecord)?;
        let declared_length = usize::try_from(u32::from_le_bytes(length_bytes))
            .map_err(|_| ZecAgreementV1Error::ApplicationIdTooLong)?;
        if declared_length > MAX_ZEC_APPLICATION_SWAP_ID_BYTES {
            return Err(ZecAgreementV1Error::ApplicationIdTooLong);
        }
        let record = decode_bounded_record(bytes)?;
        Self::validate_at(record, now)
    }

    /// Exact validated primitive record for durable persistence.
    #[must_use]
    pub const fn record(&self) -> &ZecAgreementRecordV1 {
        &self.record
    }

    /// Deterministic fresh coordinator derived from the signed terms.
    #[must_use]
    pub const fn coordinator(&self) -> &SwapCoordinator {
        &self.coordinator
    }

    /// Revalidated profile, network, amount, and BIP-199 binding.
    #[must_use]
    pub const fn binding(&self) -> &ZecSwapBinding {
        &self.binding
    }

    /// Concrete agreement schema version.
    #[must_use]
    pub const fn schema_version(&self) -> u16 {
        self.record.schema_version
    }

    /// Exact signed trade direction.
    #[must_use]
    pub const fn direction(&self) -> SwapDirection {
        self.record.body.direction()
    }

    /// Concrete LEZ chain, deployment, asset, and amount terms.
    #[must_use]
    pub const fn lez_terms(&self) -> &ZecLezTermsV1 {
        &self.record.body.lez
    }

    /// SHA-256 digest used identically by the LEZ escrow and BIP-199 contract.
    #[must_use]
    pub const fn secret_digest(&self) -> &[u8; 32] {
        &self.record.body.secret_digest
    }

    /// Exact signed agreement commitment passed to LEZ as `terms_hash`.
    #[must_use]
    pub const fn agreement_commitment(&self) -> &[u8; 32] {
        &self.record.agreement_commitment
    }

    /// Exact profile-derived LEZ refund deadline in guest milliseconds.
    #[must_use]
    pub const fn lez_refund_at_ms(&self) -> u64 {
        self.lez_refund_at_ms
    }

    /// Exact profile-derived Zcash CLTV height.
    #[must_use]
    pub const fn zcash_refund_at_height(&self) -> u32 {
        self.zcash_refund_at_height
    }

    /// Canonical wire encoding of this validated agreement.
    ///
    /// # Errors
    ///
    /// Returns an encoding/size error if the retained record violates the fixed network bound.
    pub fn encode_wire(&self) -> Result<Vec<u8>, ZecAgreementV1Error> {
        self.record.encode_wire()
    }

    /// Deterministic 32-byte LEZ guest/PDA swap identifier derived from the application ID.
    #[must_use]
    pub const fn onchain_swap_id(&self) -> &[u8; 32] {
        &self.onchain_swap_id
    }

    /// Exact role-local Zcash transaction construction policy.
    ///
    /// These fields constrain bytes built by the local funder. A remote
    /// counterparty accepts funding from canonical consensus evidence for the
    /// exact agreement-bound HTLC output; it does not require disclosure of
    /// unspent wallet candidates or reject a consensus-valid alternative
    /// input/change choice that locks the same principal.
    #[must_use]
    pub const fn transaction_policy(&self) -> &ZecTransactionPolicyV1 {
        &self.record.body.transaction_policy
    }

    /// Derives and validates the only funding request permitted by the signed agreement.
    ///
    /// # Errors
    ///
    /// Rejects a changed input set, overflowed expiry, wrong ownership, invalid amounts, or an
    /// existing canonical funding-builder error.
    pub fn funding_request(
        &self,
        candidates: Vec<TransparentUtxo>,
        current_height: BlockHeight,
    ) -> Result<TransparentFundingRequest, ZecAgreementExecutionError> {
        let committed = funding_input_set_from_utxos(&candidates)?.commitment();
        if committed
            .ct_eq(self.transaction_policy().funding_input_set_commitment())
            .unwrap_u8()
            == 0
        {
            return Err(ZecAgreementExecutionError::FundingInputCommitmentMismatch);
        }
        let policy = self.transaction_policy();
        TransparentFundingRequest::new(
            candidates,
            *self.zcash_key(self.lez_claimant()),
            self.binding.expected_output().value(),
            zatoshis(policy.funding_fee_zatoshis())?,
            zatoshis(policy.minimum_change_zatoshis())?,
            self.expiry_height(current_height)?,
            self.binding.expected_output().consensus_branch_id(),
        )
        .map_err(ZecAgreementExecutionError::Funding)
    }

    /// Validates a prebuilt funding request against every signed executable term.
    ///
    /// # Errors
    ///
    /// Returns a derivation error or `FundingRequestMismatch` for any request drift.
    pub fn validate_funding_request(
        &self,
        request: &TransparentFundingRequest,
        current_height: BlockHeight,
    ) -> Result<(), ZecAgreementExecutionError> {
        let expected = self.funding_request(request.candidates().to_vec(), current_height)?;
        if &expected == request {
            Ok(())
        } else {
            Err(ZecAgreementExecutionError::FundingRequestMismatch)
        }
    }

    /// Derives the only successful-claim spend request permitted by the agreement.
    ///
    /// # Errors
    ///
    /// Rejects wrong funding output semantics, overflowed expiry, or canonical spend errors.
    pub fn claim_spend_request(
        &self,
        prevout: OutPoint,
        funding_output: TxOut,
        current_height: BlockHeight,
    ) -> Result<TransparentSpendRequest, ZecAgreementExecutionError> {
        self.spend_request(prevout, funding_output, current_height, true)
    }

    /// Derives the only timeout-refund spend request permitted by the agreement.
    ///
    /// # Errors
    ///
    /// Rejects wrong funding output semantics, overflowed expiry, or canonical spend errors.
    pub fn refund_spend_request(
        &self,
        prevout: OutPoint,
        funding_output: TxOut,
        current_height: BlockHeight,
    ) -> Result<TransparentSpendRequest, ZecAgreementExecutionError> {
        self.spend_request(prevout, funding_output, current_height, false)
    }

    /// Validates a prebuilt claim request against every signed executable term.
    ///
    /// # Errors
    ///
    /// Returns a derivation error or `SpendRequestMismatch` for any request drift.
    pub fn validate_claim_spend_request(
        &self,
        request: &TransparentSpendRequest,
        current_height: BlockHeight,
    ) -> Result<(), ZecAgreementExecutionError> {
        self.validate_spend_request(request, current_height, true)
    }

    /// Validates a prebuilt refund request against every signed executable term.
    ///
    /// # Errors
    ///
    /// Returns a derivation error or `SpendRequestMismatch` for any request drift.
    pub fn validate_refund_spend_request(
        &self,
        request: &TransparentSpendRequest,
        current_height: BlockHeight,
    ) -> Result<(), ZecAgreementExecutionError> {
        self.validate_spend_request(request, current_height, false)
    }

    fn validate_spend_request(
        &self,
        request: &TransparentSpendRequest,
        current_height: BlockHeight,
        claim: bool,
    ) -> Result<(), ZecAgreementExecutionError> {
        let expected = self.spend_request(
            request.prevout().clone(),
            request.funding_output().clone(),
            current_height,
            claim,
        )?;
        if &expected == request {
            Ok(())
        } else {
            Err(ZecAgreementExecutionError::SpendRequestMismatch)
        }
    }

    fn spend_request(
        &self,
        prevout: OutPoint,
        funding_output: TxOut,
        current_height: BlockHeight,
        claim: bool,
    ) -> Result<TransparentSpendRequest, ZecAgreementExecutionError> {
        if funding_output.value() != self.binding.expected_output().value() {
            return Err(ZecAgreementExecutionError::FundingOutputMismatch);
        }
        let policy = self.transaction_policy();
        let (destination, fee) = if claim {
            (policy.claim_destination(), policy.claim_fee_zatoshis())
        } else {
            (policy.refund_destination(), policy.refund_fee_zatoshis())
        };
        TransparentSpendRequest::new(
            self.binding.expected_output().contract(),
            prevout,
            funding_output,
            TransparentAddress::PublicKeyHash(*destination.public_key_hash()),
            zatoshis(fee)?,
            self.expiry_height(current_height)?,
            self.binding.expected_output().consensus_branch_id(),
        )
        .map_err(ZecAgreementExecutionError::Spend)
    }

    fn expiry_height(
        &self,
        current_height: BlockHeight,
    ) -> Result<BlockHeight, ZecAgreementExecutionError> {
        u32::from(current_height)
            .checked_add(self.transaction_policy().expiry_delta_blocks())
            .map(BlockHeight::from_u32)
            .ok_or(ZecAgreementExecutionError::ExpiryHeightOverflow)
    }

    /// Final payout destination for one role under the immutable direction.
    #[must_use]
    pub fn payout_for(&self, participant: Participant) -> ZecRolePayoutV1 {
        if participant == self.lez_claimant() {
            ZecRolePayoutV1::LezAccount(
                *self
                    .record
                    .body
                    .participants
                    .for_participant(participant)
                    .lez_owner_account(),
            )
        } else {
            ZecRolePayoutV1::ZcashTransparent(self.record.body.transaction_policy.claim_destination)
        }
    }

    /// Role that deposits into the LEZ escrow.
    #[must_use]
    pub const fn lez_depositor(&self) -> Participant {
        role_mapping(self.record.body.direction()).0
    }

    /// Role that claims LEZ first and funds the Zcash contract.
    #[must_use]
    pub const fn lez_claimant(&self) -> Participant {
        role_mapping(self.record.body.direction()).1
    }

    /// Exact role-derived LEZ owner destination.
    #[must_use]
    pub const fn lez_account(&self, participant: Participant) -> &[u8; 32] {
        self.record
            .body
            .participants
            .for_participant(participant)
            .lez_owner_account()
    }

    /// Parsed canonical transparent Zcash public key for one role.
    #[must_use]
    pub const fn zcash_key(&self, participant: Participant) -> &PublicKey {
        match participant {
            Participant::Maker => &self.maker_zcash_key,
            Participant::Taker => &self.taker_zcash_key,
        }
    }
}

/// Failure deriving or validating a canonical transaction request from an agreement.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum ZecAgreementExecutionError {
    /// Candidate input set is structurally invalid.
    #[error(transparent)]
    FundingInputSet(#[from] FundingInputSetError),
    /// Candidate input set differs from the exact signed commitment.
    #[error("funding input set differs from the agreement commitment")]
    FundingInputCommitmentMismatch,
    /// Current height plus the signed expiry delta exceeds the height domain.
    #[error("transaction expiry height overflowed")]
    ExpiryHeightOverflow,
    /// Signed amount cannot be represented by the canonical Zcash amount type.
    #[error("signed Zcash amount is outside the canonical range")]
    InvalidAmount,
    /// Canonical funding request construction failed.
    #[error("canonical funding request is invalid: {0}")]
    Funding(FundingBuildError),
    /// A prebuilt funding request differs from the agreement-derived request.
    #[error("prebuilt funding request differs from signed agreement terms")]
    FundingRequestMismatch,
    /// Fetched contract output value differs from the immutable binding.
    #[error("fetched funding output value differs from the agreement")]
    FundingOutputMismatch,
    /// Canonical spend request construction failed.
    #[error("canonical spend request is invalid: {0}")]
    Spend(TransactionBuildError),
    /// A prebuilt claim/refund request differs from the agreement-derived request.
    #[error("prebuilt spend request differs from signed agreement terms")]
    SpendRequestMismatch,
}

fn zatoshis(value: u64) -> Result<Zatoshis, ZecAgreementExecutionError> {
    Zatoshis::from_u64(value).map_err(|_| ZecAgreementExecutionError::InvalidAmount)
}

fn funding_input_set_from_utxos(
    candidates: &[TransparentUtxo],
) -> Result<ZcashFundingInputSetV1, FundingInputSetError> {
    ZcashFundingInputSetV1::new(
        candidates
            .iter()
            .map(|candidate| {
                ZcashFundingInputV1::new(
                    *candidate.outpoint().hash(),
                    candidate.outpoint().n(),
                    u64::from(candidate.output().value()),
                    candidate.output().script_pubkey().0.0.clone(),
                )
            })
            .collect(),
    )
}

impl std::fmt::Debug for ZecAgreementBodyV1 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ZecAgreementBodyV1")
            .field("application_swap_id", &self.application_swap_id)
            .field("direction", &self.direction)
            .field("profile", &self.profile)
            .field("secret_digest", &"[REDACTED]")
            .field("participants", &"[REDACTED]")
            .field("lez", &"[REDACTED]")
            .field("zcash", &"[REDACTED]")
            .field("transaction_policy", &"[REDACTED]")
            .field("refund_plan", &"[REDACTED]")
            .field("transcript", &"[REDACTED]")
            .finish()
    }
}

impl std::fmt::Debug for ZecAgreementRecordV1 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ZecAgreementRecordV1")
            .field("schema_version", &self.schema_version)
            .field("body", &self.body)
            .field("agreement_commitment", &"[REDACTED]")
            .field("maker_signature", &"[REDACTED]")
            .field("taker_signature", &"[REDACTED]")
            .finish()
    }
}

impl std::fmt::Debug for ZecAgreementV1 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ZecAgreementV1")
            .field("application_swap_id", &self.record.body.application_swap_id)
            .field("direction", &self.direction())
            .field("phase", &self.coordinator.phase())
            .field("agreement", &"[REDACTED]")
            .finish_non_exhaustive()
    }
}

/// Durable local acceptance metadata stored beside the exact bounded agreement wire.
#[derive(Clone, Eq, PartialEq)]
pub struct AcceptedZecAgreementEnvelopeV1 {
    agreement_wire: Vec<u8>,
    accepted_at: UnixSeconds,
    local_participant: Participant,
    revision: u64,
}

impl std::fmt::Debug for AcceptedZecAgreementEnvelopeV1 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AcceptedZecAgreementEnvelopeV1")
            .field("agreement_wire", &"[REDACTED]")
            .field("accepted_at", &self.accepted_at.value())
            .field("local_participant", &self.local_participant)
            .field("revision", &self.revision)
            .finish()
    }
}

impl AcceptedZecAgreementEnvelopeV1 {
    /// Reconstitutes untrusted primitive fields loaded atomically from durable storage.
    ///
    /// This constructor deliberately performs no validation. Call
    /// [`AcceptedZecAgreementV1::resume`] before using any field as protocol state.
    #[must_use]
    pub fn from_durable_parts(
        agreement_wire: Vec<u8>,
        accepted_at: UnixSeconds,
        local_participant: Participant,
        revision: u64,
    ) -> Self {
        Self {
            agreement_wire,
            accepted_at,
            local_participant,
            revision,
        }
    }
}

/// Validated agreement plus durable local acceptance context.
#[derive(Clone, Eq, PartialEq)]
pub struct AcceptedZecAgreementV1 {
    agreement: ZecAgreementV1,
    accepted_at: UnixSeconds,
    local_participant: Participant,
    revision: u64,
}

impl AcceptedZecAgreementV1 {
    /// Accepts untrusted network wire exactly once at the trusted local wall clock.
    ///
    /// # Errors
    ///
    /// Returns any bounded-wire or agreement validation error, including expiry at acceptance.
    pub fn accept_wire_at(
        wire: &[u8],
        accepted_at: UnixSeconds,
        local_participant: Participant,
        revision: u64,
    ) -> Result<Self, ZecAgreementV1Error> {
        if revision > i64::MAX as u64 {
            return Err(ZecAgreementV1Error::InvalidDurableRevision(revision));
        }
        Ok(Self {
            agreement: ZecAgreementV1::from_wire_at(wire, accepted_at)?,
            accepted_at,
            local_participant,
            revision,
        })
    }

    /// Revalidates durable exact wire at its original acceptance time, not the current time.
    ///
    /// # Errors
    ///
    /// Returns any bounded-wire or agreement validation error. A record accepted before expiry
    /// remains resumable after expiry; a forged post-expiry `accepted_at` remains rejected.
    pub fn resume(envelope: &AcceptedZecAgreementEnvelopeV1) -> Result<Self, ZecAgreementV1Error> {
        Self::resume_from_durable_parts(
            envelope.agreement_wire(),
            envelope.accepted_at(),
            envelope.local_participant(),
            envelope.revision(),
        )
    }

    /// Reconstitutes fields loaded from a trusted local durable store and fully revalidates wire.
    ///
    /// # Errors
    ///
    /// Returns any bounded-wire or original-acceptance validation error. Callers must load all
    /// four fields atomically from the same local revision.
    pub fn resume_from_durable_parts(
        agreement_wire: &[u8],
        accepted_at: UnixSeconds,
        local_participant: Participant,
        revision: u64,
    ) -> Result<Self, ZecAgreementV1Error> {
        Self::accept_wire_at(agreement_wire, accepted_at, local_participant, revision)
    }

    /// Produces exact local durable fields for atomic agreement persistence.
    ///
    /// # Errors
    ///
    /// Returns an encoding error if the already validated record exceeds its fixed bound.
    pub fn durable_envelope(&self) -> Result<AcceptedZecAgreementEnvelopeV1, ZecAgreementV1Error> {
        Ok(AcceptedZecAgreementEnvelopeV1 {
            agreement_wire: self.agreement.encode_wire()?,
            accepted_at: self.accepted_at,
            local_participant: self.local_participant,
            revision: self.revision,
        })
    }

    /// Validated immutable agreement.
    #[must_use]
    pub const fn agreement(&self) -> &ZecAgreementV1 {
        &self.agreement
    }

    /// Trusted wall clock captured at first acceptance.
    #[must_use]
    pub const fn accepted_at(&self) -> UnixSeconds {
        self.accepted_at
    }

    /// Role fixed in the local durable record.
    #[must_use]
    pub const fn local_participant(&self) -> Participant {
        self.local_participant
    }

    /// Local durable revision.
    #[must_use]
    pub const fn revision(&self) -> u64 {
        self.revision
    }
}

impl std::fmt::Debug for AcceptedZecAgreementV1 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AcceptedZecAgreementV1")
            .field("agreement", &"[REDACTED]")
            .field("accepted_at", &self.accepted_at.value())
            .field("local_participant", &self.local_participant)
            .field("revision", &self.revision)
            .finish()
    }
}

struct BoundedWireReader<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> BoundedWireReader<'a> {
    const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, position: 0 }
    }

    fn fixed<const N: usize>(&mut self) -> Result<[u8; N], ZecAgreementV1Error> {
        let end = self
            .position
            .checked_add(N)
            .ok_or(ZecAgreementV1Error::MalformedWireRecord)?;
        let value = self
            .bytes
            .get(self.position..end)
            .ok_or(ZecAgreementV1Error::MalformedWireRecord)?;
        self.position = end;
        value
            .try_into()
            .map_err(|_| ZecAgreementV1Error::MalformedWireRecord)
    }

    fn u8(&mut self) -> Result<u8, ZecAgreementV1Error> {
        Ok(self.fixed::<1>()?[0])
    }

    fn u16(&mut self) -> Result<u16, ZecAgreementV1Error> {
        Ok(u16::from_le_bytes(self.fixed()?))
    }

    fn u32(&mut self) -> Result<u32, ZecAgreementV1Error> {
        Ok(u32::from_le_bytes(self.fixed()?))
    }

    fn u64(&mut self) -> Result<u64, ZecAgreementV1Error> {
        Ok(u64::from_le_bytes(self.fixed()?))
    }

    fn u128(&mut self) -> Result<u128, ZecAgreementV1Error> {
        Ok(u128::from_le_bytes(self.fixed()?))
    }

    fn program_id(&mut self) -> Result<[u32; 8], ZecAgreementV1Error> {
        let mut value = [0_u32; 8];
        for word in &mut value {
            *word = self.u32()?;
        }
        Ok(value)
    }

    fn bounded_vec(&mut self, maximum: usize) -> Result<Vec<u8>, ZecAgreementV1Error> {
        let length =
            usize::try_from(self.u32()?).map_err(|_| ZecAgreementV1Error::MalformedWireRecord)?;
        if length > maximum {
            return Err(ZecAgreementV1Error::MalformedWireRecord);
        }
        let end = self
            .position
            .checked_add(length)
            .ok_or(ZecAgreementV1Error::MalformedWireRecord)?;
        let value = self
            .bytes
            .get(self.position..end)
            .ok_or(ZecAgreementV1Error::MalformedWireRecord)?;
        self.position = end;
        Ok(value.to_vec())
    }

    fn bounded_string(&mut self, maximum: usize) -> Result<String, ZecAgreementV1Error> {
        let value = self.bounded_vec(maximum)?;
        String::from_utf8(value).map_err(|_| ZecAgreementV1Error::MalformedWireRecord)
    }

    fn finish(self) -> Result<(), ZecAgreementV1Error> {
        if self.position == self.bytes.len() {
            Ok(())
        } else {
            Err(ZecAgreementV1Error::MalformedWireRecord)
        }
    }
}

fn decode_bounded_record(bytes: &[u8]) -> Result<ZecAgreementRecordV1, ZecAgreementV1Error> {
    let mut reader = BoundedWireReader::new(bytes);
    let schema_version = reader.u16()?;
    let body = decode_bounded_body(&mut reader, schema_version)?;
    let agreement_commitment = reader.fixed()?;
    let maker_signature = reader.fixed()?;
    let taker_signature = reader.fixed()?;
    reader.finish()?;
    Ok(ZecAgreementRecordV1::from_parts(
        schema_version,
        body,
        agreement_commitment,
        maker_signature,
        taker_signature,
    ))
}

fn decode_bounded_body(
    reader: &mut BoundedWireReader<'_>,
    schema_version: u16,
) -> Result<ZecAgreementBodyV1, ZecAgreementV1Error> {
    let application_swap_id = reader.bounded_string(MAX_ZEC_APPLICATION_SWAP_ID_BYTES)?;
    let direction = match reader.u8()? {
        0 => SwapDirection::TakerSellsForeign,
        1 => SwapDirection::TakerSellsLez,
        _ => return Err(ZecAgreementV1Error::MalformedWireRecord),
    };
    let profile = decode_profile(reader.u8()?)?;
    let participants = ZecParticipantsV1::new(
        ZecParticipantIdentityV1::new(reader.fixed()?, reader.fixed()?),
        ZecParticipantIdentityV1::new(reader.fixed()?, reader.fixed()?),
    );
    let secret_digest = reader.fixed()?;
    let lez = decode_lez_terms(reader, schema_version)?;
    let zcash = decode_zcash_binding(reader)?;
    let transaction_policy = ZecTransactionPolicyV1::new(
        reader.fixed()?,
        ZcashTransparentDestinationV1::p2pkh(reader.fixed()?),
        reader.u64()?,
        reader.u64()?,
        ZcashTransparentDestinationV1::p2pkh(reader.fixed()?),
        reader.u64()?,
        ZcashTransparentDestinationV1::p2pkh(reader.fixed()?),
        reader.u64()?,
        reader.u32()?,
    );
    let refund_plan =
        ZecRefundPlanV1::new(reader.u64()?, reader.u32()?, reader.u64()?, reader.u64()?);
    let transcript = NegotiationTranscriptV1::new(reader.fixed()?, reader.fixed()?, reader.u64()?);
    Ok(ZecAgreementBodyV1::new(
        application_swap_id,
        direction,
        profile,
        participants,
        secret_digest,
        lez,
        zcash,
        transaction_policy,
        refund_plan,
        transcript,
    ))
}

fn decode_lez_terms(
    reader: &mut BoundedWireReader<'_>,
    schema_version: u16,
) -> Result<ZecLezTermsV1, ZecAgreementV1Error> {
    let environment = match reader.u8()? {
        0 => LezEnvironmentV1::DeterministicLocalV0_2,
        1 => LezEnvironmentV1::PublicTestnetV0_2,
        2 => LezEnvironmentV1::DeterministicLocalV0_1_2Compatibility,
        _ => return Err(ZecAgreementV1Error::MalformedWireRecord),
    };
    let channel_id = if schema_version == ZEC_CONCRETE_AGREEMENT_SCHEMA_V1 {
        [0; 32]
    } else {
        reader.fixed()?
    };
    let chain = LezChainIdentityV1::new(environment, channel_id, reader.fixed()?);
    let escrow_program_id = reader.program_id()?;
    let asset = match reader.u8()? {
        0 => LezAssetV1::Native {
            authenticated_transfer_program_id: reader.program_id()?,
        },
        1 => LezAssetV1::FungibleToken {
            definition_account: reader.fixed()?,
            token_program_id: reader.program_id()?,
            ata_program_id: reader.program_id()?,
            depositor_ata: reader.fixed()?,
            claimant_ata: reader.fixed()?,
        },
        _ => return Err(ZecAgreementV1Error::MalformedWireRecord),
    };
    Ok(ZecLezTermsV1::new(
        chain,
        escrow_program_id,
        asset,
        reader.u128()?,
        reader.fixed()?,
        reader.fixed()?,
    ))
}

fn decode_zcash_binding(
    reader: &mut BoundedWireReader<'_>,
) -> Result<ZecSwapBindingRecordV1, ZecAgreementV1Error> {
    let profile = decode_profile(reader.u8()?)?;
    let network = match reader.u8()? {
        0 => ZcashNetworkRecordV1::Main,
        1 => ZcashNetworkRecordV1::Test,
        2 => ZcashNetworkRecordV1::Regtest,
        _ => return Err(ZecAgreementV1Error::MalformedWireRecord),
    };
    Ok(ZecSwapBindingRecordV1::from_bounded_wire_parts(
        profile,
        network,
        reader.u32()?,
        reader.u64()?,
        reader.u32()?,
        reader.fixed()?,
        reader.fixed()?,
        reader.fixed()?,
        reader.bounded_vec(MAX_ZEC_FUNDING_SCRIPT_BYTES)?,
        reader.bounded_vec(MAX_ZEC_FUNDING_SCRIPT_BYTES)?,
    ))
}

fn decode_profile(value: u8) -> Result<ZecProfileRecordV1, ZecAgreementV1Error> {
    match value {
        0 => Ok(ZecProfileRecordV1::DeterministicLocalV1),
        1 => Ok(ZecProfileRecordV1::PublicTestnetV1),
        _ => Err(ZecAgreementV1Error::MalformedWireRecord),
    }
}

/// Rejection taxonomy for untrusted version-1 agreements.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum ZecAgreementV1Error {
    /// The wire schema is not version 1.
    #[error("unsupported concrete LEZ/ZEC agreement schema {0}")]
    UnsupportedSchema(u16),
    /// The application swap identifier is invalid.
    #[error("invalid application swap identifier")]
    InvalidSwapId,
    /// Declared application ID length exceeds the fixed pre-allocation limit.
    #[error("declared application swap identifier is too long")]
    ApplicationIdTooLong,
    /// Wire bytes exceed the fixed agreement-record limit.
    #[error("agreement record is {actual} bytes; maximum is {maximum}")]
    OversizedWireRecord {
        /// Actual received or encoded byte count.
        actual: usize,
        /// Fixed maximum byte count.
        maximum: usize,
    },
    /// Borsh bytes are truncated, malformed, non-canonical, or contain trailing data.
    #[error("agreement wire record is malformed")]
    MalformedWireRecord,
    /// Canonical in-memory wire encoding failed.
    #[error("agreement wire record could not be encoded")]
    WireEncoding,
    /// Durable revisions must fit the non-negative signed range used by `SQLite`.
    #[error("durable agreement revision {0} exceeds the supported range")]
    InvalidDurableRevision(u64),
    /// The agreement does not bind a real SHA-256 digest.
    #[error("agreement secret digest is empty")]
    EmptySecretDigest,
    /// Transcript session or offer identity is empty.
    #[error("agreement transcript identity is empty")]
    EmptyTranscriptIdentity,
    /// The countersigned agreement has expired.
    #[error("agreement transcript has expired")]
    Expired,
    /// LEZ genesis identity is empty.
    #[error("LEZ genesis identity is empty")]
    EmptyLezGenesis,
    /// LEZ v0.2 execution channel identity is empty.
    #[error("LEZ execution channel identity is empty")]
    EmptyLezChannel,
    /// Named profile and LEZ deployment family disagree.
    #[error("LEZ environment does not match the named profile")]
    LezEnvironmentMismatch,
    /// A required LEZ account or program identifier is default/empty.
    #[error("LEZ account or program identity is empty")]
    EmptyLezIdentity,
    /// LEZ programs that must be independent were aliased.
    #[error("LEZ program identities are not independent")]
    ConflictingLezPrograms,
    /// Metadata, custody, actor, definition, or asset destinations collide or are empty.
    #[error("LEZ metadata, custody, or asset destinations are invalid")]
    InvalidLezDestination,
    /// Signed LEZ accounts differ from pinned v0.2 PDA/ATA derivation.
    #[error("LEZ metadata, custody, or asset destination derivation mismatch")]
    LezDerivationMismatch,
    /// LEZ amount is zero.
    #[error("LEZ amount must be positive")]
    EmptyLezAmount,
    /// Maker and taker identities are not distinct.
    #[error("maker and taker identities must be distinct")]
    DuplicateParticipantIdentity,
    /// A role supplied a malformed or non-compressed Zcash public key.
    #[error("{0:?} supplied an invalid compressed Zcash public key")]
    InvalidZcashPublicKey(Participant),
    /// Stored commitment differs from the canonical domain-separated body hash.
    #[error("agreement commitment does not match the canonical body")]
    CommitmentMismatch,
    /// A compact ECDSA signature is malformed.
    #[error("{0:?} agreement signature is malformed")]
    InvalidSignatureEncoding(Participant),
    /// A high-S ECDSA signature is valid but non-canonical.
    #[error("{0:?} agreement signature is not low-S")]
    NonCanonicalSignature(Participant),
    /// A role did not sign this exact commitment with its committed key.
    #[error("{0:?} agreement signature does not verify")]
    SignatureMismatch(Participant),
    /// Primitive Zcash binding failed reconstruction.
    #[error(transparent)]
    Binding(#[from] ZecBindingRecordError),
    /// Body profile differs from the reconstructed Zcash binding profile.
    #[error("agreement profile and Zcash binding profile differ")]
    ProfileMismatch,
    /// Zcash contract value is zero.
    #[error("Zcash contract value must be positive")]
    EmptyZcashAmount,
    /// LEZ and Zcash legs do not use the same SHA-256 digest.
    #[error("LEZ and Zcash secret digests differ")]
    SecretDigestMismatch,
    /// BIP-199 refund branch is controlled by the wrong role.
    #[error("BIP-199 refund authority does not match the Zcash funder")]
    ZcashRefundAuthorityMismatch,
    /// BIP-199 claim branch pays the wrong role.
    #[error("BIP-199 claimant does not match the Zcash recipient")]
    ZcashClaimantMismatch,
    /// BIP-199 CLTV differs from the exact profile-derived coordinator deadline.
    #[error("BIP-199 refund height differs from the profile-derived deadline")]
    ZcashRefundDeadlineMismatch,
    /// Conservative earlier-refund bound is before the actual profile-derived LEZ deadline.
    #[error("earlier-refund latest bound precedes the actual LEZ refund deadline")]
    EarlierRefundBoundBeforeLezDeadline,
    /// Funding input-set commitment is empty.
    #[error("funding input-set commitment is empty")]
    EmptyFundingInputSetCommitment,
    /// Exact funding, claim, or refund fee is zero or can consume the contract principal.
    #[error("{0} fee is zero or can consume the contract principal")]
    UnsafeTransactionFee(&'static str),
    /// Signed transaction destination differs from its role-bound public key.
    #[error("{0} destination does not match its role-bound public key")]
    TransactionDestinationMismatch(&'static str),
    /// Signed transaction expiry delta differs from the named profile.
    #[error("transaction expiry delta differs from the named profile")]
    ExpiryDeltaMismatch,
    /// Funding dust/change threshold is zero or exceeds the contract principal.
    #[error("funding dust and change policy is invalid")]
    InvalidDustPolicy,
    /// Refund anchor/calibration terms cannot satisfy the named profile.
    #[error("invalid named refund profile: {0}")]
    InvalidRefundProfile(crate::ProfileError),
    /// Named confirmation policy could not be reconstructed.
    #[error("invalid named confirmation policy")]
    InvalidConfirmationPolicy,
}

struct ValidatedEnvelope {
    swap_id: SwapId,
    maker_zcash_key: PublicKey,
    taker_zcash_key: PublicKey,
}

fn validate_envelope(
    record: &ZecAgreementRecordV1,
    now: UnixSeconds,
) -> Result<ValidatedEnvelope, ZecAgreementV1Error> {
    if record.schema_version != ZEC_CONCRETE_AGREEMENT_SCHEMA_V2 {
        return Err(ZecAgreementV1Error::UnsupportedSchema(
            record.schema_version,
        ));
    }
    let body = &record.body;
    if body.application_swap_id.len() > MAX_ZEC_APPLICATION_SWAP_ID_BYTES {
        return Err(ZecAgreementV1Error::ApplicationIdTooLong);
    }
    let swap_id = SwapId::new(body.application_swap_id.clone())
        .map_err(|_| ZecAgreementV1Error::InvalidSwapId)?;
    if body.secret_digest == [0; 32] {
        return Err(ZecAgreementV1Error::EmptySecretDigest);
    }
    validate_transcript(&body.transcript, now)?;
    validate_participants(&body.participants)?;
    validate_lez_terms(body)?;
    let expected_commitment = body.commitment();
    if record
        .agreement_commitment
        .ct_eq(&expected_commitment)
        .unwrap_u8()
        == 0
    {
        return Err(ZecAgreementV1Error::CommitmentMismatch);
    }
    let maker_zcash_key = parse_role_key(&body.participants, Participant::Maker)?;
    let taker_zcash_key = parse_role_key(&body.participants, Participant::Taker)?;
    verify_role_signature(
        Participant::Maker,
        &maker_zcash_key,
        &record.maker_signature,
        expected_commitment,
    )?;
    verify_role_signature(
        Participant::Taker,
        &taker_zcash_key,
        &record.taker_signature,
        expected_commitment,
    )?;
    Ok(ValidatedEnvelope {
        swap_id,
        maker_zcash_key,
        taker_zcash_key,
    })
}

fn validate_binding(
    body: &ZecAgreementBodyV1,
    maker_zcash_key: &PublicKey,
    taker_zcash_key: &PublicKey,
) -> Result<ZecSwapBinding, ZecAgreementV1Error> {
    let binding = body.zcash.validate()?;
    if binding.profile_id() != ZecProfileId::from(body.profile) {
        return Err(ZecAgreementV1Error::ProfileMismatch);
    }
    let expected_output = binding.expected_output();
    if expected_output.value().is_zero() {
        return Err(ZecAgreementV1Error::EmptyZcashAmount);
    }
    let contract = expected_output.contract();
    if contract.secret_digest() != body.secret_digest {
        return Err(ZecAgreementV1Error::SecretDigestMismatch);
    }
    let (_, _, zcash_refunder, zcash_claimant) = role_mapping(body.direction());
    debug_assert_ne!(zcash_refunder, zcash_claimant);
    let expected_refund_hash = pubkey_hash(match zcash_refunder {
        Participant::Maker => maker_zcash_key,
        Participant::Taker => taker_zcash_key,
    });
    if contract.refund_pubkey_hash() != expected_refund_hash {
        return Err(ZecAgreementV1Error::ZcashRefundAuthorityMismatch);
    }
    let expected_claimant_hash = pubkey_hash(match zcash_claimant {
        Participant::Maker => maker_zcash_key,
        Participant::Taker => taker_zcash_key,
    });
    if contract.claimant_pubkey_hash() != expected_claimant_hash {
        return Err(ZecAgreementV1Error::ZcashClaimantMismatch);
    }
    validate_transaction_policy(body, &binding, expected_refund_hash, expected_claimant_hash)?;
    Ok(binding)
}

fn validate_transaction_policy(
    body: &ZecAgreementBodyV1,
    binding: &ZecSwapBinding,
    refunder_hash: [u8; 20],
    claimant_hash: [u8; 20],
) -> Result<(), ZecAgreementV1Error> {
    let policy = &body.transaction_policy;
    if policy.funding_input_set_commitment == [0; 32] {
        return Err(ZecAgreementV1Error::EmptyFundingInputSetCommitment);
    }
    let principal = u64::from(binding.expected_output().value());
    if policy.funding_fee_zatoshis == 0 || policy.funding_fee_zatoshis > principal {
        return Err(ZecAgreementV1Error::UnsafeTransactionFee("funding"));
    }
    if policy.minimum_change_zatoshis == 0 || policy.minimum_change_zatoshis > principal {
        return Err(ZecAgreementV1Error::InvalidDustPolicy);
    }
    if policy.claim_fee_zatoshis == 0 || policy.claim_fee_zatoshis >= principal {
        return Err(ZecAgreementV1Error::UnsafeTransactionFee("claim"));
    }
    if policy.refund_fee_zatoshis == 0 || policy.refund_fee_zatoshis >= principal {
        return Err(ZecAgreementV1Error::UnsafeTransactionFee("refund"));
    }
    if policy.funding_change_destination.public_key_hash != refunder_hash {
        return Err(ZecAgreementV1Error::TransactionDestinationMismatch(
            "funding change",
        ));
    }
    if policy.claim_destination.public_key_hash != claimant_hash {
        return Err(ZecAgreementV1Error::TransactionDestinationMismatch("claim"));
    }
    if policy.refund_destination.public_key_hash != refunder_hash {
        return Err(ZecAgreementV1Error::TransactionDestinationMismatch(
            "refund",
        ));
    }
    let profile = ZecRefundProfile::for_id(binding.profile_id());
    if policy.expiry_delta_blocks != profile.expiry_delta_blocks() {
        return Err(ZecAgreementV1Error::ExpiryDeltaMismatch);
    }
    Ok(())
}

struct DerivedProtocol {
    coordinator: SwapCoordinator,
    lez_refund_at_ms: u64,
    zcash_refund_at_height: u32,
}

fn derive_protocol(
    body: &ZecAgreementBodyV1,
    binding: &ZecSwapBinding,
    swap_id: SwapId,
) -> Result<DerivedProtocol, ZecAgreementV1Error> {
    let profile = ZecRefundProfile::for_id(binding.profile_id());
    let lez_refund_at = profile
        .lez_refund_at(UnixSeconds::new(
            body.refund_plan.lez_funding_anchor_unix_seconds,
        ))
        .map_err(ZecAgreementV1Error::InvalidRefundProfile)?;
    if body.refund_plan.earlier_refund_latest_lez_ms < lez_refund_at.value() {
        return Err(ZecAgreementV1Error::EarlierRefundBoundBeforeLezDeadline);
    }
    let zcash_refund_at = profile
        .zcash_refund_at(BlockHeight::from_u32(
            body.refund_plan.zcash_funding_anchor_height,
        ))
        .map_err(ZecAgreementV1Error::InvalidRefundProfile)?;
    if binding.expected_output().contract().refund_lock_time() != u32::from(zcash_refund_at) {
        return Err(ZecAgreementV1Error::ZcashRefundDeadlineMismatch);
    }
    let direction = body.direction();
    let schedule = profile
        .recovery_schedule(
            direction,
            lez_refund_at.to_unix_seconds_floor(),
            zcash_refund_at,
            lez_swap_core::LezUnixMilliseconds::new(body.refund_plan.earlier_refund_latest_lez_ms),
            Some(UnixSeconds::new(
                body.refund_plan.later_refund_earliest_unix_seconds,
            )),
        )
        .map_err(ZecAgreementV1Error::InvalidRefundProfile)?;
    let coordinator = SwapCoordinator::new_with_confirmation_policies(
        swap_id,
        lez_swap_core::Pair::Zcash,
        direction,
        ConfirmationPolicy::new(confirmations_for(profile, direction, Participant::Taker))
            .map_err(|_| ZecAgreementV1Error::InvalidConfirmationPolicy)?,
        ConfirmationPolicy::new(confirmations_for(profile, direction, Participant::Maker))
            .map_err(|_| ZecAgreementV1Error::InvalidConfirmationPolicy)?,
        schedule,
    );
    let (lez_depositor, lez_claimant, _, _) = role_mapping(direction);
    debug_assert_eq!(coordinator.funded_chain(lez_depositor), Chain::Lez);
    debug_assert_eq!(coordinator.funded_chain(lez_claimant), Chain::Zcash);
    Ok(DerivedProtocol {
        coordinator,
        lez_refund_at_ms: lez_refund_at.value(),
        zcash_refund_at_height: zcash_refund_at.into(),
    })
}

fn validate_transcript(
    transcript: &NegotiationTranscriptV1,
    now: UnixSeconds,
) -> Result<(), ZecAgreementV1Error> {
    if transcript.session_id == [0; 32]
        || transcript.offer_commitment == [0; 32]
        || transcript.expires_at_unix_seconds == 0
    {
        return Err(ZecAgreementV1Error::EmptyTranscriptIdentity);
    }
    if now.value() >= transcript.expires_at_unix_seconds {
        return Err(ZecAgreementV1Error::Expired);
    }
    Ok(())
}

fn validate_participants(participants: &ZecParticipantsV1) -> Result<(), ZecAgreementV1Error> {
    let maker = participants.for_participant(Participant::Maker);
    let taker = participants.for_participant(Participant::Taker);
    if maker.lez_owner_account == [0; 32] || taker.lez_owner_account == [0; 32] {
        return Err(ZecAgreementV1Error::EmptyLezIdentity);
    }
    if maker.lez_owner_account == taker.lez_owner_account
        || maker.zcash_compressed_pubkey == taker.zcash_compressed_pubkey
    {
        return Err(ZecAgreementV1Error::DuplicateParticipantIdentity);
    }
    let _ = parse_role_key(participants, Participant::Maker)?;
    let _ = parse_role_key(participants, Participant::Taker)?;
    Ok(())
}

fn validate_lez_terms(body: &ZecAgreementBodyV1) -> Result<(), ZecAgreementV1Error> {
    let terms = &body.lez;
    validate_lez_chain_profile(body, terms)?;
    if terms.amount == 0 {
        return Err(ZecAgreementV1Error::EmptyLezAmount);
    }
    if terms.escrow_program_id == [0; 8] {
        return Err(ZecAgreementV1Error::EmptyLezIdentity);
    }
    let onchain_swap_id = derive_lez_swap_id_v1(body.application_swap_id.as_bytes());
    let expected_metadata = derive_runtime_metadata_account(
        terms.chain.environment,
        &terms.escrow_program_id,
        &onchain_swap_id,
    );
    if terms.metadata_account != expected_metadata {
        return Err(ZecAgreementV1Error::LezDerivationMismatch);
    }
    let (lez_depositor, lez_claimant, _, _) = role_mapping(body.direction());
    let depositor_owner = *body
        .participants
        .for_participant(lez_depositor)
        .lez_owner_account();
    let claimant_owner = *body
        .participants
        .for_participant(lez_claimant)
        .lez_owner_account();
    validate_lez_asset(
        terms,
        &onchain_swap_id,
        expected_metadata,
        depositor_owner,
        claimant_owner,
    )
}

fn validate_lez_chain_profile(
    body: &ZecAgreementBodyV1,
    terms: &ZecLezTermsV1,
) -> Result<(), ZecAgreementV1Error> {
    if terms.chain.channel_id == [0; 32] {
        return Err(ZecAgreementV1Error::EmptyLezChannel);
    }
    if terms.chain.genesis_block_hash == [0; 32] {
        return Err(ZecAgreementV1Error::EmptyLezGenesis);
    }
    let environment_matches_profile = match body.profile {
        ZecProfileRecordV1::DeterministicLocalV1 => matches!(
            terms.chain.environment,
            LezEnvironmentV1::DeterministicLocalV0_2
                | LezEnvironmentV1::DeterministicLocalV0_1_2Compatibility
        ),
        ZecProfileRecordV1::PublicTestnetV1 => {
            terms.chain.environment == LezEnvironmentV1::PublicTestnetV0_2
        }
    };
    if !environment_matches_profile {
        return Err(ZecAgreementV1Error::LezEnvironmentMismatch);
    }
    Ok(())
}

fn validate_lez_asset(
    terms: &ZecLezTermsV1,
    onchain_swap_id: &[u8; 32],
    expected_metadata: [u8; 32],
    depositor_owner: [u8; 32],
    claimant_owner: [u8; 32],
) -> Result<(), ZecAgreementV1Error> {
    match &terms.asset {
        LezAssetV1::Native {
            authenticated_transfer_program_id,
        } => {
            if *authenticated_transfer_program_id == [0; 8] {
                return Err(ZecAgreementV1Error::EmptyLezIdentity);
            }
            if *authenticated_transfer_program_id == terms.escrow_program_id {
                return Err(ZecAgreementV1Error::ConflictingLezPrograms);
            }
            let expected_custody = derive_runtime_native_custody_account(
                terms.chain.environment,
                &terms.escrow_program_id,
                onchain_swap_id,
            );
            if terms.custody_account != expected_custody {
                return Err(ZecAgreementV1Error::LezDerivationMismatch);
            }
        }
        LezAssetV1::FungibleToken {
            definition_account,
            token_program_id,
            ata_program_id,
            depositor_ata,
            claimant_ata,
        } => {
            if *definition_account == [0; 32]
                || *token_program_id == [0; 8]
                || *ata_program_id == [0; 8]
            {
                return Err(ZecAgreementV1Error::EmptyLezIdentity);
            }
            if *token_program_id == *ata_program_id
                || *token_program_id == terms.escrow_program_id
                || *ata_program_id == terms.escrow_program_id
            {
                return Err(ZecAgreementV1Error::ConflictingLezPrograms);
            }
            let derive_token = |owner| {
                derive_runtime_token_account(
                    terms.chain.environment,
                    ata_program_id,
                    owner,
                    definition_account,
                )
            };
            let expected_custody = derive_token(&expected_metadata);
            let expected_depositor = derive_token(&depositor_owner);
            let expected_claimant = derive_token(&claimant_owner);
            if terms.custody_account != expected_custody
                || *depositor_ata != expected_depositor
                || *claimant_ata != expected_claimant
            {
                return Err(ZecAgreementV1Error::LezDerivationMismatch);
            }
        }
    }
    Ok(())
}

fn derive_runtime_metadata_account(
    environment: LezEnvironmentV1,
    escrow_program_id: &[u32; 8],
    onchain_swap_id: &[u8; 32],
) -> [u8; 32] {
    if environment == LezEnvironmentV1::DeterministicLocalV0_1_2Compatibility {
        derive_nssa_v0_1_2_metadata_account_v1(escrow_program_id, onchain_swap_id)
    } else {
        derive_lez_metadata_account_v1(escrow_program_id, onchain_swap_id)
    }
}

fn derive_runtime_native_custody_account(
    environment: LezEnvironmentV1,
    escrow_program_id: &[u32; 8],
    onchain_swap_id: &[u8; 32],
) -> [u8; 32] {
    if environment == LezEnvironmentV1::DeterministicLocalV0_1_2Compatibility {
        derive_nssa_v0_1_2_native_custody_account_v1(escrow_program_id, onchain_swap_id)
    } else {
        derive_lez_native_custody_account_v1(escrow_program_id, onchain_swap_id)
    }
}

fn derive_runtime_token_account(
    environment: LezEnvironmentV1,
    ata_program_id: &[u32; 8],
    owner: &[u8; 32],
    definition: &[u8; 32],
) -> [u8; 32] {
    if environment == LezEnvironmentV1::DeterministicLocalV0_1_2Compatibility {
        derive_nssa_v0_1_2_token_account_v1(ata_program_id, owner, definition)
    } else {
        derive_lez_token_account_v1(ata_program_id, owner, definition)
    }
}

fn parse_role_key(
    participants: &ZecParticipantsV1,
    participant: Participant,
) -> Result<PublicKey, ZecAgreementV1Error> {
    let bytes = participants
        .for_participant(participant)
        .zcash_compressed_pubkey();
    if !matches!(bytes[0], 0x02 | 0x03) {
        return Err(ZecAgreementV1Error::InvalidZcashPublicKey(participant));
    }
    let key = PublicKey::from_slice(bytes)
        .map_err(|_| ZecAgreementV1Error::InvalidZcashPublicKey(participant))?;
    if key.serialize() != *bytes {
        return Err(ZecAgreementV1Error::InvalidZcashPublicKey(participant));
    }
    Ok(key)
}

fn verify_role_signature(
    participant: Participant,
    public_key: &PublicKey,
    compact: &[u8; 64],
    commitment: [u8; 32],
) -> Result<(), ZecAgreementV1Error> {
    let signature = Signature::from_compact(compact)
        .map_err(|_| ZecAgreementV1Error::InvalidSignatureEncoding(participant))?;
    let mut normalized = signature;
    normalized.normalize_s();
    if normalized != signature {
        return Err(ZecAgreementV1Error::NonCanonicalSignature(participant));
    }
    Secp256k1::verification_only()
        .verify_ecdsa(&Message::from_digest(commitment), &signature, public_key)
        .map_err(|_| ZecAgreementV1Error::SignatureMismatch(participant))
}

fn pubkey_hash(public_key: &PublicKey) -> [u8; 20] {
    match TransparentAddress::from_pubkey(public_key) {
        TransparentAddress::PublicKeyHash(hash) => hash,
        TransparentAddress::ScriptHash(_) => unreachable!("public keys always yield P2PKH"),
    }
}

const fn role_mapping(
    direction: SwapDirection,
) -> (Participant, Participant, Participant, Participant) {
    match direction {
        SwapDirection::TakerSellsForeign => (
            Participant::Maker,
            Participant::Taker,
            Participant::Taker,
            Participant::Maker,
        ),
        SwapDirection::TakerSellsLez => (
            Participant::Taker,
            Participant::Maker,
            Participant::Maker,
            Participant::Taker,
        ),
    }
}

const fn confirmations_for(
    profile: ZecRefundProfile,
    direction: SwapDirection,
    participant: Participant,
) -> u32 {
    let funds_zcash = matches!(
        (direction, participant),
        (SwapDirection::TakerSellsForeign, Participant::Taker)
            | (SwapDirection::TakerSellsLez, Participant::Maker)
    );
    if funds_zcash {
        profile.zcash_confirmations()
    } else {
        profile.lez_confirmations()
    }
}

impl AcceptedZecAgreementEnvelopeV1 {
    /// Exact bounded agreement wire for local durable storage.
    #[must_use]
    pub fn agreement_wire(&self) -> &[u8] {
        &self.agreement_wire
    }

    /// Trusted acceptance wall clock persisted atomically with the wire.
    #[must_use]
    pub const fn accepted_at(&self) -> UnixSeconds {
        self.accepted_at
    }

    /// Local role persisted atomically with the wire.
    #[must_use]
    pub const fn local_participant(&self) -> Participant {
        self.local_participant
    }

    /// Durable local revision persisted atomically with the wire.
    #[must_use]
    pub const fn revision(&self) -> u64 {
        self.revision
    }
}
