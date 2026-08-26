//! Canonical role-owned public contributions for LEZ/Bitcoin agreement setup.
//!
//! These records let Maker and Taker create their private authority in separate
//! processes while exchanging only bounded public material. The final swap ID
//! commits both validated contributions, so a countersigned agreement binds the
//! pre-agreement transcript without making Delivery or Chat runtime dependencies
//! of the post-lock protocol.

use bitcoin::secp256k1::{Message, PublicKey, Secp256k1, XOnlyPublicKey, schnorr::Signature};
use borsh::BorshSerialize;
use lez_swap_core::{Participant, SwapDirection};
use sha2::{Digest as _, Sha256};
use subtle::ConstantTimeEq as _;
use thiserror::Error;

use crate::{
    BtcAgreementBodyV1, BtcChainPolicyV1, BtcParticipantIdentityV1, BtcParticipantsV1,
    MAX_BITCOIN_REQUIRED_CONFIRMATIONS,
};

/// Domain separator for the offer/reservation pre-session binding.
pub const BTC_PRE_SESSION_V1_DOMAIN: &[u8] = b"logos.gateway.lez-btc.pre-session.v1\0";

/// Domain separator for one role-owned public-contribution commitment.
pub const BTC_ROLE_CONTRIBUTION_V1_DOMAIN: &[u8] = b"logos.gateway.lez-btc.role-contribution.v1\0";

/// Domain separator for the final jointly contributed swap identity.
pub const BTC_JOINT_SWAP_ID_V1_DOMAIN: &[u8] = b"logos.gateway.lez-btc.joint-swap-id.v1\0";

/// Only accepted role-contribution wire schema.
pub const BTC_ROLE_CONTRIBUTION_SCHEMA_V1: u16 = 1;

/// Maximum canonical role-contribution record size accepted from a peer.
pub const MAX_BTC_ROLE_CONTRIBUTION_RECORD_BYTES: usize = 2 * 1024;

/// Maximum opaque reservation binding admitted into a pre-session identity.
pub const MAX_BTC_PRE_SESSION_RESERVATION_BYTES: usize = 256;

const MAX_CLAIM_DESTINATION_SCRIPT_BYTES: usize = 520;

#[derive(BorshSerialize, Clone, Copy, Debug, Eq, PartialEq)]
enum BtcContributionRoleRecordV1 {
    Maker,
    Taker,
}

impl From<Participant> for BtcContributionRoleRecordV1 {
    fn from(participant: Participant) -> Self {
        match participant {
            Participant::Maker => Self::Maker,
            Participant::Taker => Self::Taker,
        }
    }
}

impl From<BtcContributionRoleRecordV1> for Participant {
    fn from(role: BtcContributionRoleRecordV1) -> Self {
        match role {
            BtcContributionRoleRecordV1::Maker => Self::Maker,
            BtcContributionRoleRecordV1::Taker => Self::Taker,
        }
    }
}

#[derive(BorshSerialize, Clone, Copy, Debug, Eq, PartialEq)]
enum BtcContributionDirectionRecordV1 {
    TakerSellsForeign,
    TakerSellsLez,
}

impl From<SwapDirection> for BtcContributionDirectionRecordV1 {
    fn from(direction: SwapDirection) -> Self {
        match direction {
            SwapDirection::TakerSellsForeign => Self::TakerSellsForeign,
            SwapDirection::TakerSellsLez => Self::TakerSellsLez,
        }
    }
}

impl From<BtcContributionDirectionRecordV1> for SwapDirection {
    fn from(direction: BtcContributionDirectionRecordV1) -> Self {
        match direction {
            BtcContributionDirectionRecordV1::TakerSellsForeign => Self::TakerSellsForeign,
            BtcContributionDirectionRecordV1::TakerSellsLez => Self::TakerSellsLez,
        }
    }
}

/// Public LEZ deployment identity both roles must independently pin.
#[derive(BorshSerialize, Clone, Copy, Debug, Eq, PartialEq)]
pub struct BtcLezChainIdentityV1 {
    genesis_block_hash: [u8; 32],
    channel_id: [u8; 32],
    escrow_program_id: [u8; 32],
    authenticated_transfer_program_id: [u8; 32],
}

