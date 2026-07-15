//! Canonical, bounded, countersigned version-1 LEZ/BTC agreement.
//!
//! The agreement stores primitive wire fields but accepts them only after
//! reconstructing the `MuSig2` aggregate key, complete P2TR commitment, exact
//! cooperative claim transaction and BIP-341 sighash, and direction-specific
//! recovery schedule with pinned libraries.

use bitcoin::hashes::Hash as _;
use bitcoin::secp256k1::{Message, PublicKey, Secp256k1, XOnlyPublicKey, schnorr::Signature};
use bitcoin::{Amount, OutPoint, ScriptBuf, TxOut, Txid};
use borsh::BorshSerialize;
use lez_swap_core::{
    Chain, ChainPosition, Pair, Participant, RecoverySchedule, SwapDirection, TimelockSafety,
};
use musig2::KeyAggContext;
use musig2::secp::Point;
use sha2::{Digest as _, Sha256};
use subtle::ConstantTimeEq as _;
use thiserror::Error;

use crate::{
    CooperativeKeyPathSpend, CsvBlockDelay, OutputKeyParity, P2trSwapOutput, RefundXOnlyKey,
    TwoPartyAggregateKey,
};

/// Domain separator for canonical version-1 LEZ/BTC agreement commitments.
pub const BTC_AGREEMENT_V1_DOMAIN: &[u8] = b"logos.gateway.lez-btc.agreement.v1\0";

/// Only accepted wire schema.
pub const BTC_AGREEMENT_SCHEMA_V1: u16 = 1;

/// Maximum canonical agreement record size accepted from a peer.
pub const MAX_BTC_AGREEMENT_RECORD_BYTES: usize = 16 * 1024;

/// Largest confirmation policy accepted by version one: one difficulty period.
///
/// Confirmation counts stay wire-compatible as `u32`, while the explicit
/// operational ceiling rejects policy values unsuitable for an atomic swap.
pub const MAX_BITCOIN_REQUIRED_CONFIRMATIONS: u32 = 2_016;

const MAX_SCRIPT_BYTES: usize = 520;
const MAX_CONTROL_BLOCK_BYTES: usize = 4_129;
const MAX_UNSIGNED_TRANSACTION_BYTES: usize = 4 * 1024;

#[derive(BorshSerialize, Clone, Copy, Debug, Eq, PartialEq)]
enum BtcAgreementDirectionRecordV1 {
    TakerSellsForeign,
    TakerSellsLez,
}

impl From<SwapDirection> for BtcAgreementDirectionRecordV1 {
    fn from(direction: SwapDirection) -> Self {
        match direction {
            SwapDirection::TakerSellsForeign => Self::TakerSellsForeign,
            SwapDirection::TakerSellsLez => Self::TakerSellsLez,
        }
    }
}

impl From<BtcAgreementDirectionRecordV1> for SwapDirection {
    fn from(direction: BtcAgreementDirectionRecordV1) -> Self {
        match direction {
            BtcAgreementDirectionRecordV1::TakerSellsForeign => Self::TakerSellsForeign,
            BtcAgreementDirectionRecordV1::TakerSellsLez => Self::TakerSellsLez,
        }
    }
}

/// Stable wire spelling of the tweaked output key parity.
#[derive(BorshSerialize, Clone, Copy, Debug, Eq, PartialEq)]
pub enum BtcOutputKeyParityV1 {
    /// Even Y coordinate.
    Even,
    /// Odd Y coordinate.
    Odd,
}

impl From<OutputKeyParity> for BtcOutputKeyParityV1 {
    fn from(parity: OutputKeyParity) -> Self {
        match parity {
            OutputKeyParity::Even => Self::Even,
            OutputKeyParity::Odd => Self::Odd,
        }
    }
}

/// Bitcoin network identity and activation confirmation policy.
#[derive(BorshSerialize, Clone, Copy, Debug, Eq, PartialEq)]
pub struct BtcChainPolicyV1 {
    genesis_block_hash: [u8; 32],
    required_confirmations: u32,
}

impl BtcChainPolicyV1 {
    /// Creates untrusted primitive Bitcoin network policy fields.
    #[must_use]
    pub const fn new(genesis_block_hash: [u8; 32], required_confirmations: u32) -> Self {
        Self {
            genesis_block_hash,
            required_confirmations,
        }
    }

    /// Exact Bitcoin genesis hash bytes expected from the Core adapter.
    #[must_use]
    pub const fn genesis_block_hash(&self) -> &[u8; 32] {
        &self.genesis_block_hash
    }

    /// Confirmations required before the funding output may activate a swap.
    #[must_use]
    pub const fn required_confirmations(&self) -> u32 {
        self.required_confirmations
    }
}

/// Public identities fixed to one protocol role.
#[derive(BorshSerialize, Clone, Debug, Eq, PartialEq)]
pub struct BtcParticipantIdentityV1 {
    lez_owner_account: [u8; 32],
    musig2_public_key: [u8; 33],
    bitcoin_refund_key: [u8; 32],
    claim_destination_script_pubkey: Vec<u8>,
}

impl BtcParticipantIdentityV1 {
    /// Creates primitive role identity fields. Agreement validation reparses all keys.
    #[must_use]
    pub fn new(
        lez_owner_account: [u8; 32],
        musig2_public_key: [u8; 33],
        bitcoin_refund_key: [u8; 32],
        claim_destination_script_pubkey: Vec<u8>,
    ) -> Self {
        Self {
            lez_owner_account,
            musig2_public_key,
            bitcoin_refund_key,
            claim_destination_script_pubkey,
        }
    }

    /// Exact LEZ owner account.
    #[must_use]
    pub const fn lez_owner_account(&self) -> &[u8; 32] {
        &self.lez_owner_account
    }

    /// Compressed secp256k1 key used in maker then taker `MuSig2` order.
    #[must_use]
    pub const fn musig2_public_key(&self) -> &[u8; 33] {
        &self.musig2_public_key
    }

    /// X-only key controlling this role's Bitcoin refund leaf.
    #[must_use]
    pub const fn bitcoin_refund_key(&self) -> &[u8; 32] {
        &self.bitcoin_refund_key
    }

    /// Exact successful Bitcoin claim destination for this role.
    #[must_use]
    pub fn claim_destination_script_pubkey(&self) -> &[u8] {
        &self.claim_destination_script_pubkey
    }
}

/// Maker and taker public identities in fixed signing order.
#[derive(BorshSerialize, Clone, Debug, Eq, PartialEq)]
pub struct BtcParticipantsV1 {
    maker: BtcParticipantIdentityV1,
    taker: BtcParticipantIdentityV1,
}

impl BtcParticipantsV1 {
    /// Creates the fixed role-indexed participant set.
    #[must_use]
    pub const fn new(maker: BtcParticipantIdentityV1, taker: BtcParticipantIdentityV1) -> Self {
        Self { maker, taker }
    }

    /// Returns one role's immutable public identity.
    #[must_use]
    pub const fn for_participant(&self, participant: Participant) -> &BtcParticipantIdentityV1 {
        match participant {
            Participant::Maker => &self.maker,
            Participant::Taker => &self.taker,
        }
    }
}

/// Exact LEZ chain, deployment, accounts, value, deadline, and claim message.
#[derive(BorshSerialize, Clone, Debug, Eq, PartialEq)]
pub struct BtcLezTermsV1 {
    channel_id: [u8; 32],
    genesis_block_hash: [u8; 32],
    escrow_program_id: [u8; 32],
    authenticated_transfer_program_id: [u8; 32],
    aggregate_authority_account: [u8; 32],
    metadata_account: [u8; 32],
    custody_account: [u8; 32],
    depositor_account: [u8; 32],
    claimant_account: [u8; 32],
    amount: u128,
    refund_at_ms: u64,
    claim_message_hash: [u8; 32],
}

impl BtcLezTermsV1 {
    /// Creates untrusted primitive LEZ terms.
    #[allow(clippy::too_many_arguments)]
    #[must_use]
    pub const fn new(
        channel_id: [u8; 32],
        genesis_block_hash: [u8; 32],
        escrow_program_id: [u8; 32],
        authenticated_transfer_program_id: [u8; 32],
        aggregate_authority_account: [u8; 32],
        metadata_account: [u8; 32],
        custody_account: [u8; 32],
        depositor_account: [u8; 32],
        claimant_account: [u8; 32],
        amount: u128,
        refund_at_ms: u64,
        claim_message_hash: [u8; 32],
    ) -> Self {
        Self {
            channel_id,
            genesis_block_hash,
            escrow_program_id,
            authenticated_transfer_program_id,
            aggregate_authority_account,
            metadata_account,
            custody_account,
            depositor_account,
            claimant_account,
            amount,
            refund_at_ms,
            claim_message_hash,
        }
    }

    /// Exact LEZ channel identifier.
    #[must_use]
    pub const fn channel_id(&self) -> &[u8; 32] {
        &self.channel_id
    }

    /// Exact LEZ genesis block hash.
    #[must_use]
    pub const fn genesis_block_hash(&self) -> &[u8; 32] {
        &self.genesis_block_hash
    }

    /// Exact deployed escrow program identifier.
    #[must_use]
    pub const fn escrow_program_id(&self) -> &[u8; 32] {
        &self.escrow_program_id
    }

    /// Exact authenticated-transfer program identifier used by native custody.
    #[must_use]
    pub const fn authenticated_transfer_program_id(&self) -> &[u8; 32] {
        &self.authenticated_transfer_program_id
    }

    /// Exact official aggregate-authority account.
    #[must_use]
    pub const fn aggregate_authority_account(&self) -> &[u8; 32] {
        &self.aggregate_authority_account
    }

    /// Exact escrow metadata account.
    #[must_use]
    pub const fn metadata_account(&self) -> &[u8; 32] {
        &self.metadata_account
    }

    /// Exact escrow custody account.
    #[must_use]
    pub const fn custody_account(&self) -> &[u8; 32] {
        &self.custody_account
    }

    /// Exact LEZ depositor account selected by direction.
    #[must_use]
    pub const fn depositor_account(&self) -> &[u8; 32] {
        &self.depositor_account
    }

    /// Exact LEZ claimant account selected by direction.
    #[must_use]
    pub const fn claimant_account(&self) -> &[u8; 32] {
        &self.claimant_account
    }

    /// Exact native LEZ amount held by the escrow.
    #[must_use]
    pub const fn amount(&self) -> u128 {
        self.amount
    }

    /// Exact LEZ refund deadline in guest milliseconds.
    #[must_use]
    pub const fn refund_at_ms(&self) -> u64 {
        self.refund_at_ms
    }

    /// Exact 32-byte witnessed claim message.
    #[must_use]
    pub const fn claim_message_hash(&self) -> &[u8; 32] {
        &self.claim_message_hash
    }
}

/// Exact primitive fields of the one-leaf P2TR swap output.
#[derive(BorshSerialize, Clone, Debug, Eq, PartialEq)]
pub struct BtcP2trTermsV1 {
    aggregate_internal_key: [u8; 32],
    refund_key: [u8; 32],
    refund_csv_blocks: u32,
    refund_leaf_version: u8,
    refund_script: Vec<u8>,
    tapleaf_hash: [u8; 32],
    merkle_root: [u8; 32],
    tap_tweak_hash: [u8; 32],
    output_key: [u8; 32],
    output_key_parity: BtcOutputKeyParityV1,
    refund_control_block: Vec<u8>,
    script_pubkey: Vec<u8>,
}

impl BtcP2trTermsV1 {
    /// Captures every P2TR field produced by the canonical contract builder.
    #[must_use]
    pub fn from_contract(contract: &P2trSwapOutput) -> Self {
        Self::from_parts(
            contract.aggregate_internal_key_bytes(),
            contract.refund_key_bytes(),
            u32::from(contract.refund_delay().blocks()),
            contract.refund_leaf_version(),
            contract.refund_script_bytes().to_vec(),
            contract.tapleaf_hash_bytes(),
            contract.merkle_root_bytes(),
            contract.tap_tweak_hash_bytes(),
            contract.output_key_bytes(),
            contract.output_key_parity().into(),
            contract.refund_control_block_bytes(),
            contract.script_pubkey_bytes().to_vec(),
        )
    }

    /// Creates untrusted primitive P2TR fields. Validation reconstructs all of them.
    #[allow(clippy::too_many_arguments)]
    #[must_use]
    pub fn from_parts(
        aggregate_internal_key: [u8; 32],
        refund_key: [u8; 32],
        refund_csv_blocks: u32,
        refund_leaf_version: u8,
        refund_script: Vec<u8>,
        tapleaf_hash: [u8; 32],
        merkle_root: [u8; 32],
        tap_tweak_hash: [u8; 32],
        output_key: [u8; 32],
        output_key_parity: BtcOutputKeyParityV1,
        refund_control_block: Vec<u8>,
        script_pubkey: Vec<u8>,
    ) -> Self {
        Self {
            aggregate_internal_key,
            refund_key,
            refund_csv_blocks,
            refund_leaf_version,
            refund_script,
            tapleaf_hash,
            merkle_root,
            tap_tweak_hash,
            output_key,
            output_key_parity,
            refund_control_block,
            script_pubkey,
        }
    }

    fn matches_contract(&self, contract: &P2trSwapOutput) -> bool {
        self.aggregate_internal_key == contract.aggregate_internal_key_bytes()
            && self.refund_key == contract.refund_key_bytes()
            && self.refund_csv_blocks == u32::from(contract.refund_delay().blocks())
            && self.refund_leaf_version == contract.refund_leaf_version()
            && self.refund_script == contract.refund_script_bytes()
            && self.tapleaf_hash == contract.tapleaf_hash_bytes()
            && self.merkle_root == contract.merkle_root_bytes()
            && self.tap_tweak_hash == contract.tap_tweak_hash_bytes()
            && self.output_key == contract.output_key_bytes()
            && self.output_key_parity == contract.output_key_parity().into()
            && self.refund_control_block == contract.refund_control_block_bytes()
            && self.script_pubkey == contract.script_pubkey_bytes()
    }
}