impl BtcLezChainIdentityV1 {
    /// Creates one untrusted LEZ identity. Contribution validation rejects zero
    /// or aliased deployment identities before a role may sign it.
    #[must_use]
    pub const fn new(
        genesis_block_hash: [u8; 32],
        channel_id: [u8; 32],
        escrow_program_id: [u8; 32],
        authenticated_transfer_program_id: [u8; 32],
    ) -> Self {
        Self {
            genesis_block_hash,
            channel_id,
            escrow_program_id,
            authenticated_transfer_program_id,
        }
    }

    /// Exact LEZ genesis identity.
    #[must_use]
    pub const fn genesis_block_hash(&self) -> &[u8; 32] {
        &self.genesis_block_hash
    }

    /// Exact LEZ channel identity.
    #[must_use]
    pub const fn channel_id(&self) -> &[u8; 32] {
        &self.channel_id
    }

    /// Exact escrow program identity.
    #[must_use]
    pub const fn escrow_program_id(&self) -> &[u8; 32] {
        &self.escrow_program_id
    }

    /// Exact authenticated-transfer program identity.
    #[must_use]
    pub const fn authenticated_transfer_program_id(&self) -> &[u8; 32] {
        &self.authenticated_transfer_program_id
    }
}

/// Public role-owned material signed before a final agreement body is composed.
#[derive(BorshSerialize, Clone, Debug, Eq, PartialEq)]
pub struct BtcRoleContributionBodyV1 {
    pre_session_id: [u8; 32],
    role: BtcContributionRoleRecordV1,
    direction: BtcContributionDirectionRecordV1,
    bitcoin_chain_policy: BtcChainPolicyV1,
    lez_chain_identity: BtcLezChainIdentityV1,
    participant_identity: BtcParticipantIdentityV1,
    bitcoin_funding_key: [u8; 32],
    adaptor_point: Option<[u8; 33]>,
    role_entropy: [u8; 32],
    expires_at_unix_seconds: u64,
}

impl BtcRoleContributionBodyV1 {
    /// Constructs and validates one role's public contribution.
    ///
    /// Maker must omit the adaptor point. Taker must provide the public point
    /// corresponding to its role-private adaptor scalar.
    ///
    /// # Errors
    ///
    /// Rejects malformed, zero, aliased, oversized, or role-inappropriate
    /// public material.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        pre_session_id: [u8; 32],
        role: Participant,
        direction: SwapDirection,
        bitcoin_chain_policy: BtcChainPolicyV1,
        lez_chain_identity: BtcLezChainIdentityV1,
        participant_identity: BtcParticipantIdentityV1,
        bitcoin_funding_key: [u8; 32],
        adaptor_point: Option<[u8; 33]>,
        role_entropy: [u8; 32],
        expires_at_unix_seconds: u64,
    ) -> Result<Self, BtcRoleContributionV1Error> {
        let body = Self {
            pre_session_id,
            role: role.into(),
            direction: direction.into(),
            bitcoin_chain_policy,
            lez_chain_identity,
            participant_identity,
            bitcoin_funding_key,
            adaptor_point,
            role_entropy,
            expires_at_unix_seconds,
        };
        validate_body(&body)?;
        Ok(body)
    }

    /// Fixed-domain SHA-256 over the canonical contribution body.
    ///
    /// # Panics
    ///
    /// Panics only if serializing this fixed in-memory record into a `Vec`
    /// unexpectedly reports an I/O error.
    #[must_use]
    pub fn commitment(&self) -> [u8; 32] {
        let encoded = borsh::to_vec(self).expect("serializing into a Vec cannot fail");
        let mut hasher = Sha256::new();
        hasher.update(BTC_ROLE_CONTRIBUTION_V1_DOMAIN);
        hasher.update(encoded);
        hasher.finalize().into()
    }

    /// Offer/reservation identity shared before both contributions exist.
    #[must_use]
    pub const fn pre_session_id(&self) -> &[u8; 32] {
        &self.pre_session_id
    }

    /// Immutable role that owns the private counterpart of this material.
    #[must_use]
    pub const fn role(&self) -> Participant {
        match self.role {
            BtcContributionRoleRecordV1::Maker => Participant::Maker,
            BtcContributionRoleRecordV1::Taker => Participant::Taker,
        }
    }

    /// Exact economic direction selected by the authenticated offer.
    #[must_use]
    pub const fn direction(&self) -> SwapDirection {
        match self.direction {
            BtcContributionDirectionRecordV1::TakerSellsForeign => SwapDirection::TakerSellsForeign,
            BtcContributionDirectionRecordV1::TakerSellsLez => SwapDirection::TakerSellsLez,
        }
    }

    /// Exact Bitcoin network and confirmation policy checked by this role.
    #[must_use]
    pub const fn bitcoin_chain_policy(&self) -> &BtcChainPolicyV1 {
        &self.bitcoin_chain_policy
    }

    /// Exact LEZ deployment identity checked by this role.
    #[must_use]
    pub const fn lez_chain_identity(&self) -> &BtcLezChainIdentityV1 {
        &self.lez_chain_identity
    }

    /// Role-owned public agreement, payout, refund, and LEZ identities.
    #[must_use]
    pub const fn participant_identity(&self) -> &BtcParticipantIdentityV1 {
        &self.participant_identity
    }

    /// X-only key controlling the role-owned Bitcoin funding input selected by
    /// the direction. The runner may fund this public key but cannot spend it.
    #[must_use]
    pub const fn bitcoin_funding_key(&self) -> &[u8; 32] {
        &self.bitcoin_funding_key
    }

    /// Taker-owned adaptor point, or `None` for Maker.
    #[must_use]
    pub const fn adaptor_point(&self) -> Option<&[u8; 33]> {
        self.adaptor_point.as_ref()
    }

    /// Role-provided uniqueness material committed by the final swap identity.
    #[must_use]
    pub const fn role_entropy(&self) -> &[u8; 32] {
        &self.role_entropy
    }

    /// Signed contribution expiry.
    #[must_use]
    pub const fn expires_at_unix_seconds(&self) -> u64 {
        self.expires_at_unix_seconds
    }
}