/// Exact Bitcoin contract funding outpoint and value.
#[derive(BorshSerialize, Clone, Copy, Debug, Eq, PartialEq)]
pub struct BtcFundingTermsV1 {
    transaction_id: [u8; 32],
    output_index: u32,
    value_sat: u64,
}

impl BtcFundingTermsV1 {
    /// Creates untrusted primitive funding terms.
    #[must_use]
    pub const fn new(transaction_id: [u8; 32], output_index: u32, value_sat: u64) -> Self {
        Self {
            transaction_id,
            output_index,
            value_sat,
        }
    }

    /// Exact transaction ID byte array consumed by the claim.
    #[must_use]
    pub const fn transaction_id(&self) -> &[u8; 32] {
        &self.transaction_id
    }

    /// Exact output index.
    #[must_use]
    pub const fn output_index(&self) -> u32 {
        self.output_index
    }

    /// Exact funding value in satoshis.
    #[must_use]
    pub const fn value_sat(&self) -> u64 {
        self.value_sat
    }
}

/// Exact cooperative claim destination, output, fee, unsigned transaction, and sighash.
#[derive(BorshSerialize, Clone, Debug, Eq, PartialEq)]
pub struct BtcClaimTermsV1 {
    destination_script_pubkey: Vec<u8>,
    output_value_sat: u64,
    fee_sat: u64,
    unsigned_transaction: Vec<u8>,
    bip341_sighash: [u8; 32],
}

impl BtcClaimTermsV1 {
    /// Captures one exact single-output cooperative spend.
    ///
    /// # Errors
    ///
    /// Rejects a generic cooperative spend that does not have exactly one output.
    pub fn from_spend(spend: &CooperativeKeyPathSpend) -> Result<Self, BtcAgreementV1Error> {
        let [output] = spend.unsigned_transaction().output.as_slice() else {
            return Err(BtcAgreementV1Error::BitcoinClaimMismatch);
        };
        Ok(Self::from_parts(
            output.script_pubkey.as_bytes().to_vec(),
            output.value.to_sat(),
            spend.fee().to_sat(),
            spend.unsigned_transaction_bytes(),
            spend.sighash_bytes(),
        ))
    }

    /// Creates untrusted primitive claim fields. Validation reconstructs all of them.
    #[must_use]
    pub fn from_parts(
        destination_script_pubkey: Vec<u8>,
        output_value_sat: u64,
        fee_sat: u64,
        unsigned_transaction: Vec<u8>,
        bip341_sighash: [u8; 32],
    ) -> Self {
        Self {
            destination_script_pubkey,
            output_value_sat,
            fee_sat,
            unsigned_transaction,
            bip341_sighash,
        }
    }

    fn matches_spend(&self, spend: &CooperativeKeyPathSpend) -> bool {
        let [output] = spend.unsigned_transaction().output.as_slice() else {
            return false;
        };
        self.destination_script_pubkey == output.script_pubkey.as_bytes()
            && self.output_value_sat == output.value.to_sat()
            && self.fee_sat == spend.fee().to_sat()
            && self.unsigned_transaction == spend.unsigned_transaction_bytes()
            && self.bip341_sighash == spend.sighash_bytes()
    }
}

/// Signed anchors and conservative wall-clock bounds for direction-correct recovery.
#[derive(BorshSerialize, Clone, Copy, Debug, Eq, PartialEq)]
pub struct BtcRecoveryPlanV1 {
    bitcoin_funding_anchor_height: u32,
    bitcoin_refund_height: u32,
    earlier_refund_latest_unix_seconds: u64,
    later_refund_earliest_unix_seconds: u64,
    required_margin_seconds: u64,
}

impl BtcRecoveryPlanV1 {
    /// Creates untrusted primitive recovery terms.
    #[must_use]
    pub const fn new(
        bitcoin_funding_anchor_height: u32,
        bitcoin_refund_height: u32,
        earlier_refund_latest_unix_seconds: u64,
        later_refund_earliest_unix_seconds: u64,
        required_margin_seconds: u64,
    ) -> Self {
        Self {
            bitcoin_funding_anchor_height,
            bitcoin_refund_height,
            earlier_refund_latest_unix_seconds,
            later_refund_earliest_unix_seconds,
            required_margin_seconds,
        }
    }

    /// Bitcoin height at which the relative CSV delay begins.
    #[must_use]
    pub const fn bitcoin_funding_anchor_height(&self) -> u32 {
        self.bitcoin_funding_anchor_height
    }

    /// Exact Bitcoin height at which the signed refund path becomes available.
    #[must_use]
    pub const fn bitcoin_refund_height(&self) -> u32 {
        self.bitcoin_refund_height
    }

    /// Conservative latest wall-clock bound for the earlier refund.
    #[must_use]
    pub const fn earlier_refund_latest_unix_seconds(&self) -> u64 {
        self.earlier_refund_latest_unix_seconds
    }

    /// Conservative earliest wall-clock bound for the later refund.
    #[must_use]
    pub const fn later_refund_earliest_unix_seconds(&self) -> u64 {
        self.later_refund_earliest_unix_seconds
    }

    /// Required cross-chain safety margin in seconds.
    #[must_use]
    pub const fn required_margin_seconds(&self) -> u64 {
        self.required_margin_seconds
    }
}

/// Canonical version-1 agreement body signed by both roles.
#[derive(BorshSerialize, Clone, Debug, Eq, PartialEq)]
pub struct BtcAgreementBodyV1 {
    swap_id: [u8; 32],
    direction: BtcAgreementDirectionRecordV1,
    bitcoin_chain_policy: BtcChainPolicyV1,
    participants: BtcParticipantsV1,
    adaptor_point: [u8; 33],
    lez: BtcLezTermsV1,
    p2tr: BtcP2trTermsV1,
    funding: BtcFundingTermsV1,
    claim: BtcClaimTermsV1,
    recovery: BtcRecoveryPlanV1,
}

impl BtcAgreementBodyV1 {
    /// Creates an untrusted body. `BtcAgreementV1::validate` cross-binds every field.
    #[allow(clippy::too_many_arguments)]
    #[must_use]
    pub const fn new(
        swap_id: [u8; 32],
        direction: SwapDirection,
        bitcoin_chain_policy: BtcChainPolicyV1,
        participants: BtcParticipantsV1,
        adaptor_point: [u8; 33],
        lez: BtcLezTermsV1,
        p2tr: BtcP2trTermsV1,
        funding: BtcFundingTermsV1,
        claim: BtcClaimTermsV1,
        recovery: BtcRecoveryPlanV1,
    ) -> Self {
        Self {
            swap_id,
            direction: match direction {
                SwapDirection::TakerSellsForeign => {
                    BtcAgreementDirectionRecordV1::TakerSellsForeign
                }
                SwapDirection::TakerSellsLez => BtcAgreementDirectionRecordV1::TakerSellsLez,
            },
            bitcoin_chain_policy,
            participants,
            adaptor_point,
            lez,
            p2tr,
            funding,
            claim,
            recovery,
        }
    }

    /// Fixed-domain SHA-256 over canonical Borsh body bytes.
    ///
    /// # Panics
    ///
    /// Serializing these in-memory fields to a `Vec` has no fallible sink. A
    /// panic would indicate a broken `BorshSerialize` implementation.
    #[must_use]
    pub fn commitment(&self) -> [u8; 32] {
        let encoded = borsh::to_vec(self).expect("serializing into a Vec cannot fail");
        let mut hasher = Sha256::new();
        hasher.update(BTC_AGREEMENT_V1_DOMAIN);
        hasher.update(encoded);
        hasher.finalize().into()
    }

    /// Exact canonical Borsh body bytes used by the commitment.
    ///
    /// # Errors
    ///
    /// Returns an encoding error if an implementation of canonical encoding fails.
    pub fn encode_canonical(&self) -> Result<Vec<u8>, BtcAgreementV1Error> {
        borsh::to_vec(self).map_err(|_| BtcAgreementV1Error::WireEncoding)
    }

    /// Exact signed swap identifier.
    #[must_use]
    pub const fn swap_id(&self) -> &[u8; 32] {
        &self.swap_id
    }

    /// Exact signed direction.
    #[must_use]
    pub const fn direction(&self) -> SwapDirection {
        match self.direction {
            BtcAgreementDirectionRecordV1::TakerSellsForeign => SwapDirection::TakerSellsForeign,
            BtcAgreementDirectionRecordV1::TakerSellsLez => SwapDirection::TakerSellsLez,
        }
    }

    /// Signed Bitcoin network and confirmation policy.
    #[must_use]
    pub const fn bitcoin_chain_policy(&self) -> &BtcChainPolicyV1 {
        &self.bitcoin_chain_policy
    }

    /// Maker and taker identities in canonical signing order.
    #[must_use]
    pub const fn participants(&self) -> &BtcParticipantsV1 {
        &self.participants
    }

    /// Exact public adaptor point bound to both signing sessions.
    #[must_use]
    pub const fn adaptor_point(&self) -> &[u8; 33] {
        &self.adaptor_point
    }

    /// Exact signed LEZ terms.
    #[must_use]
    pub const fn lez_terms(&self) -> &BtcLezTermsV1 {
        &self.lez
    }

    /// Exact signed P2TR fields.
    #[must_use]
    pub const fn p2tr_terms(&self) -> &BtcP2trTermsV1 {
        &self.p2tr
    }

    /// Exact signed Bitcoin funding terms.
    #[must_use]
    pub const fn funding_terms(&self) -> &BtcFundingTermsV1 {
        &self.funding
    }

    /// Exact signed cooperative claim terms.
    #[must_use]
    pub const fn claim_terms(&self) -> &BtcClaimTermsV1 {
        &self.claim
    }

    /// Exact signed recovery plan.
    #[must_use]
    pub const fn recovery_plan(&self) -> &BtcRecoveryPlanV1 {
        &self.recovery
    }
}

/// Primitive untrusted wire record.
#[derive(BorshSerialize, Clone, Debug, Eq, PartialEq)]
pub struct BtcAgreementRecordV1 {
    schema_version: u16,
    body: BtcAgreementBodyV1,
    agreement_commitment: [u8; 32],
    maker_signature: [u8; 64],
    taker_signature: [u8; 64],
}

impl BtcAgreementRecordV1 {
    /// Assembles untrusted wire fields.
    #[must_use]
    pub const fn from_parts(
        schema_version: u16,
        body: BtcAgreementBodyV1,
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

    /// Exact wire schema version.
    #[must_use]
    pub const fn schema_version(&self) -> u16 {
        self.schema_version
    }

    /// Exact canonical agreement body.
    #[must_use]
    pub const fn body(&self) -> &BtcAgreementBodyV1 {
        &self.body
    }

    /// Exact body commitment countersigned by both roles.
    #[must_use]
    pub const fn agreement_commitment(&self) -> &[u8; 32] {
        &self.agreement_commitment
    }

    /// Exact role signature stored in the canonical record.
    #[must_use]
    pub const fn signature(&self, participant: Participant) -> &[u8; 64] {
        match participant {
            Participant::Maker => &self.maker_signature,
            Participant::Taker => &self.taker_signature,
        }
    }

    /// Encodes canonical Borsh and enforces the fixed network bound.
    ///
    /// # Errors
    ///
    /// Returns an encoding or oversize error.
    pub fn encode_wire(&self) -> Result<Vec<u8>, BtcAgreementV1Error> {
        let encoded = borsh::to_vec(self).map_err(|_| BtcAgreementV1Error::WireEncoding)?;
        if encoded.len() > MAX_BTC_AGREEMENT_RECORD_BYTES {
            return Err(BtcAgreementV1Error::OversizedWireRecord {
                actual: encoded.len(),
                maximum: MAX_BTC_AGREEMENT_RECORD_BYTES,
            });
        }
        Ok(encoded)
    }
}

/// Validated immutable agreement and reconstructed executable Bitcoin fields.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BtcAgreementV1 {
    record: BtcAgreementRecordV1,
    contract: P2trSwapOutput,
    cooperative_claim: CooperativeKeyPathSpend,
    recovery_schedule: RecoverySchedule,
}

impl BtcAgreementV1 {
    /// Validates one untrusted in-memory record.
    ///
    /// # Errors
    ///
    /// Rejects any schema, role, key, signature, derived Bitcoin, LEZ, or
    /// recovery-schedule mismatch.
    pub fn validate(record: BtcAgreementRecordV1) -> Result<Self, BtcAgreementV1Error> {
        if record.schema_version != BTC_AGREEMENT_SCHEMA_V1 {
            return Err(BtcAgreementV1Error::UnsupportedSchema(
                record.schema_version,
            ));
        }
        validate_fixed_body(&record.body)?;
        // Keep direct in-memory validation subject to the same total bound as
        // records received over the wire. Validate every variable-length field
        // first so a caller-constructed record cannot force an oversized Borsh
        // allocation before it is rejected.
        let _ = record.encode_wire()?;
        let expected_commitment = record.body.commitment();
        if record
            .agreement_commitment
            .ct_eq(&expected_commitment)
            .unwrap_u8()
            == 0
        {
            return Err(BtcAgreementV1Error::CommitmentMismatch);
        }
        let maker_key = parse_participant_key(&record.body.participants, Participant::Maker)?;
        let taker_key = parse_participant_key(&record.body.participants, Participant::Taker)?;
        verify_role_signature(
            Participant::Maker,
            &maker_key,
            record.maker_signature,
            expected_commitment,
        )?;
        verify_role_signature(
            Participant::Taker,
            &taker_key,
            record.taker_signature,
            expected_commitment,
        )?;

        let aggregate_key = derive_aggregate_key(&record.body.participants)?;
        if aggregate_key != record.body.p2tr.aggregate_internal_key {
            return Err(BtcAgreementV1Error::BitcoinAggregateKeyMismatch);
        }
        let bitcoin_funder = bitcoin_funder(record.body.direction());
        if record.body.p2tr.refund_key
            != *record
                .body
                .participants
                .for_participant(bitcoin_funder)
                .bitcoin_refund_key()
        {
            return Err(BtcAgreementV1Error::BitcoinRefundRoleMismatch);
        }
        let contract = reconstruct_contract(&record.body.p2tr)?;
        let cooperative_claim = reconstruct_claim(&record.body, &contract)?;
        let recovery_schedule = reconstruct_recovery(&record.body)?;
        Ok(Self {
            record,
            contract,
            cooperative_claim,
            recovery_schedule,
        })
    }