/// Primitive canonical role-contribution record.
#[derive(BorshSerialize, Clone, Debug, Eq, PartialEq)]
pub struct BtcRoleContributionRecordV1 {
    schema_version: u16,
    body: BtcRoleContributionBodyV1,
    contribution_commitment: [u8; 32],
    role_signature: [u8; 64],
}

impl BtcRoleContributionRecordV1 {
    /// Assembles untrusted contribution fields for validation.
    #[must_use]
    pub const fn from_parts(
        schema_version: u16,
        body: BtcRoleContributionBodyV1,
        contribution_commitment: [u8; 32],
        role_signature: [u8; 64],
    ) -> Self {
        Self {
            schema_version,
            body,
            contribution_commitment,
            role_signature,
        }
    }

    /// Canonical Borsh encoding with a fixed network bound.
    ///
    /// # Errors
    ///
    /// Rejects encoding failure or an oversized record.
    pub fn encode_wire(&self) -> Result<Vec<u8>, BtcRoleContributionV1Error> {
        let encoded = borsh::to_vec(self).map_err(|_| BtcRoleContributionV1Error::WireEncoding)?;
        if encoded.len() > MAX_BTC_ROLE_CONTRIBUTION_RECORD_BYTES {
            return Err(BtcRoleContributionV1Error::OversizedWireRecord {
                actual: encoded.len(),
                maximum: MAX_BTC_ROLE_CONTRIBUTION_RECORD_BYTES,
            });
        }
        Ok(encoded)
    }
}

/// Validated immutable role-owned public contribution.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BtcRoleContributionV1 {
    record: BtcRoleContributionRecordV1,
}

impl BtcRoleContributionV1 {
    /// Validates schema, body, commitment, proof-of-possession signature, and
    /// canonical size.
    ///
    /// # Errors
    ///
    /// Rejects every malformed or mismatched field without accepting partial
    /// authority.
    pub fn validate(
        record: BtcRoleContributionRecordV1,
    ) -> Result<Self, BtcRoleContributionV1Error> {
        if record.schema_version != BTC_ROLE_CONTRIBUTION_SCHEMA_V1 {
            return Err(BtcRoleContributionV1Error::UnsupportedSchema(
                record.schema_version,
            ));
        }
        validate_body(&record.body)?;
        let _ = record.encode_wire()?;
        let expected = record.body.commitment();
        if record.contribution_commitment.ct_eq(&expected).unwrap_u8() == 0 {
            return Err(BtcRoleContributionV1Error::CommitmentMismatch);
        }
        verify_role_signature(&record.body, record.role_signature, expected)?;
        Ok(Self { record })
    }