    /// Validates an agreement and requires the exact configured Bitcoin policy.
    ///
    /// # Errors
    ///
    /// Returns every intrinsic validation error, or a policy mismatch when the
    /// signed network or required confirmations differ from the local adapter.
    pub fn validate_for_bitcoin_policy(
        record: BtcAgreementRecordV1,
        expected: &BtcChainPolicyV1,
    ) -> Result<Self, BtcAgreementV1Error> {
        validate_bitcoin_chain_policy(expected)?;
        let agreement = Self::validate(record)?;
        agreement.ensure_bitcoin_policy(expected)?;
        Ok(agreement)
    }

    /// Bounded canonical decode followed by complete validation.
    ///
    /// # Errors
    ///
    /// Rejects oversized, truncated, malformed, trailing, non-canonical, or invalid records.
    pub fn from_wire(bytes: &[u8]) -> Result<Self, BtcAgreementV1Error> {
        if bytes.len() > MAX_BTC_AGREEMENT_RECORD_BYTES {
            return Err(BtcAgreementV1Error::OversizedWireRecord {
                actual: bytes.len(),
                maximum: MAX_BTC_AGREEMENT_RECORD_BYTES,
            });
        }
        let record = decode_bounded_record(bytes)?;
        if record.encode_wire()?.as_slice() != bytes {
            return Err(BtcAgreementV1Error::MalformedWireRecord);
        }
        Self::validate(record)
    }

    /// Bounded canonical decode with an exact configured Bitcoin policy.
    ///
    /// # Errors
    ///
    /// Returns every wire or intrinsic validation error, or a policy mismatch.
    pub fn from_wire_for_bitcoin_policy(
        bytes: &[u8],
        expected: &BtcChainPolicyV1,
    ) -> Result<Self, BtcAgreementV1Error> {
        let agreement = Self::from_wire(bytes)?;
        validate_bitcoin_chain_policy(expected)?;
        agreement.ensure_bitcoin_policy(expected)?;
        Ok(agreement)
    }

    /// Canonical wire replay.
    ///
    /// # Errors
    ///
    /// Returns an encoding or size error.
    pub fn encode_wire(&self) -> Result<Vec<u8>, BtcAgreementV1Error> {
        self.record.encode_wire()
    }

    /// Exact validated canonical record, including both role signatures.
    #[must_use]
    pub const fn record(&self) -> &BtcAgreementRecordV1 {
        &self.record
    }

    /// Exact validated canonical body used by both signing sessions.
    #[must_use]
    pub const fn body(&self) -> &BtcAgreementBodyV1 {
        &self.record.body
    }

    /// Exact signed direction.
    #[must_use]
    pub const fn direction(&self) -> SwapDirection {
        self.record.body.direction()
    }

    /// Agreement commitment signed by both roles.
    #[must_use]
    pub const fn agreement_commitment(&self) -> &[u8; 32] {
        &self.record.agreement_commitment
    }

    /// Commitment used as the role-signing session binding.
    #[must_use]
    pub const fn role_session_binding(&self) -> [u8; 32] {
        self.record.agreement_commitment
    }

    /// Commitment passed to LEZ as the immutable terms binding.
    #[must_use]
    pub const fn lez_terms_binding(&self) -> [u8; 32] {
        self.record.agreement_commitment
    }

    /// Exact LEZ terms.
    #[must_use]
    pub const fn lez_terms(&self) -> &BtcLezTermsV1 {
        &self.record.body.lez
    }

    /// Signed Bitcoin network and activation confirmation policy.
    #[must_use]
    pub const fn bitcoin_chain_policy(&self) -> &BtcChainPolicyV1 {
        &self.record.body.bitcoin_chain_policy
    }

    /// Exact signed Bitcoin genesis hash bytes.
    #[must_use]
    pub const fn bitcoin_genesis_hash(&self) -> &[u8; 32] {
        self.bitcoin_chain_policy().genesis_block_hash()
    }

    /// Confirmations required before funding activates the swap.
    #[must_use]
    pub const fn required_bitcoin_confirmations(&self) -> u32 {
        self.bitcoin_chain_policy().required_confirmations()
    }

    /// Maker and taker identities in canonical signing order.
    #[must_use]
    pub const fn participants(&self) -> &BtcParticipantsV1 {
        &self.record.body.participants
    }

    /// Immutable public identity for one role.
    #[must_use]
    pub const fn participant(&self, participant: Participant) -> &BtcParticipantIdentityV1 {
        self.participants().for_participant(participant)
    }

    /// Exact public adaptor point bound to both signing sessions.
    #[must_use]
    pub const fn adaptor_point(&self) -> &[u8; 33] {
        &self.record.body.adaptor_point
    }

    /// Exact signed Bitcoin funding outpoint and amount.
    #[must_use]
    pub const fn funding_terms(&self) -> &BtcFundingTermsV1 {
        &self.record.body.funding
    }

    /// Requires this already validated record to match local Bitcoin policy.
    ///
    /// # Errors
    ///
    /// Returns a mismatch when network identity or confirmation policy differs.
    pub fn ensure_bitcoin_policy(
        &self,
        expected: &BtcChainPolicyV1,
    ) -> Result<(), BtcAgreementV1Error> {
        if self.bitcoin_chain_policy() == expected {
            Ok(())
        } else {
            Err(BtcAgreementV1Error::BitcoinChainPolicyMismatch)
        }
    }

    /// Reconstructed exact P2TR output.
    #[must_use]
    pub const fn p2tr_contract(&self) -> &P2trSwapOutput {
        &self.contract
    }

    /// Reconstructed exact unsigned cooperative claim and BIP-341 sighash.
    #[must_use]
    pub const fn cooperative_claim(&self) -> &CooperativeKeyPathSpend {
        &self.cooperative_claim
    }

    /// Reconstructed direction-correct recovery schedule.
    #[must_use]
    pub const fn recovery_schedule(&self) -> RecoverySchedule {
        self.recovery_schedule
    }

    /// Role funding Bitcoin.
    #[must_use]
    pub const fn bitcoin_funder(&self) -> Participant {
        bitcoin_funder(self.direction())
    }

    /// Role receiving the successful Bitcoin claim.
    #[must_use]
    pub const fn bitcoin_claimant(&self) -> Participant {
        self.bitcoin_funder().other()
    }

    /// Role depositing LEZ.
    #[must_use]
    pub const fn lez_depositor(&self) -> Participant {
        lez_depositor(self.direction())
    }

    /// Role receiving the successful LEZ claim.
    #[must_use]
    pub const fn lez_claimant(&self) -> Participant {
        self.lez_depositor().other()
    }
}

/// Rejection taxonomy for untrusted version-1 agreements.
#[derive(Clone, Debug, Eq, PartialEq, Error)]
pub enum BtcAgreementV1Error {
    /// Schema is not version one.
    #[error("unsupported concrete LEZ/BTC agreement schema {0}")]
    UnsupportedSchema(u16),
    /// Wire bytes exceed the fixed limit.
    #[error("agreement record is {actual} bytes; maximum is {maximum}")]
    OversizedWireRecord {
        /// Actual byte count.
        actual: usize,
        /// Fixed maximum.
        maximum: usize,
    },
    /// Wire is truncated, malformed, non-canonical, or contains trailing bytes.
    #[error("agreement wire record is malformed")]
    MalformedWireRecord,
    /// In-memory canonical encoding failed.
    #[error("agreement wire record could not be encoded")]
    WireEncoding,
    /// A required fixed identity, value, message, or script is empty or invalid.
    #[error("agreement contains an empty or invalid identity")]
    InvalidIdentity,
    /// Bitcoin genesis hash or confirmation policy is empty or out of bounds.
    #[error("Bitcoin chain policy is invalid")]
    InvalidBitcoinChainPolicy,
    /// Signed Bitcoin network or confirmation policy differs from local configuration.
    #[error("signed Bitcoin chain policy does not match local configuration")]
    BitcoinChainPolicyMismatch,
    /// Maker and taker public identities are not distinct.
    #[error("maker and taker identities must be distinct")]
    DuplicateParticipantIdentity,
    /// Role's compressed signing key is malformed.
    #[error("{0:?} agreement signing key is malformed")]
    InvalidParticipantKey(Participant),
    /// Public adaptor point is malformed.
    #[error("agreement adaptor point is malformed")]
    InvalidAdaptorPoint,
    /// Stored commitment differs from canonical body hash.
    #[error("agreement commitment does not match canonical body")]
    CommitmentMismatch,
    /// Role signature encoding is malformed.
    #[error("{0:?} agreement signature is malformed")]
    InvalidSignatureEncoding(Participant),
    /// Role did not sign the exact commitment.
    #[error("{0:?} agreement signature does not verify")]
    SignatureMismatch(Participant),
    /// Ordered participant keys do not derive the signed aggregate key.
    #[error("participant keys do not derive the signed aggregate key")]
    BitcoinAggregateKeyMismatch,
    /// Refund leaf is not controlled by the direction-derived Bitcoin funder.
    #[error("Bitcoin refund key does not belong to the Bitcoin funder")]
    BitcoinRefundRoleMismatch,
    /// LEZ depositor or claimant account disagrees with direction.
    #[error("LEZ role accounts do not match the signed direction")]
    LezRoleMismatch,
    /// A P2TR key or delay cannot be parsed.
    #[error("Bitcoin contract key or CSV delay is invalid")]
    InvalidBitcoinContract,
    /// Any signed derived P2TR field differs from canonical reconstruction.
    #[error("signed Bitcoin contract fields differ from canonical reconstruction")]
    BitcoinContractMismatch,
    /// Funding outpoint or amount is invalid.
    #[error("Bitcoin funding terms are invalid")]
    InvalidBitcoinFunding,
    /// Claim destination belongs to the wrong role.
    #[error("Bitcoin claim destination does not belong to the claimant")]
    BitcoinClaimRoleMismatch,
    /// Claim cannot be built or any signed derived claim field drifted.
    #[error("signed Bitcoin claim differs from canonical reconstruction")]
    BitcoinClaimMismatch,
    /// Signed recovery anchors or direction-specific schedule are invalid.
    #[error("signed recovery schedule is invalid")]
    RecoveryScheduleMismatch,
}

fn validate_fixed_body(body: &BtcAgreementBodyV1) -> Result<(), BtcAgreementV1Error> {
    if body.swap_id == [0; 32]
        || body.adaptor_point == [0; 33]
        || body.funding.transaction_id == [0; 32]
        || body.funding.value_sat == 0
        || body.funding.value_sat > Amount::MAX_MONEY.to_sat()
    {
        return Err(BtcAgreementV1Error::InvalidIdentity);
    }
    Point::from_slice(&body.adaptor_point).map_err(|_| BtcAgreementV1Error::InvalidAdaptorPoint)?;
    validate_bitcoin_chain_policy(&body.bitcoin_chain_policy)?;
    validate_participants(&body.participants)?;
    validate_lez(body)?;
    validate_bounded_fields(body)?;
    Ok(())
}

fn validate_bitcoin_chain_policy(policy: &BtcChainPolicyV1) -> Result<(), BtcAgreementV1Error> {
    if policy.genesis_block_hash == [0; 32]
        || policy.required_confirmations == 0
        || policy.required_confirmations > MAX_BITCOIN_REQUIRED_CONFIRMATIONS
    {
        return Err(BtcAgreementV1Error::InvalidBitcoinChainPolicy);
    }
    Ok(())
}

fn validate_participants(participants: &BtcParticipantsV1) -> Result<(), BtcAgreementV1Error> {
    let maker = participants.for_participant(Participant::Maker);
    let taker = participants.for_participant(Participant::Taker);
    for role in [Participant::Maker, Participant::Taker] {
        let identity = participants.for_participant(role);
        if identity.lez_owner_account == [0; 32]
            || identity.bitcoin_refund_key == [0; 32]
            || identity.claim_destination_script_pubkey.is_empty()
            || identity.claim_destination_script_pubkey.len() > MAX_SCRIPT_BYTES
        {
            return Err(BtcAgreementV1Error::InvalidIdentity);
        }
        let _ = parse_participant_key(participants, role)?;
        XOnlyPublicKey::from_slice(&identity.bitcoin_refund_key)
            .map_err(|_| BtcAgreementV1Error::InvalidIdentity)?;
    }
    if maker.lez_owner_account == taker.lez_owner_account
        || maker.musig2_public_key == taker.musig2_public_key
        || maker.bitcoin_refund_key == taker.bitcoin_refund_key
        || maker.claim_destination_script_pubkey == taker.claim_destination_script_pubkey
    {
        return Err(BtcAgreementV1Error::DuplicateParticipantIdentity);
    }
    Ok(())
}

fn validate_lez(body: &BtcAgreementBodyV1) -> Result<(), BtcAgreementV1Error> {
    let lez = &body.lez;
    let fixed = [
        lez.channel_id,
        lez.genesis_block_hash,
        lez.escrow_program_id,
        lez.authenticated_transfer_program_id,
        lez.aggregate_authority_account,
        lez.metadata_account,
        lez.custody_account,
        lez.depositor_account,
        lez.claimant_account,
        lez.claim_message_hash,
    ];
    if fixed.contains(&[0; 32])
        || lez.amount == 0
        || lez.refund_at_ms == 0
        || !lez.refund_at_ms.is_multiple_of(1_000)
        || lez.escrow_program_id == lez.authenticated_transfer_program_id
        || lez.metadata_account == lez.custody_account
        || lez.depositor_account == lez.claimant_account
        || lez.aggregate_authority_account == lez.claimant_account
        || lez.aggregate_authority_account == lez.depositor_account
    {
        return Err(BtcAgreementV1Error::InvalidIdentity);
    }
    let expected_depositor = *body
        .participants
        .for_participant(lez_depositor(body.direction()))
        .lez_owner_account();
    let expected_claimant = *body
        .participants
        .for_participant(lez_depositor(body.direction()).other())
        .lez_owner_account();
    if lez.depositor_account != expected_depositor || lez.claimant_account != expected_claimant {
        return Err(BtcAgreementV1Error::LezRoleMismatch);
    }
    Ok(())
}