    /// Bounded canonical decode followed by complete validation.
    ///
    /// # Errors
    ///
    /// Rejects oversized, truncated, malformed, trailing, non-canonical, or
    /// semantically invalid records.
    pub fn from_wire(bytes: &[u8]) -> Result<Self, BtcRoleContributionV1Error> {
        if bytes.len() > MAX_BTC_ROLE_CONTRIBUTION_RECORD_BYTES {
            return Err(BtcRoleContributionV1Error::OversizedWireRecord {
                actual: bytes.len(),
                maximum: MAX_BTC_ROLE_CONTRIBUTION_RECORD_BYTES,
            });
        }
        let record = decode_bounded_record(bytes)?;
        if record.encode_wire()?.as_slice() != bytes {
            return Err(BtcRoleContributionV1Error::MalformedWireRecord);
        }
        Self::validate(record)
    }

    /// Canonical wire replay.
    ///
    /// # Errors
    ///
    /// Returns an encoding or size error.
    pub fn encode_wire(&self) -> Result<Vec<u8>, BtcRoleContributionV1Error> {
        self.record.encode_wire()
    }

    /// Exact validated contribution body.
    #[must_use]
    pub const fn body(&self) -> &BtcRoleContributionBodyV1 {
        &self.record.body
    }

    /// Body commitment authenticated by the role-owned agreement key.
    #[must_use]
    pub const fn contribution_commitment(&self) -> &[u8; 32] {
        &self.record.contribution_commitment
    }

    /// Exact retained proof-of-possession signature.
    #[must_use]
    pub const fn role_signature(&self) -> &[u8; 64] {
        &self.record.role_signature
    }
}

/// Validated Maker/Taker contribution pair and its jointly derived swap ID.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BtcRoleContributionPairV1 {
    maker: BtcRoleContributionV1,
    taker: BtcRoleContributionV1,
    swap_id: [u8; 32],
}

impl BtcRoleContributionPairV1 {
    /// Cross-binds two independently signed role contributions.
    ///
    /// # Errors
    ///
    /// Rejects wrong roles, mismatched sessions/directions/chains/expiry, or
    /// aliased role identities and entropy.
    pub fn new(
        maker: BtcRoleContributionV1,
        taker: BtcRoleContributionV1,
    ) -> Result<Self, BtcRoleContributionV1Error> {
        let maker_body = maker.body();
        let taker_body = taker.body();
        if maker_body.role() != Participant::Maker || taker_body.role() != Participant::Taker {
            return Err(BtcRoleContributionV1Error::PairRoleMismatch);
        }
        if maker_body.pre_session_id != taker_body.pre_session_id {
            return Err(BtcRoleContributionV1Error::PreSessionMismatch);
        }
        if maker_body.direction != taker_body.direction {
            return Err(BtcRoleContributionV1Error::DirectionMismatch);
        }
        if maker_body.bitcoin_chain_policy != taker_body.bitcoin_chain_policy
            || maker_body.lez_chain_identity != taker_body.lez_chain_identity
        {
            return Err(BtcRoleContributionV1Error::ChainIdentityMismatch);
        }
        if maker_body.expires_at_unix_seconds != taker_body.expires_at_unix_seconds {
            return Err(BtcRoleContributionV1Error::ExpiryMismatch);
        }
        let maker_identity = &maker_body.participant_identity;
        let taker_identity = &taker_body.participant_identity;
        if maker_identity.musig2_public_key() == taker_identity.musig2_public_key()
            || maker_identity.lez_owner_account() == taker_identity.lez_owner_account()
            || maker_body.bitcoin_funding_key == taker_body.bitcoin_funding_key
            || maker_body.role_entropy == taker_body.role_entropy
        {
            return Err(BtcRoleContributionV1Error::AliasedRoleIdentity);
        }
        let swap_id = derive_joint_swap_id(
            &maker_body.pre_session_id,
            maker.contribution_commitment(),
            taker.contribution_commitment(),
        );
        Ok(Self {
            maker,
            taker,
            swap_id,
        })
    }

    /// Maker-owned validated contribution.
    #[must_use]
    pub const fn maker(&self) -> &BtcRoleContributionV1 {
        &self.maker
    }

    /// Taker-owned validated contribution.
    #[must_use]
    pub const fn taker(&self) -> &BtcRoleContributionV1 {
        &self.taker
    }

    /// Joint swap identity committed by the final agreement body.
    #[must_use]
    pub const fn swap_id(&self) -> &[u8; 32] {
        &self.swap_id
    }

    /// Canonical fixed Maker-then-Taker participant order.
    #[must_use]
    pub fn participants(&self) -> BtcParticipantsV1 {
        BtcParticipantsV1::new(
            self.maker.body().participant_identity.clone(),
            self.taker.body().participant_identity.clone(),
        )
    }

    /// Taker-owned public adaptor point committed by the pair.
    #[must_use]
    pub fn adaptor_point(&self) -> &[u8; 33] {
        match self.taker.body().adaptor_point.as_ref() {
            Some(point) => point,
            None => unreachable!("validated Taker contribution has an adaptor point"),
        }
    }

    /// Audits every agreement-v1 field whose authority originates in the two
    /// role contributions, including the jointly derived swap identity.
    ///
    /// Agreement-v1 already commits every executable chain field; this method
    /// supplies the missing transitive check from the pre-agreement transcript
    /// to that body without introducing a second agreement schema.
    ///
    /// # Errors
    ///
    /// Rejects an invalid acceptance time, expired contributions, or any swap,
    /// direction, participant, adaptor, Bitcoin-policy, or LEZ-identity drift.
    pub fn validate_agreement_body(
        &self,
        body: &BtcAgreementBodyV1,
        accepted_at_unix_seconds: u64,
    ) -> Result<(), BtcRoleContributionV1Error> {
        if accepted_at_unix_seconds == 0
            || accepted_at_unix_seconds >= self.maker.body().expires_at_unix_seconds()
        {
            return Err(BtcRoleContributionV1Error::ContributionExpired);
        }
        self.validate_agreement_body_fields(body)
    }

    /// Revalidates the immutable contribution-to-agreement binding without
    /// applying a new wall-clock acceptance decision.
    ///
    /// This is the restart-safe counterpart to [`Self::validate_agreement_body`]:
    /// callers use it only after the original acceptance point was durably
    /// recorded, so an already accepted agreement does not become invalid merely
    /// because its contribution exchange has since expired.
    ///
    /// # Errors
    ///
    /// Rejects any swap, direction, participant, adaptor, Bitcoin-policy, or
    /// LEZ-identity drift.
    pub fn validate_agreement_body_fields(
        &self,
        body: &BtcAgreementBodyV1,
    ) -> Result<(), BtcRoleContributionV1Error> {
        let contribution = self.maker.body();
        let chain = contribution.lez_chain_identity();
        let lez = body.lez_terms();
        if self.swap_id() != body.swap_id()
            || contribution.direction() != body.direction()
            || self.participants() != *body.participants()
            || self.adaptor_point() != body.adaptor_point()
            || contribution.bitcoin_chain_policy() != body.bitcoin_chain_policy()
            || chain.genesis_block_hash() != lez.genesis_block_hash()
            || chain.channel_id() != lez.channel_id()
            || chain.escrow_program_id() != lez.escrow_program_id()
            || chain.authenticated_transfer_program_id() != lez.authenticated_transfer_program_id()
        {
            return Err(BtcRoleContributionV1Error::AgreementBindingMismatch);
        }
        Ok(())
    }
}