fn validate_bounded_fields(body: &BtcAgreementBodyV1) -> Result<(), BtcAgreementV1Error> {
    let p2tr = &body.p2tr;
    if p2tr.refund_script.is_empty()
        || p2tr.refund_script.len() > MAX_SCRIPT_BYTES
        || p2tr.refund_control_block.is_empty()
        || p2tr.refund_control_block.len() > MAX_CONTROL_BLOCK_BYTES
        || p2tr.script_pubkey.is_empty()
        || p2tr.script_pubkey.len() > MAX_SCRIPT_BYTES
        || body.claim.destination_script_pubkey.is_empty()
        || body.claim.destination_script_pubkey.len() > MAX_SCRIPT_BYTES
        || body.claim.unsigned_transaction.is_empty()
        || body.claim.unsigned_transaction.len() > MAX_UNSIGNED_TRANSACTION_BYTES
    {
        return Err(BtcAgreementV1Error::InvalidIdentity);
    }
    Ok(())
}

fn parse_participant_key(
    participants: &BtcParticipantsV1,
    role: Participant,
) -> Result<PublicKey, BtcAgreementV1Error> {
    let bytes = participants.for_participant(role).musig2_public_key();
    if !matches!(bytes[0], 0x02 | 0x03) {
        return Err(BtcAgreementV1Error::InvalidParticipantKey(role));
    }
    let key = PublicKey::from_slice(bytes)
        .map_err(|_| BtcAgreementV1Error::InvalidParticipantKey(role))?;
    if key.serialize() != *bytes {
        return Err(BtcAgreementV1Error::InvalidParticipantKey(role));
    }
    Ok(key)
}

fn verify_role_signature(
    role: Participant,
    public_key: &PublicKey,
    signature_bytes: [u8; 64],
    commitment: [u8; 32],
) -> Result<(), BtcAgreementV1Error> {
    let signature = Signature::from_slice(&signature_bytes)
        .map_err(|_| BtcAgreementV1Error::InvalidSignatureEncoding(role))?;
    let (x_only, _) = public_key.x_only_public_key();
    Secp256k1::verification_only()
        .verify_schnorr(&signature, &Message::from_digest(commitment), &x_only)
        .map_err(|_| BtcAgreementV1Error::SignatureMismatch(role))
}

fn derive_aggregate_key(participants: &BtcParticipantsV1) -> Result<[u8; 32], BtcAgreementV1Error> {
    let maker = Point::from_slice(
        participants
            .for_participant(Participant::Maker)
            .musig2_public_key(),
    )
    .map_err(|_| BtcAgreementV1Error::InvalidParticipantKey(Participant::Maker))?;
    let taker = Point::from_slice(
        participants
            .for_participant(Participant::Taker)
            .musig2_public_key(),
    )
    .map_err(|_| BtcAgreementV1Error::InvalidParticipantKey(Participant::Taker))?;
    KeyAggContext::new([maker, taker])
        .map(|context| context.aggregated_pubkey::<Point>().serialize_xonly())
        .map_err(|_| BtcAgreementV1Error::BitcoinAggregateKeyMismatch)
}

fn reconstruct_contract(terms: &BtcP2trTermsV1) -> Result<P2trSwapOutput, BtcAgreementV1Error> {
    let aggregate = TwoPartyAggregateKey::from_bytes(terms.aggregate_internal_key)
        .map_err(|_| BtcAgreementV1Error::InvalidBitcoinContract)?;
    let refund = RefundXOnlyKey::from_bytes(terms.refund_key)
        .map_err(|_| BtcAgreementV1Error::InvalidBitcoinContract)?;
    let delay = CsvBlockDelay::new(terms.refund_csv_blocks)
        .map_err(|_| BtcAgreementV1Error::InvalidBitcoinContract)?;
    let contract = P2trSwapOutput::new(aggregate, refund, delay)
        .map_err(|_| BtcAgreementV1Error::InvalidBitcoinContract)?;
    if !terms.matches_contract(&contract) {
        return Err(BtcAgreementV1Error::BitcoinContractMismatch);
    }
    Ok(contract)
}

fn reconstruct_claim(
    body: &BtcAgreementBodyV1,
    contract: &P2trSwapOutput,
) -> Result<CooperativeKeyPathSpend, BtcAgreementV1Error> {
    let expected_destination = body
        .participants
        .for_participant(bitcoin_funder(body.direction()).other())
        .claim_destination_script_pubkey();
    if body.claim.destination_script_pubkey != expected_destination {
        return Err(BtcAgreementV1Error::BitcoinClaimRoleMismatch);
    }
    let spend = CooperativeKeyPathSpend::new(
        contract,
        OutPoint {
            txid: Txid::from_byte_array(body.funding.transaction_id),
            vout: body.funding.output_index,
        },
        Amount::from_sat(body.funding.value_sat),
        vec![TxOut {
            value: Amount::from_sat(body.claim.output_value_sat),
            script_pubkey: ScriptBuf::from_bytes(body.claim.destination_script_pubkey.clone()),
        }],
    )
    .map_err(|_| BtcAgreementV1Error::BitcoinClaimMismatch)?;
    if !body.claim.matches_spend(&spend) {
        return Err(BtcAgreementV1Error::BitcoinClaimMismatch);
    }
    Ok(spend)
}

fn reconstruct_recovery(
    body: &BtcAgreementBodyV1,
) -> Result<RecoverySchedule, BtcAgreementV1Error> {
    let recovery = body.recovery;
    if recovery.bitcoin_funding_anchor_height == 0
        || recovery.bitcoin_refund_height
            != recovery
                .bitcoin_funding_anchor_height
                .checked_add(body.p2tr.refund_csv_blocks)
                .ok_or(BtcAgreementV1Error::RecoveryScheduleMismatch)?
    {
        return Err(BtcAgreementV1Error::RecoveryScheduleMismatch);
    }
    let lez_seconds = body.lez.refund_at_ms / 1_000;
    let bitcoin_position =
        ChainPosition::block_height(Chain::Bitcoin, u64::from(recovery.bitcoin_refund_height));
    let lez_position = ChainPosition::timestamp(Chain::Lez, lez_seconds);
    let (maker_position, taker_position) = match body.direction() {
        SwapDirection::TakerSellsForeign => (lez_position, bitcoin_position),
        SwapDirection::TakerSellsLez => (bitcoin_position, lez_position),
    };
    let safety = TimelockSafety::between(
        maker_position.chain(),
        taker_position.chain(),
        recovery.earlier_refund_latest_unix_seconds,
        recovery.later_refund_earliest_unix_seconds,
        recovery.required_margin_seconds,
    )
    .map_err(|_| BtcAgreementV1Error::RecoveryScheduleMismatch)?;
    match body.direction() {
        SwapDirection::TakerSellsForeign
            if lez_seconds != recovery.earlier_refund_latest_unix_seconds =>
        {
            return Err(BtcAgreementV1Error::RecoveryScheduleMismatch);
        }
        SwapDirection::TakerSellsLez
            if lez_seconds != recovery.later_refund_earliest_unix_seconds =>
        {
            return Err(BtcAgreementV1Error::RecoveryScheduleMismatch);
        }
        _ => {}
    }
    RecoverySchedule::new(
        Pair::Bitcoin,
        body.direction(),
        maker_position,
        taker_position,
        safety,
    )
    .map_err(|_| BtcAgreementV1Error::RecoveryScheduleMismatch)
}