/// Derives the non-circular offer/reservation binding used by both roles.
///
/// # Errors
///
/// Rejects a zero offer commitment or an empty/oversized reservation binding.
pub fn derive_btc_pre_session_id_v1(
    offer_commitment: &[u8; 32],
    reservation_binding: &[u8],
    direction: SwapDirection,
) -> Result<[u8; 32], BtcRoleContributionV1Error> {
    if offer_commitment == &[0; 32] {
        return Err(BtcRoleContributionV1Error::InvalidOfferCommitment);
    }
    if reservation_binding.is_empty()
        || reservation_binding.len() > MAX_BTC_PRE_SESSION_RESERVATION_BYTES
    {
        return Err(BtcRoleContributionV1Error::InvalidReservationBinding);
    }
    let reservation_len = u32::try_from(reservation_binding.len())
        .map_err(|_| BtcRoleContributionV1Error::InvalidReservationBinding)?;
    let mut hasher = Sha256::new();
    hasher.update(BTC_PRE_SESSION_V1_DOMAIN);
    hasher.update(offer_commitment);
    hasher.update(reservation_len.to_le_bytes());
    hasher.update(reservation_binding);
    hasher.update(match direction {
        SwapDirection::TakerSellsForeign => b"taker-sells-foreign".as_slice(),
        SwapDirection::TakerSellsLez => b"taker-sells-lez".as_slice(),
    });
    Ok(hasher.finalize().into())
}

fn derive_joint_swap_id(
    pre_session_id: &[u8; 32],
    maker_commitment: &[u8; 32],
    taker_commitment: &[u8; 32],
) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(BTC_JOINT_SWAP_ID_V1_DOMAIN);
    hasher.update(pre_session_id);
    hasher.update(maker_commitment);
    hasher.update(taker_commitment);
    hasher.finalize().into()
}

/// Bounded public-contribution validation failure.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum BtcRoleContributionV1Error {
    /// Wire schema is not supported.
    #[error("unsupported BTC role-contribution schema {0}")]
    UnsupportedSchema(u16),
    /// Canonical record exceeds the network bound.
    #[error("BTC role-contribution wire has {actual} bytes; maximum is {maximum}")]
    OversizedWireRecord {
        /// Actual byte count.
        actual: usize,
        /// Maximum accepted byte count.
        maximum: usize,
    },
    /// Canonical Borsh encoding failed.
    #[error("BTC role-contribution encoding failed")]
    WireEncoding,
    /// Wire bytes are truncated, trailing, or non-canonical.
    #[error("BTC role-contribution wire is malformed")]
    MalformedWireRecord,
    /// Offer commitment is the forbidden zero identity.
    #[error("BTC pre-session offer commitment is invalid")]
    InvalidOfferCommitment,
    /// Reservation binding is empty or oversized.
    #[error("BTC pre-session reservation binding is invalid")]
    InvalidReservationBinding,
    /// Pre-session identity or role entropy is zero.
    #[error("BTC role-contribution session identity is invalid")]
    InvalidSessionIdentity,
    /// Bitcoin chain identity or confirmation policy is unsafe.
    #[error("BTC role-contribution Bitcoin policy is invalid")]
    InvalidBitcoinPolicy,
    /// LEZ chain/program identity is zero or aliased.
    #[error("BTC role-contribution LEZ identity is invalid")]
    InvalidLezIdentity,
    /// Participant public identity is malformed or oversized.
    #[error("BTC role-contribution participant identity is invalid")]
    InvalidParticipantIdentity,
    /// Maker/Taker adaptor-point ownership is malformed.
    #[error("BTC role-contribution adaptor point is invalid for its role")]
    InvalidAdaptorPoint,
    /// Signed expiry is zero.
    #[error("BTC role-contribution expiry is invalid")]
    InvalidExpiry,
    /// Encoded commitment differs from the canonical body commitment.
    #[error("BTC role-contribution commitment mismatch")]
    CommitmentMismatch,
    /// Schnorr signature encoding is malformed.
    #[error("BTC role-contribution signature is malformed")]
    InvalidSignatureEncoding,
    /// Schnorr proof-of-possession does not verify.
    #[error("BTC role-contribution signature does not verify")]
    SignatureMismatch,
    /// Pair does not contain Maker then Taker.
    #[error("BTC role-contribution pair roles mismatch")]
    PairRoleMismatch,
    /// Pair contributions refer to different pre-sessions.
    #[error("BTC role-contribution pre-session mismatch")]
    PreSessionMismatch,
    /// Pair contributions select different directions.
    #[error("BTC role-contribution direction mismatch")]
    DirectionMismatch,
    /// Pair contributions select different chain identities.
    #[error("BTC role-contribution chain identity mismatch")]
    ChainIdentityMismatch,
    /// Pair contributions have different expiries.
    #[error("BTC role-contribution expiry mismatch")]
    ExpiryMismatch,
    /// Pair aliases a role key, LEZ account, or entropy value.
    #[error("BTC role-contribution pair aliases role identity")]
    AliasedRoleIdentity,
    /// Contribution authority expired before the local acceptance point.
    #[error("BTC role contributions expired before agreement acceptance")]
    ContributionExpired,
    /// Final agreement fields differ from the signed role contributions.
    #[error("BTC agreement differs from its signed role contributions")]
    AgreementBindingMismatch,
}

fn validate_body(body: &BtcRoleContributionBodyV1) -> Result<(), BtcRoleContributionV1Error> {
    if body.pre_session_id == [0; 32] || body.role_entropy == [0; 32] {
        return Err(BtcRoleContributionV1Error::InvalidSessionIdentity);
    }
    let policy = body.bitcoin_chain_policy;
    if policy.genesis_block_hash() == &[0; 32]
        || policy.required_confirmations() == 0
        || policy.required_confirmations() > MAX_BITCOIN_REQUIRED_CONFIRMATIONS
    {
        return Err(BtcRoleContributionV1Error::InvalidBitcoinPolicy);
    }
    let lez = body.lez_chain_identity;
    if lez.genesis_block_hash == [0; 32]
        || lez.channel_id == [0; 32]
        || lez.escrow_program_id == [0; 32]
        || lez.authenticated_transfer_program_id == [0; 32]
        || lez.escrow_program_id == lez.authenticated_transfer_program_id
    {
        return Err(BtcRoleContributionV1Error::InvalidLezIdentity);
    }
    let identity = &body.participant_identity;
    let participant_key = PublicKey::from_slice(identity.musig2_public_key())
        .map_err(|_| BtcRoleContributionV1Error::InvalidParticipantIdentity)?;
    if participant_key.serialize() != *identity.musig2_public_key()
        || XOnlyPublicKey::from_slice(identity.bitcoin_refund_key()).is_err()
        || identity.lez_owner_account() == &[0; 32]
        || identity.claim_destination_script_pubkey().is_empty()
        || identity.claim_destination_script_pubkey().len() > MAX_CLAIM_DESTINATION_SCRIPT_BYTES
    {
        return Err(BtcRoleContributionV1Error::InvalidParticipantIdentity);
    }
    if XOnlyPublicKey::from_slice(&body.bitcoin_funding_key).is_err() {
        return Err(BtcRoleContributionV1Error::InvalidParticipantIdentity);
    }
    match (body.role(), body.adaptor_point) {
        (Participant::Maker, None) => {}
        (Participant::Taker, Some(point)) => {
            let parsed = PublicKey::from_slice(&point)
                .map_err(|_| BtcRoleContributionV1Error::InvalidAdaptorPoint)?;
            if parsed.serialize() != point {
                return Err(BtcRoleContributionV1Error::InvalidAdaptorPoint);
            }
        }
        (Participant::Maker, Some(_)) | (Participant::Taker, None) => {
            return Err(BtcRoleContributionV1Error::InvalidAdaptorPoint);
        }
    }
    if body.expires_at_unix_seconds == 0 {
        return Err(BtcRoleContributionV1Error::InvalidExpiry);
    }
    Ok(())
}

fn verify_role_signature(
    body: &BtcRoleContributionBodyV1,
    signature_bytes: [u8; 64],
    commitment: [u8; 32],
) -> Result<(), BtcRoleContributionV1Error> {
    let signature = Signature::from_slice(&signature_bytes)
        .map_err(|_| BtcRoleContributionV1Error::InvalidSignatureEncoding)?;
    let public_key = PublicKey::from_slice(body.participant_identity.musig2_public_key())
        .map_err(|_| BtcRoleContributionV1Error::InvalidParticipantIdentity)?;
    let (x_only, _) = public_key.x_only_public_key();
    Secp256k1::verification_only()
        .verify_schnorr(&signature, &Message::from_digest(commitment), &x_only)
        .map_err(|_| BtcRoleContributionV1Error::SignatureMismatch)
}