const fn bitcoin_funder(direction: SwapDirection) -> Participant {
    match direction {
        SwapDirection::TakerSellsForeign => Participant::Taker,
        SwapDirection::TakerSellsLez => Participant::Maker,
    }
}

const fn lez_depositor(direction: SwapDirection) -> Participant {
    match direction {
        SwapDirection::TakerSellsForeign => Participant::Maker,
        SwapDirection::TakerSellsLez => Participant::Taker,
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

    fn fixed<const N: usize>(&mut self) -> Result<[u8; N], BtcAgreementV1Error> {
        let end = self
            .position
            .checked_add(N)
            .ok_or(BtcAgreementV1Error::MalformedWireRecord)?;
        let bytes = self
            .bytes
            .get(self.position..end)
            .ok_or(BtcAgreementV1Error::MalformedWireRecord)?;
        self.position = end;
        bytes
            .try_into()
            .map_err(|_| BtcAgreementV1Error::MalformedWireRecord)
    }

    fn u8(&mut self) -> Result<u8, BtcAgreementV1Error> {
        Ok(self.fixed::<1>()?[0])
    }

    fn u16(&mut self) -> Result<u16, BtcAgreementV1Error> {
        Ok(u16::from_le_bytes(self.fixed()?))
    }

    fn u32(&mut self) -> Result<u32, BtcAgreementV1Error> {
        Ok(u32::from_le_bytes(self.fixed()?))
    }

    fn u64(&mut self) -> Result<u64, BtcAgreementV1Error> {
        Ok(u64::from_le_bytes(self.fixed()?))
    }

    fn u128(&mut self) -> Result<u128, BtcAgreementV1Error> {
        Ok(u128::from_le_bytes(self.fixed()?))
    }

    fn bounded_vec(&mut self, maximum: usize) -> Result<Vec<u8>, BtcAgreementV1Error> {
        let length =
            usize::try_from(self.u32()?).map_err(|_| BtcAgreementV1Error::MalformedWireRecord)?;
        if length > maximum {
            return Err(BtcAgreementV1Error::MalformedWireRecord);
        }
        let end = self
            .position
            .checked_add(length)
            .ok_or(BtcAgreementV1Error::MalformedWireRecord)?;
        let value = self
            .bytes
            .get(self.position..end)
            .ok_or(BtcAgreementV1Error::MalformedWireRecord)?;
        self.position = end;
        Ok(value.to_vec())
    }

    fn finish(self) -> Result<(), BtcAgreementV1Error> {
        if self.position == self.bytes.len() {
            Ok(())
        } else {
            Err(BtcAgreementV1Error::MalformedWireRecord)
        }
    }
}

fn decode_bounded_record(bytes: &[u8]) -> Result<BtcAgreementRecordV1, BtcAgreementV1Error> {
    let mut reader = BoundedWireReader::new(bytes);
    let schema = reader.u16()?;
    if schema != BTC_AGREEMENT_SCHEMA_V1 {
        return Err(BtcAgreementV1Error::UnsupportedSchema(schema));
    }
    let body = decode_bounded_body(&mut reader)?;
    let commitment = reader.fixed()?;
    let maker_signature = reader.fixed()?;
    let taker_signature = reader.fixed()?;
    reader.finish()?;
    Ok(BtcAgreementRecordV1::from_parts(
        schema,
        body,
        commitment,
        maker_signature,
        taker_signature,
    ))
}

fn decode_bounded_body(
    reader: &mut BoundedWireReader<'_>,
) -> Result<BtcAgreementBodyV1, BtcAgreementV1Error> {
    let swap_id = reader.fixed()?;
    let direction = match reader.u8()? {
        0 => SwapDirection::TakerSellsForeign,
        1 => SwapDirection::TakerSellsLez,
        _ => return Err(BtcAgreementV1Error::MalformedWireRecord),
    };
    let bitcoin_chain_policy = BtcChainPolicyV1::new(reader.fixed()?, reader.u32()?);
    let participants =
        BtcParticipantsV1::new(decode_participant(reader)?, decode_participant(reader)?);
    let adaptor_point = reader.fixed()?;
    let lez = BtcLezTermsV1::new(
        reader.fixed()?,
        reader.fixed()?,
        reader.fixed()?,
        reader.fixed()?,
        reader.fixed()?,
        reader.fixed()?,
        reader.fixed()?,
        reader.fixed()?,
        reader.fixed()?,
        reader.u128()?,
        reader.u64()?,
        reader.fixed()?,
    );
    let aggregate_internal_key = reader.fixed()?;
    let refund_key = reader.fixed()?;
    let refund_csv_blocks = reader.u32()?;
    let refund_leaf_version = reader.u8()?;
    let refund_script = reader.bounded_vec(MAX_SCRIPT_BYTES)?;
    let tapleaf_hash = reader.fixed()?;
    let merkle_root = reader.fixed()?;
    let tap_tweak_hash = reader.fixed()?;
    let output_key = reader.fixed()?;
    let output_key_parity = match reader.u8()? {
        0 => BtcOutputKeyParityV1::Even,
        1 => BtcOutputKeyParityV1::Odd,
        _ => return Err(BtcAgreementV1Error::MalformedWireRecord),
    };
    let p2tr = BtcP2trTermsV1::from_parts(
        aggregate_internal_key,
        refund_key,
        refund_csv_blocks,
        refund_leaf_version,
        refund_script,
        tapleaf_hash,
        merkle_root,
        tap_tweak_hash,
        output_key,
        output_key_parity,
        reader.bounded_vec(MAX_CONTROL_BLOCK_BYTES)?,
        reader.bounded_vec(MAX_SCRIPT_BYTES)?,
    );
    let funding = BtcFundingTermsV1::new(reader.fixed()?, reader.u32()?, reader.u64()?);
    let claim = BtcClaimTermsV1::from_parts(
        reader.bounded_vec(MAX_SCRIPT_BYTES)?,
        reader.u64()?,
        reader.u64()?,
        reader.bounded_vec(MAX_UNSIGNED_TRANSACTION_BYTES)?,
        reader.fixed()?,
    );
    let recovery = BtcRecoveryPlanV1::new(
        reader.u32()?,
        reader.u32()?,
        reader.u64()?,
        reader.u64()?,
        reader.u64()?,
    );
    Ok(BtcAgreementBodyV1::new(
        swap_id,
        direction,
        bitcoin_chain_policy,
        participants,
        adaptor_point,
        lez,
        p2tr,
        funding,
        claim,
        recovery,
    ))
}

fn decode_participant(
    reader: &mut BoundedWireReader<'_>,
) -> Result<BtcParticipantIdentityV1, BtcAgreementV1Error> {
    Ok(BtcParticipantIdentityV1::new(
        reader.fixed()?,
        reader.fixed()?,
        reader.fixed()?,
        reader.bounded_vec(MAX_SCRIPT_BYTES)?,
    ))
}