fn decode_bounded_record(
    bytes: &[u8],
) -> Result<BtcRoleContributionRecordV1, BtcRoleContributionV1Error> {
    let mut reader = BoundedWireReader::new(bytes);
    let schema_version = reader.u16()?;
    let pre_session_id = reader.fixed()?;
    let role = match reader.u8()? {
        0 => BtcContributionRoleRecordV1::Maker,
        1 => BtcContributionRoleRecordV1::Taker,
        _ => return Err(BtcRoleContributionV1Error::MalformedWireRecord),
    };
    let direction = match reader.u8()? {
        0 => BtcContributionDirectionRecordV1::TakerSellsForeign,
        1 => BtcContributionDirectionRecordV1::TakerSellsLez,
        _ => return Err(BtcRoleContributionV1Error::MalformedWireRecord),
    };
    let bitcoin_chain_policy = BtcChainPolicyV1::new(reader.fixed()?, reader.u32()?);
    let lez_chain_identity = BtcLezChainIdentityV1::new(
        reader.fixed()?,
        reader.fixed()?,
        reader.fixed()?,
        reader.fixed()?,
    );
    let participant_identity = BtcParticipantIdentityV1::new(
        reader.fixed()?,
        reader.fixed()?,
        reader.fixed()?,
        reader.vec(MAX_CLAIM_DESTINATION_SCRIPT_BYTES)?,
    );
    let bitcoin_funding_key = reader.fixed()?;
    let adaptor_point = match reader.u8()? {
        0 => None,
        1 => Some(reader.fixed()?),
        _ => return Err(BtcRoleContributionV1Error::MalformedWireRecord),
    };
    let role_entropy = reader.fixed()?;
    let expires_at_unix_seconds = reader.u64()?;
    let contribution_commitment = reader.fixed()?;
    let role_signature = reader.fixed()?;
    reader.finish()?;
    Ok(BtcRoleContributionRecordV1::from_parts(
        schema_version,
        BtcRoleContributionBodyV1 {
            pre_session_id,
            role,
            direction,
            bitcoin_chain_policy,
            lez_chain_identity,
            participant_identity,
            bitcoin_funding_key,
            adaptor_point,
            role_entropy,
            expires_at_unix_seconds,
        },
        contribution_commitment,
        role_signature,
    ))
}

struct BoundedWireReader<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> BoundedWireReader<'a> {
    const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }

    fn take(&mut self, length: usize) -> Result<&'a [u8], BtcRoleContributionV1Error> {
        let end = self
            .offset
            .checked_add(length)
            .ok_or(BtcRoleContributionV1Error::MalformedWireRecord)?;
        let value = self
            .bytes
            .get(self.offset..end)
            .ok_or(BtcRoleContributionV1Error::MalformedWireRecord)?;
        self.offset = end;
        Ok(value)
    }

    fn fixed<const N: usize>(&mut self) -> Result<[u8; N], BtcRoleContributionV1Error> {
        self.take(N)?
            .try_into()
            .map_err(|_| BtcRoleContributionV1Error::MalformedWireRecord)
    }

    fn u8(&mut self) -> Result<u8, BtcRoleContributionV1Error> {
        Ok(self.fixed::<1>()?[0])
    }

    fn u16(&mut self) -> Result<u16, BtcRoleContributionV1Error> {
        Ok(u16::from_le_bytes(self.fixed()?))
    }

    fn u32(&mut self) -> Result<u32, BtcRoleContributionV1Error> {
        Ok(u32::from_le_bytes(self.fixed()?))
    }

    fn u64(&mut self) -> Result<u64, BtcRoleContributionV1Error> {
        Ok(u64::from_le_bytes(self.fixed()?))
    }

    fn vec(&mut self, maximum: usize) -> Result<Vec<u8>, BtcRoleContributionV1Error> {
        let length = usize::try_from(self.u32()?)
            .map_err(|_| BtcRoleContributionV1Error::MalformedWireRecord)?;
        if length > maximum {
            return Err(BtcRoleContributionV1Error::MalformedWireRecord);
        }
        Ok(self.take(length)?.to_vec())
    }

    fn finish(self) -> Result<(), BtcRoleContributionV1Error> {
        if self.offset == self.bytes.len() {
            Ok(())
        } else {
            Err(BtcRoleContributionV1Error::MalformedWireRecord)
        }
    }
}
