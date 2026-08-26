//! Canonical countersigned LEZ/XMR agreement and claim-release boundary.
//!
//! Version 1 supports only Taker-sells-LEZ. The Taker locks LEZ, the Maker
//! funds the exact shared Monero address, and the Taker releases its owner-local
//! claim partial only after an exact canonical Monero output is unlocked and
//! has the countersigned confirmation depth.

use lez_adaptor_signature::{
    AdaptorSessionContext, AdaptorSessionError, SigningRole, aggregate_adaptor_presignature,
    verify_adaptor_partial_signature, verify_nonce_commitment,
};
use lez_swap_core::{
    Chain, ChainPosition, ConfirmationPolicy, LezUnixMilliseconds, Pair, RecoverySchedule,
    SwapCoordinator, SwapDirection, SwapId,
};
use musig2::{KeyAggContext, secp::Point as MusigPoint};
use secp256k1::{
    Message, PublicKey, Secp256k1, XOnlyPublicKey, schnorr::Signature as SchnorrSignature,
};
use sha2::{Digest as _, Sha256};
use thiserror::Error;

use crate::{
    CrossCurveDleqError, CrossCurveDleqProofV1, MoneroAddressNetworkV1, MoneroPrivateViewKey,
    MoneroSharedAddressV1, MoneroSharedSpendError,
};

/// Version accepted by the countersigned agreement wire boundary.
pub const XMR_AGREEMENT_SCHEMA_V1: u16 = 1;
/// Version accepted by the countersigned activation wire boundary.
pub const XMR_ACTIVATION_SCHEMA_V1: u16 = 1;
/// Maximum canonical agreement wire size accepted from an untrusted peer.
pub const MAX_XMR_AGREEMENT_WIRE_BYTES: usize = 270 * 1024;
/// Maximum canonical fixed-width activation record.
pub const MAX_XMR_ACTIVATION_WIRE_BYTES: usize = 2 * 1024;
/// Maximum canonical unsigned Stage-A prefix through its commitment.
pub const MAX_XMR_UNSIGNED_STAGE_A_WIRE_BYTES: usize = MAX_XMR_AGREEMENT_WIRE_BYTES - 128;
/// Maximum canonical unsigned Stage-B prefix through its commitment.
pub const MAX_XMR_UNSIGNED_STAGE_B_WIRE_BYTES: usize = MAX_XMR_ACTIVATION_WIRE_BYTES - 128;

const MAX_DLEQ_WIRE_BYTES: usize = 129 * 1024;
const MAX_MONERO_ADDRESS_BYTES: usize = 128;
const AGREEMENT_DOMAIN: &[u8] = b"logos.gateway.lez-xmr.agreement.v1\0";
const SESSION_DOMAIN: &[u8] = b"logos.gateway.lez-xmr.adaptor-session.v1\0";
const ACTIVATION_DOMAIN: &[u8] = b"logos.gateway.lez-xmr.activation.v1\0";
const CLAIM_PARTIAL_COMMITMENT_DOMAIN: &[u8] =
    b"logos.gateway.lez-xmr.claim-partial-commitment.v1\0";
const CLAIM_PARTIAL_CONTEXT_DOMAIN: &[u8] = b"logos.gateway.lez-xmr.claim-partial-context.v1\0";
// Pinned LEZ v0.2.0 public-key-to-account mapping.
const PUBLIC_ACCOUNT_ID_PREFIX: &[u8; 32] = b"/LEE/v0.3/AccountId/Public/\0\0\0\0\0";

const XMR_CONFIRMATIONS: u32 = 10;
const LEZ_FINALITY_UNITS: u32 = 2;
const REGTEST_MINIMUM_FUNDING_TO_REFUND_MS: u64 = 10_000;
const REGTEST_MINIMUM_REFUND_TO_PUNISH_MS: u64 = 10_000;
const STAGENET_MINIMUM_FUNDING_TO_REFUND_MS: u64 = 12 * 60 * 60 * 1_000;
const STAGENET_MINIMUM_REFUND_TO_PUNISH_MS: u64 = 2 * 60 * 60 * 1_000;
const XMR_METADATA_VERSION_V3: u8 = 3;

/// Immutable network and safety policy selected before negotiation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum XmrNamedProfileV1 {
    /// Controlled local nodes with accelerated, whole-second recovery windows.
    AcceleratedRegtest,
    /// Public Stagenet policy from the reviewed M1 profile.
    PublicStagenet,
}

impl XmrNamedProfileV1 {
    const fn tag(self) -> u8 {
        match self {
            Self::AcceleratedRegtest => 0,
            Self::PublicStagenet => 1,
        }
    }

    fn from_tag(tag: u8) -> Result<Self, XmrAgreementV1Error> {
        match tag {
            0 => Ok(Self::AcceleratedRegtest),
            1 => Ok(Self::PublicStagenet),
            _ => Err(XmrAgreementV1Error::MalformedWire),
        }
    }

    const fn network(self) -> MoneroAddressNetworkV1 {
        match self {
            Self::AcceleratedRegtest => MoneroAddressNetworkV1::Regtest,
            Self::PublicStagenet => MoneroAddressNetworkV1::Stagenet,
        }
    }

    const fn minimum_funding_to_refund_ms(self) -> u64 {
        match self {
            Self::AcceleratedRegtest => REGTEST_MINIMUM_FUNDING_TO_REFUND_MS,
            Self::PublicStagenet => STAGENET_MINIMUM_FUNDING_TO_REFUND_MS,
        }
    }

    const fn minimum_refund_to_punish_ms(self) -> u64 {
        match self {
            Self::AcceleratedRegtest => REGTEST_MINIMUM_REFUND_TO_PUNISH_MS,
            Self::PublicStagenet => STAGENET_MINIMUM_REFUND_TO_PUNISH_MS,
        }
    }

    /// Exact Monero confirmation depth for this profile.
    #[must_use]
    pub const fn required_monero_confirmations(self) -> u32 {
        XMR_CONFIRMATIONS
    }

    /// Exact finalized LEZ observation units for this profile.
    #[must_use]
    pub const fn required_lez_finality_units(self) -> u32 {
        LEZ_FINALITY_UNITS
    }
}

/// Immutable actor role in agreement and validation errors.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum XmrRoleV1 {
    /// Liquidity provider, Monero funder, and LEZ claimant.
    Maker,
    /// Swap initiator, LEZ depositor, and Monero recipient.
    Taker,
}

/// Direction encoded on wire; version 1 accepts only Taker-sells-LEZ.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum XmrSwapDirectionV1 {
    /// Taker deposits LEZ before Maker funds Monero.
    TakerSellsLez,
    /// Reserved unsupported direction, retained for typed fail-closed parsing.
    TakerSellsXmr,
}

impl XmrSwapDirectionV1 {
    const fn tag(self) -> u8 {
        match self {
            Self::TakerSellsLez => 0,
            Self::TakerSellsXmr => 1,
        }
    }

    fn from_tag(tag: u8) -> Result<Self, XmrAgreementV1Error> {
        match tag {
            0 => Ok(Self::TakerSellsLez),
            1 => Ok(Self::TakerSellsXmr),
            _ => Err(XmrAgreementV1Error::MalformedWire),
        }
    }
}

/// Exact public identity assigned to one protocol role.
///
/// Agreement authentication, claim `MuSig2`, and refund `MuSig2` use three
/// independent compressed keys. Validation rejects every alias across both
/// roles, preventing a nonce or authority from crossing purposes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct XmrParticipantIdentityV1 {
    lez_owner_account: [u8; 32],
    agreement_public_key: [u8; 33],
    claim_session_public_key: [u8; 33],
    refund_session_public_key: [u8; 33],
}

impl XmrParticipantIdentityV1 {
    /// Creates an untrusted role identity; agreement validation parses every field.
    #[must_use]
    pub const fn new(
        lez_owner_account: [u8; 32],
        agreement_public_key: [u8; 33],
        claim_session_public_key: [u8; 33],
        refund_session_public_key: [u8; 33],
    ) -> Self {
        Self {
            lez_owner_account,
            agreement_public_key,
            claim_session_public_key,
            refund_session_public_key,
        }
    }

    /// Exact LEZ owner account authorized for this role.
    #[must_use]
    pub const fn lez_owner_account(&self) -> [u8; 32] {
        self.lez_owner_account
    }

    /// BIP340 key authenticating the complete agreement body.
    #[must_use]
    pub const fn agreement_public_key(&self) -> [u8; 33] {
        self.agreement_public_key
    }

    /// Purpose-exclusive `MuSig2` key for the Maker-share-adapted claim.
    #[must_use]
    pub const fn claim_session_public_key(&self) -> [u8; 33] {
        self.claim_session_public_key
    }

    /// Purpose-exclusive `MuSig2` key for the Taker-share-adapted signed refund.
    #[must_use]
    pub const fn refund_session_public_key(&self) -> [u8; 33] {
        self.refund_session_public_key
    }
}

/// Maker and Taker identities committed in fixed order.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct XmrParticipantsV1 {
    maker: XmrParticipantIdentityV1,
    taker: XmrParticipantIdentityV1,
}

impl XmrParticipantsV1 {
    /// Creates the exact role-indexed identities.
    #[must_use]
    pub const fn new(maker: XmrParticipantIdentityV1, taker: XmrParticipantIdentityV1) -> Self {
        Self { maker, taker }
    }

    /// Returns one immutable role identity.
    #[must_use]
    pub const fn for_role(&self, role: XmrRoleV1) -> &XmrParticipantIdentityV1 {
        match role {
            XmrRoleV1::Maker => &self.maker,
            XmrRoleV1::Taker => &self.taker,
        }
    }

    /// Derives the x-only aggregate key for the purpose-exclusive claim keys.
    ///
    /// # Errors
    ///
    /// Rejects malformed, aliased, or non-aggregatable participant keys.
    pub fn claim_aggregate_x_only_key(&self) -> Result<[u8; 32], XmrAgreementV1Error> {
        validate_participant_keys(self)?;
        aggregate_x_only([
            self.maker.claim_session_public_key,
            self.taker.claim_session_public_key,
        ])
    }

    /// Derives the x-only aggregate key for the purpose-exclusive refund keys.
    ///
    /// # Errors
    ///
    /// Rejects malformed, aliased, or non-aggregatable participant keys.
    pub fn refund_aggregate_x_only_key(&self) -> Result<[u8; 32], XmrAgreementV1Error> {
        validate_participant_keys(self)?;
        aggregate_x_only([
            self.maker.refund_session_public_key,
            self.taker.refund_session_public_key,
        ])
    }
}

/// Exact Monero lock terms and both role-owned cross-curve proofs.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct XmrMoneroTermsV1 {
    network: MoneroAddressNetworkV1,
    genesis_hash: [u8; 32],
    amount_piconero: u64,
    required_confirmations: u32,
    maker_dleq_proof_wire: Vec<u8>,
    taker_dleq_proof_wire: Vec<u8>,
    public_view_key: [u8; 32],
    public_spend_key: [u8; 32],
    address: String,
}

impl XmrMoneroTermsV1 {
    /// Creates untrusted exact Monero terms; agreement validation re-derives
    /// both proof points, the aggregate spend key, and the address.
    #[allow(clippy::too_many_arguments)]
    #[must_use]
    pub fn new(
        network: MoneroAddressNetworkV1,
        genesis_hash: [u8; 32],
        amount_piconero: u64,
        required_confirmations: u32,
        maker_dleq_proof_wire: Vec<u8>,
        taker_dleq_proof_wire: Vec<u8>,
        public_view_key: [u8; 32],
        public_spend_key: [u8; 32],
        address: impl Into<String>,
    ) -> Self {
        Self {
            network,
            genesis_hash,
            amount_piconero,
            required_confirmations,
            maker_dleq_proof_wire,
            taker_dleq_proof_wire,
            public_view_key,
            public_spend_key,
            address: address.into(),
        }
    }

    /// Exact Monero address domain.
    #[must_use]
    pub const fn network(&self) -> MoneroAddressNetworkV1 {
        self.network
    }
    /// Exact daemon genesis identity.
    #[must_use]
    pub const fn genesis_hash(&self) -> [u8; 32] {
        self.genesis_hash
    }
    /// Exact output principal in piconero.
    #[must_use]
    pub const fn amount_piconero(&self) -> u64 {
        self.amount_piconero
    }
    /// Minimum canonical confirmations before claim-partial release.
    #[must_use]
    pub const fn required_confirmations(&self) -> u32 {
        self.required_confirmations
    }
    /// Exact canonical Maker-owned DLEQ proof wire.
    #[must_use]
    pub fn maker_dleq_proof_wire(&self) -> &[u8] {
        &self.maker_dleq_proof_wire
    }
    /// Exact canonical Taker-owned DLEQ proof wire.
    #[must_use]
    pub fn taker_dleq_proof_wire(&self) -> &[u8] {
        &self.taker_dleq_proof_wire
    }
    /// Exact public view key used by the shared wallet.
    #[must_use]
    pub const fn public_view_key(&self) -> [u8; 32] {
        self.public_view_key
    }
    /// Re-derived aggregate public spend key.
    #[must_use]
    pub const fn public_spend_key(&self) -> [u8; 32] {
        self.public_spend_key
    }
    /// Exact standard address funded by the Maker.
    #[must_use]
    pub fn address(&self) -> &str {
        &self.address
    }
}

/// Exact LEZ deployment, accounts, authorities, windows, and principal.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct XmrLezTermsV1 {
    channel_id: [u8; 32],
    genesis_hash: [u8; 32],
    escrow_program_id: [u32; 8],
    authenticated_transfer_program_id: [u32; 8],
    required_finality_units: u32,
    metadata_account: [u8; 32],
    custody_account: [u8; 32],
    depositor_account: [u8; 32],
    claimant_account: [u8; 32],
    claim_aggregate_x_only_key: [u8; 32],
    claim_authority_account: [u8; 32],
    refund_aggregate_x_only_key: [u8; 32],
    refund_authority_account: [u8; 32],
    maker_dleq_transcript_commitment: [u8; 32],
    taker_dleq_transcript_commitment: [u8; 32],
    amount: u128,
}

impl XmrLezTermsV1 {
    /// Creates untrusted exact LEZ terms.
    #[allow(clippy::too_many_arguments)]
    #[must_use]
    pub const fn new(
        channel_id: [u8; 32],
        genesis_hash: [u8; 32],
        escrow_program_id: [u32; 8],
        authenticated_transfer_program_id: [u32; 8],
        required_finality_units: u32,
        metadata_account: [u8; 32],
        custody_account: [u8; 32],
        depositor_account: [u8; 32],
        claimant_account: [u8; 32],
        claim_aggregate_x_only_key: [u8; 32],
        claim_authority_account: [u8; 32],
        refund_aggregate_x_only_key: [u8; 32],
        refund_authority_account: [u8; 32],
        maker_dleq_transcript_commitment: [u8; 32],
        taker_dleq_transcript_commitment: [u8; 32],
        amount: u128,
    ) -> Self {
        Self {
            channel_id,
            genesis_hash,
            escrow_program_id,
            authenticated_transfer_program_id,
            required_finality_units,
            metadata_account,
            custody_account,
            depositor_account,
            claimant_account,
            claim_aggregate_x_only_key,
            claim_authority_account,
            refund_aggregate_x_only_key,
            refund_authority_account,
            maker_dleq_transcript_commitment,
            taker_dleq_transcript_commitment,
            amount,
        }
    }

    /// Exact LEZ channel identifier.
    #[must_use]
    pub const fn channel_id(&self) -> [u8; 32] {
        self.channel_id
    }
    /// Exact LEZ genesis identity.
    #[must_use]
    pub const fn genesis_hash(&self) -> [u8; 32] {
        self.genesis_hash
    }
    /// Exact LEZ escrow program ID.
    #[must_use]
    pub const fn escrow_program_id(&self) -> [u32; 8] {
        self.escrow_program_id
    }
    /// Exact authenticated-transfer program used by native custody.
    #[must_use]
    pub const fn authenticated_transfer_program_id(&self) -> [u32; 8] {
        self.authenticated_transfer_program_id
    }
    /// Exact finalized LEZ observation depth.
    #[must_use]
    pub const fn required_finality_units(&self) -> u32 {
        self.required_finality_units
    }
    /// Exact metadata account.
    #[must_use]
    pub const fn metadata_account(&self) -> [u8; 32] {
        self.metadata_account
    }
    /// Exact custody account.
    #[must_use]
    pub const fn custody_account(&self) -> [u8; 32] {
        self.custody_account
    }
    /// Taker owner account that deposits LEZ.
    #[must_use]
    pub const fn depositor_account(&self) -> [u8; 32] {
        self.depositor_account
    }
    /// Maker owner account that claims LEZ.
    #[must_use]
    pub const fn claimant_account(&self) -> [u8; 32] {
        self.claimant_account
    }
    /// Aggregate x-only key for the claim branch.
    #[must_use]
    pub const fn claim_aggregate_x_only_key(&self) -> [u8; 32] {
        self.claim_aggregate_x_only_key
    }
    /// LEZ account derived from the claim aggregate key.
    #[must_use]
    pub const fn claim_authority_account(&self) -> [u8; 32] {
        self.claim_authority_account
    }
    /// Aggregate x-only key for the signed-refund branch.
    #[must_use]
    pub const fn refund_aggregate_x_only_key(&self) -> [u8; 32] {
        self.refund_aggregate_x_only_key
    }
    /// LEZ account derived from the refund aggregate key.
    #[must_use]
    pub const fn refund_authority_account(&self) -> [u8; 32] {
        self.refund_authority_account
    }
    /// Maker DLEQ envelope commitment stored by the guest.
    #[must_use]
    pub const fn maker_dleq_transcript_commitment(&self) -> [u8; 32] {
        self.maker_dleq_transcript_commitment
    }
    /// Taker DLEQ envelope commitment stored by the guest.
    #[must_use]
    pub const fn taker_dleq_transcript_commitment(&self) -> [u8; 32] {
        self.taker_dleq_transcript_commitment
    }
    /// Exact LEZ principal.
    #[must_use]
    pub const fn amount(&self) -> u128 {
        self.amount
    }

    /// Derives the exact LEZ v0.2 public authority account for an x-only key.
    #[must_use]
    pub fn authority_account_for_key(x_only_public_key: [u8; 32]) -> [u8; 32] {
        witnessed_account_id(x_only_public_key)
    }
}

/// Three distinct exact LEZ signature messages.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct XmrMessagesV1 {
    claim: [u8; 32],
    refund: [u8; 32],
    punish: [u8; 32],
}

impl XmrMessagesV1 {
    /// Creates untrusted claim, signed-refund, and punishment messages.
    #[must_use]
    pub const fn new(claim: [u8; 32], refund: [u8; 32], punish: [u8; 32]) -> Self {
        Self {
            claim,
            refund,
            punish,
        }
    }
    /// Maker-share-adapted successful claim message.
    #[must_use]
    pub const fn claim(&self) -> [u8; 32] {
        self.claim
    }
    /// Taker-share-adapted signed-refund message.
    #[must_use]
    pub const fn refund(&self) -> [u8; 32] {
        self.refund
    }
    /// Maker punishment message after the final timeout.
    #[must_use]
    pub const fn punish(&self) -> [u8; 32] {
        self.punish
    }
}

/// Distinct LEZ refund and punishment validity windows.
#[allow(clippy::struct_field_names)] // The unit suffix is part of the public wire contract.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct XmrWindowsV1 {
    maker_xmr_funding_cutoff_ms: u64,
    refund_at_ms: u64,
    punish_at_ms: u64,
}

impl XmrWindowsV1 {
    /// Creates untrusted guest-time boundaries.
    #[must_use]
    pub const fn new(
        maker_xmr_funding_cutoff_ms: u64,
        refund_at_ms: u64,
        punish_at_ms: u64,
    ) -> Self {
        Self {
            maker_xmr_funding_cutoff_ms,
            refund_at_ms,
            punish_at_ms,
        }
    }
    /// Latest LEZ consensus-clock instant at which Maker may fund Monero.
    #[must_use]
    pub const fn maker_xmr_funding_cutoff_ms(&self) -> u64 {
        self.maker_xmr_funding_cutoff_ms
    }
    /// Earliest Taker signed-refund time.
    #[must_use]
    pub const fn refund_at_ms(&self) -> u64 {
        self.refund_at_ms
    }
    /// Earliest Maker punishment time; must be later than refund.
    #[must_use]
    pub const fn punish_at_ms(&self) -> u64 {
        self.punish_at_ms
    }
}

/// Canonical body countersigned before either chain lock.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct XmrAgreementBodyV1 {
    direction: XmrSwapDirectionV1,
    profile: XmrNamedProfileV1,
    swap_id: [u8; 32],
    participants: XmrParticipantsV1,
    monero: XmrMoneroTermsV1,
    lez: XmrLezTermsV1,
    messages: XmrMessagesV1,
    windows: XmrWindowsV1,
}

impl XmrAgreementBodyV1 {
    /// Creates an untrusted body; agreement validation makes it executable.
    #[allow(clippy::too_many_arguments)] // Mirrors the eight committed protocol sections.
    #[must_use]
    pub const fn new(
        direction: XmrSwapDirectionV1,
        profile: XmrNamedProfileV1,
        swap_id: [u8; 32],
        participants: XmrParticipantsV1,
        monero: XmrMoneroTermsV1,
        lez: XmrLezTermsV1,
        messages: XmrMessagesV1,
        windows: XmrWindowsV1,
    ) -> Self {
        Self {
            direction,
            profile,
            swap_id,
            participants,
            monero,
            lez,
            messages,
            windows,
        }
    }

    /// Domain-separated commitment signed by both agreement keys.
    #[must_use]
    pub fn commitment(&self) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(AGREEMENT_DOMAIN);
        hasher.update(self.encode_body());
        hasher.finalize().into()
    }
    /// Fixed protocol direction.
    #[must_use]
    pub const fn direction(&self) -> XmrSwapDirectionV1 {
        self.direction
    }
    /// Exact immutable network and recovery profile.
    #[must_use]
    pub const fn profile(&self) -> XmrNamedProfileV1 {
        self.profile
    }
    /// Stable binary swap ID.
    #[must_use]
    pub const fn swap_id(&self) -> [u8; 32] {
        self.swap_id
    }
    /// Exact role identities.
    #[must_use]
    pub const fn participants(&self) -> &XmrParticipantsV1 {
        &self.participants
    }
    /// Exact Monero lock terms.
    #[must_use]
    pub const fn monero(&self) -> &XmrMoneroTermsV1 {
        &self.monero
    }
    /// Exact LEZ escrow terms.
    #[must_use]
    pub const fn lez(&self) -> &XmrLezTermsV1 {
        &self.lez
    }
    /// Exact purpose-separated chain messages.
    #[must_use]
    pub const fn messages(&self) -> XmrMessagesV1 {
        self.messages
    }
    /// Exact refund/punishment validity windows.
    #[must_use]
    pub const fn windows(&self) -> XmrWindowsV1 {
        self.windows
    }

    fn encode_body(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(1024);
        bytes.push(self.direction.tag());
        bytes.push(self.profile.tag());
        bytes.extend_from_slice(&self.swap_id);
        encode_identity(&mut bytes, &self.participants.maker);
        encode_identity(&mut bytes, &self.participants.taker);
        bytes.push(network_tag(self.monero.network));
        bytes.extend_from_slice(&self.monero.genesis_hash);
        bytes.extend_from_slice(&self.monero.amount_piconero.to_le_bytes());
        bytes.extend_from_slice(&self.monero.required_confirmations.to_le_bytes());
        encode_vec(&mut bytes, &self.monero.maker_dleq_proof_wire);
        encode_vec(&mut bytes, &self.monero.taker_dleq_proof_wire);
        bytes.extend_from_slice(&self.monero.public_view_key);
        bytes.extend_from_slice(&self.monero.public_spend_key);
        encode_vec(&mut bytes, self.monero.address.as_bytes());
        bytes.extend_from_slice(&self.lez.channel_id);
        bytes.extend_from_slice(&self.lez.genesis_hash);
        for word in self.lez.escrow_program_id {
            bytes.extend_from_slice(&word.to_le_bytes());
        }
        for word in self.lez.authenticated_transfer_program_id {
            bytes.extend_from_slice(&word.to_le_bytes());
        }
        bytes.extend_from_slice(&self.lez.required_finality_units.to_le_bytes());
        bytes.extend_from_slice(&self.lez.metadata_account);
        bytes.extend_from_slice(&self.lez.custody_account);
        bytes.extend_from_slice(&self.lez.depositor_account);
        bytes.extend_from_slice(&self.lez.claimant_account);
        bytes.extend_from_slice(&self.lez.claim_aggregate_x_only_key);
        bytes.extend_from_slice(&self.lez.claim_authority_account);
        bytes.extend_from_slice(&self.lez.refund_aggregate_x_only_key);
        bytes.extend_from_slice(&self.lez.refund_authority_account);
        bytes.extend_from_slice(&self.lez.maker_dleq_transcript_commitment);
        bytes.extend_from_slice(&self.lez.taker_dleq_transcript_commitment);
        bytes.extend_from_slice(&self.lez.amount.to_le_bytes());
        bytes.extend_from_slice(&self.messages.claim);
        bytes.extend_from_slice(&self.messages.refund);
        bytes.extend_from_slice(&self.messages.punish);
        bytes.extend_from_slice(&self.windows.maker_xmr_funding_cutoff_ms.to_le_bytes());
        bytes.extend_from_slice(&self.windows.refund_at_ms.to_le_bytes());
        bytes.extend_from_slice(&self.windows.punish_at_ms.to_le_bytes());
        bytes
    }
}

/// Primitive untrusted wire record containing both BIP340 countersignatures.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct XmrAgreementRecordV1 {
    schema_version: u16,
    body: XmrAgreementBodyV1,
    agreement_commitment: [u8; 32],
    maker_signature: [u8; 64],
    taker_signature: [u8; 64],
}

impl XmrAgreementRecordV1 {
    /// Assembles untrusted wire parts for validation.
    #[must_use]
    pub const fn from_parts(
        schema_version: u16,
        body: XmrAgreementBodyV1,
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

    /// Exact body purportedly signed by both actors.
    #[must_use]
    pub const fn body(&self) -> &XmrAgreementBodyV1 {
        &self.body
    }

    /// Encodes this primitive record under the fixed network bound.
    ///
    /// # Errors
    ///
    /// Rejects any variable field or complete record exceeding its bound.
    pub fn encode_wire(&self) -> Result<Vec<u8>, XmrAgreementV1Error> {
        let mut bytes = encode_unsigned_agreement_wire(
            self.schema_version,
            &self.body,
            self.agreement_commitment,
        )?;
        bytes.extend_from_slice(&self.maker_signature);
        bytes.extend_from_slice(&self.taker_signature);
        Ok(bytes)
    }
}

/// Purpose of one purpose-separated Monero adaptor-signature session.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use]
pub enum XmrAdaptorSessionPurposeV1 {
    /// Claim session adapted to the Maker-owned Monero spend share.
    Claim,
    /// Refund session adapted to the Taker-owned Monero spend share.
    Refund,
}

impl XmrAdaptorSessionPurposeV1 {
    const fn session_label(self) -> &'static [u8] {
        match self {
            Self::Claim => b"claim",
            Self::Refund => b"refund",
        }
    }
}

/// Immutable public inputs for reconstructing one validated adaptor session.
///
/// All fields are copied from an already-validated agreement. The context
/// method revalidates both the purpose-separated session identity and the
/// durable context binding before returning a signing context.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use]
pub struct XmrAdaptorSessionDescriptorV1 {
    purpose: XmrAdaptorSessionPurposeV1,
    agreement_commitment: [u8; 32],
    session_id: [u8; 32],
    exact_message: [u8; 32],
    adaptor_point: [u8; 33],
    ordered_public_keys: [[u8; 33]; 2],
    context_binding: [u8; 32],
}

impl XmrAdaptorSessionDescriptorV1 {
    fn from_validated_context(
        purpose: XmrAdaptorSessionPurposeV1,
        agreement_commitment: [u8; 32],
        context: &AdaptorSessionContext,
    ) -> Self {
        Self {
            purpose,
            agreement_commitment,
            session_id: context.session_id(),
            exact_message: context.message(),
            adaptor_point: context.adaptor_point(),
            ordered_public_keys: context.ordered_public_keys(),
            context_binding: context.durable_context_binding(),
        }
    }

    /// Purpose assigned by the validated agreement.
    #[must_use = "the session purpose must select the matching protocol branch"]
    pub const fn purpose(&self) -> XmrAdaptorSessionPurposeV1 {
        self.purpose
    }

    /// Purpose-separated session identity.
    #[must_use]
    pub const fn session_id(&self) -> [u8; 32] {
        self.session_id
    }

    /// Exact 32-byte chain message signed by this session.
    #[must_use]
    pub const fn exact_message(&self) -> [u8; 32] {
        self.exact_message
    }

    /// Public adaptor point for this session.
    #[must_use]
    pub const fn adaptor_point(&self) -> [u8; 33] {
        self.adaptor_point
    }

    /// Compressed participant keys in canonical Maker/Taker order.
    #[must_use]
    pub const fn ordered_public_keys(&self) -> [[u8; 33]; 2] {
        self.ordered_public_keys
    }

    /// Durable binding to every public input that affects signing.
    #[must_use]
    pub const fn context_binding(&self) -> [u8; 32] {
        self.context_binding
    }

    /// Reconstructs and validates the exact untweaked adaptor context.
    ///
    /// # Errors
    ///
    /// Rejects a descriptor whose purpose/session identity is crossed or whose
    /// public signing fields no longer produce the retained durable binding.
    pub fn context(&self) -> Result<AdaptorSessionContext, XmrAgreementV1Error> {
        if self.session_id != session_id(self.agreement_commitment, self.purpose.session_label()) {
            return Err(XmrAgreementV1Error::AdaptorSessionDescriptorMismatch);
        }
        let context = AdaptorSessionContext::untweaked(
            self.ordered_public_keys,
            self.exact_message,
            self.adaptor_point,
            self.session_id,
        )?;
        if context.durable_context_binding() != self.context_binding {
            return Err(XmrAgreementV1Error::AdaptorSessionDescriptorMismatch);
        }
        Ok(context)
    }
}

/// Semantically validated unsigned Stage-A body ready for role countersigning.
///
/// All fields are private so an untrusted XMR agreement body cannot be
/// mistaken for a body whose bounds, roles, proofs, derived address, windows,
/// and purpose-separated adaptor contexts have been checked.
#[must_use = "a validated Stage-A body must be countersigned or explicitly discarded"]
pub struct ValidatedXmrAgreementBodyV1 {
    body: XmrAgreementBodyV1,
    commitment: [u8; 32],
    maker_agreement_key: PublicKey,
    taker_agreement_key: PublicKey,
    maker_proof: CrossCurveDleqProofV1,
    taker_proof: CrossCurveDleqProofV1,
    shared_address: MoneroSharedAddressV1,
    claim_context: AdaptorSessionContext,
    refund_context: AdaptorSessionContext,
}

impl std::fmt::Debug for ValidatedXmrAgreementBodyV1 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ValidatedXmrAgreementBodyV1")
            .field("swap_id", &self.body.swap_id)
            .field("direction", &self.body.direction)
            .field("commitment", &self.commitment)
            .finish_non_exhaustive()
    }
}

impl ValidatedXmrAgreementBodyV1 {
    /// Validates every unsigned Stage-A field before either role signs it.
    ///
    /// # Errors
    ///
    /// Rejects unsupported directions, oversized fields, malformed or aliased
    /// identities, invalid proofs, derived-address or LEZ-authority drift,
    /// invalid messages/windows, and invalid purpose-separated adaptor contexts.
    pub fn validate(body: XmrAgreementBodyV1) -> Result<Self, XmrAgreementV1Error> {
        validate_wire_field_bounds(&body)?;
        validate_cheap_body_invariants(&body)?;
        let (maker_agreement_key, taker_agreement_key) =
            validate_participant_keys(&body.participants)?;
        let commitment = body.commitment();
        let maker_proof =
            CrossCurveDleqProofV1::from_wire_bytes(&body.monero.maker_dleq_proof_wire)?;
        let taker_proof =
            CrossCurveDleqProofV1::from_wire_bytes(&body.monero.taker_dleq_proof_wire)?;
        if maker_proof.ed25519_public_key() == taker_proof.ed25519_public_key()
            || maker_proof.secp256k1_public_key() == taker_proof.secp256k1_public_key()
        {
            return Err(XmrAgreementV1Error::DuplicateSpendShares);
        }
        if body.lez.maker_dleq_transcript_commitment != maker_proof.transcript_commitment()
            || body.lez.taker_dleq_transcript_commitment != taker_proof.transcript_commitment()
        {
            return Err(XmrAgreementV1Error::LezDleqCommitmentMismatch);
        }
        reject_dleq_signing_key_reuse(
            &body.participants,
            [
                maker_proof.secp256k1_public_key(),
                taker_proof.secp256k1_public_key(),
            ],
        )?;
        let shared_address = MoneroSharedAddressV1::derive_from_public_view_key(
            body.monero.network,
            &maker_proof,
            &taker_proof,
            body.monero.public_view_key,
        )?;
        if shared_address.public_view_key() != body.monero.public_view_key
            || shared_address.public_spend_key() != body.monero.public_spend_key
            || shared_address.address_string() != body.monero.address
        {
            return Err(XmrAgreementV1Error::MoneroAddressDerivationMismatch);
        }

        let claim_context = AdaptorSessionContext::untweaked(
            [
                body.participants.maker.claim_session_public_key,
                body.participants.taker.claim_session_public_key,
            ],
            body.messages.claim,
            maker_proof.secp256k1_public_key(),
            session_id(commitment, b"claim"),
        )?;
        let refund_context = AdaptorSessionContext::untweaked(
            [
                body.participants.maker.refund_session_public_key,
                body.participants.taker.refund_session_public_key,
            ],
            body.messages.refund,
            taker_proof.secp256k1_public_key(),
            session_id(commitment, b"refund"),
        )?;
        if claim_context.durable_context_binding() == refund_context.durable_context_binding() {
            return Err(XmrAgreementV1Error::AdaptorContextsNotDistinct);
        }
        validate_lez_authorities(&body.lez, &claim_context, &refund_context)?;

        Ok(Self {
            body,
            commitment,
            maker_agreement_key,
            taker_agreement_key,
            maker_proof,
            taker_proof,
            shared_address,
            claim_context,
            refund_context,
        })
    }

    /// Parses and semantically validates the canonical unsigned Stage-A wire.
    ///
    /// The accepted bytes are exactly the canonical signed agreement prefix:
    /// schema, existing agreement body, and its domain-separated commitment.
    ///
    /// # Errors
    ///
    /// Rejects oversized, malformed, trailing, noncanonical, unsupported, or
    /// semantically invalid bytes and a commitment that differs from the body.
    pub fn from_unsigned_wire(bytes: &[u8]) -> Result<Self, XmrAgreementV1Error> {
        if bytes.len() > MAX_XMR_UNSIGNED_STAGE_A_WIRE_BYTES {
            return Err(XmrAgreementV1Error::OversizedWire);
        }
        let (schema_version, body, agreement_commitment) = decode_unsigned_agreement_wire(bytes)?;
        if schema_version != XMR_AGREEMENT_SCHEMA_V1 {
            return Err(XmrAgreementV1Error::UnsupportedSchema(schema_version));
        }
        let validated = Self::validate(body)?;
        if agreement_commitment != validated.commitment {
            return Err(XmrAgreementV1Error::CommitmentMismatch);
        }
        if validated.encode_unsigned_wire()?.as_slice() != bytes {
            return Err(XmrAgreementV1Error::NonCanonicalWire);
        }
        Ok(validated)
    }

    /// Encodes the canonical signed Stage-A prefix without role signatures.
    ///
    /// # Errors
    ///
    /// Returns an error if retained variable fields or the complete unsigned
    /// prefix exceed their fixed network bounds.
    pub fn encode_unsigned_wire(&self) -> Result<Vec<u8>, XmrAgreementV1Error> {
        encode_unsigned_agreement_wire(XMR_AGREEMENT_SCHEMA_V1, &self.body, self.commitment)
    }

    /// Exact semantically validated body that both roles must inspect and sign.
    #[must_use]
    pub const fn body(&self) -> &XmrAgreementBodyV1 {
        &self.body
    }

    /// Domain-separated commitment that both agreement-role keys must sign.
    #[must_use]
    pub const fn commitment(&self) -> [u8; 32] {
        self.commitment
    }

    /// Attaches role-indexed signatures and returns the existing validated Stage A.
    ///
    /// # Errors
    ///
    /// Rejects malformed, wrong, or crossed Maker/Taker signatures.
    pub fn attach_signatures(
        self,
        maker_signature: [u8; 64],
        taker_signature: [u8; 64],
    ) -> Result<XmrAgreementV1, XmrAgreementV1Error> {
        verify_role_signature(
            XmrRoleV1::Maker,
            self.maker_agreement_key,
            maker_signature,
            self.commitment,
        )?;
        verify_role_signature(
            XmrRoleV1::Taker,
            self.taker_agreement_key,
            taker_signature,
            self.commitment,
        )?;
        Ok(XmrAgreementV1 {
            record: XmrAgreementRecordV1 {
                schema_version: XMR_AGREEMENT_SCHEMA_V1,
                body: self.body,
                agreement_commitment: self.commitment,
                maker_signature,
                taker_signature,
            },
            maker_proof: self.maker_proof,
            taker_proof: self.taker_proof,
            shared_address: self.shared_address,
            claim_context: self.claim_context,
            refund_context: self.refund_context,
        })
    }
}

/// Fully validated agreement with exact claim and signed-refund sessions.
pub struct XmrAgreementV1 {
    record: XmrAgreementRecordV1,
    maker_proof: CrossCurveDleqProofV1,
    taker_proof: CrossCurveDleqProofV1,
    shared_address: MoneroSharedAddressV1,
    claim_context: AdaptorSessionContext,
    refund_context: AdaptorSessionContext,
}

impl std::fmt::Debug for XmrAgreementV1 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("XmrAgreementV1")
            .field("swap_id", &self.record.body.swap_id)
            .field("direction", &self.record.body.direction)
            .field("agreement_commitment", &self.record.agreement_commitment)
            .finish_non_exhaustive()
    }
}

impl XmrAgreementV1 {
    /// Revalidates every primitive and derived field in an untrusted record.
    ///
    /// # Errors
    ///
    /// Rejects unsupported schemas/directions, malformed or aliased identities,
    /// wrong role mappings, invalid proofs, derived address or LEZ authority
    /// drift, invalid windows, changed commitments, either invalid BIP340
    /// signature, or invalid purpose-separated adaptor contexts.
    pub fn validate(record: XmrAgreementRecordV1) -> Result<Self, XmrAgreementV1Error> {
        let XmrAgreementRecordV1 {
            schema_version,
            body,
            agreement_commitment,
            maker_signature,
            taker_signature,
        } = record;
        if schema_version != XMR_AGREEMENT_SCHEMA_V1 {
            return Err(XmrAgreementV1Error::UnsupportedSchema(schema_version));
        }
        let validated = ValidatedXmrAgreementBodyV1::validate(body)?;
        if agreement_commitment != validated.commitment() {
            return Err(XmrAgreementV1Error::CommitmentMismatch);
        }
        validated.attach_signatures(maker_signature, taker_signature)
    }

    /// Parses, validates, and canonically re-encodes the only accepted wire.
    ///
    /// # Errors
    ///
    /// Rejects oversized, truncated, trailing, noncanonical records and every
    /// semantic validation failure from agreement validation.
    pub fn from_wire(bytes: &[u8]) -> Result<Self, XmrAgreementV1Error> {
        if bytes.len() > MAX_XMR_AGREEMENT_WIRE_BYTES {
            return Err(XmrAgreementV1Error::OversizedWire);
        }
        let record = decode_record(bytes)?;
        let agreement = Self::validate(record)?;
        if agreement.encode_wire()?.as_slice() != bytes {
            return Err(XmrAgreementV1Error::NonCanonicalWire);
        }
        Ok(agreement)
    }

    /// Canonical validated wire bytes.
    ///
    /// # Errors
    ///
    /// Returns an error if retained primitive fields exceed fixed bounds.
    pub fn encode_wire(&self) -> Result<Vec<u8>, XmrAgreementV1Error> {
        self.record.encode_wire()
    }
    /// Exact countersigned body.
    #[must_use]
    pub const fn body(&self) -> &XmrAgreementBodyV1 {
        &self.record.body
    }
    /// Exact agreement commitment.
    #[must_use]
    pub const fn agreement_commitment(&self) -> [u8; 32] {
        self.record.agreement_commitment
    }
    /// Verified Maker-owned share proof used by successful claim.
    #[must_use]
    pub const fn maker_proof(&self) -> &CrossCurveDleqProofV1 {
        &self.maker_proof
    }
    /// Verified Taker-owned share proof used by signed refund.
    #[must_use]
    pub const fn taker_proof(&self) -> &CrossCurveDleqProofV1 {
        &self.taker_proof
    }
    /// Re-derived exact shared Monero address.
    #[must_use]
    pub const fn shared_address(&self) -> &MoneroSharedAddressV1 {
        &self.shared_address
    }
    /// Public durable binding for the claim session. The raw signing context is
    /// deliberately not exposed by the pair SDK.
    #[must_use]
    pub fn claim_context_binding(&self) -> [u8; 32] {
        self.claim_context.durable_context_binding()
    }
    /// Public durable binding for the signed-refund session.
    #[must_use]
    pub fn refund_context_binding(&self) -> [u8; 32] {
        self.refund_context.durable_context_binding()
    }
    /// Immutable public descriptor for the exact validated claim session.
    #[must_use = "the validated claim descriptor must be used or explicitly discarded"]
    pub fn claim_session_descriptor(&self) -> XmrAdaptorSessionDescriptorV1 {
        XmrAdaptorSessionDescriptorV1::from_validated_context(
            XmrAdaptorSessionPurposeV1::Claim,
            self.record.agreement_commitment,
            &self.claim_context,
        )
    }

    /// Immutable public descriptor for the exact validated refund session.
    #[must_use = "the validated refund descriptor must be used or explicitly discarded"]
    pub fn refund_session_descriptor(&self) -> XmrAdaptorSessionDescriptorV1 {
        XmrAdaptorSessionDescriptorV1::from_validated_context(
            XmrAdaptorSessionPurposeV1::Refund,
            self.record.agreement_commitment,
            &self.refund_context,
        )
    }

    /// Derives the exact guest-stored context binding for a future on-chain
    /// Taker claim-partial publication.
    ///
    /// # Errors
    ///
    /// Rejects changed nonce openings or an invalid Maker claim partial.
    pub fn claim_partial_context_binding(
        &self,
        transcript: &XmrSessionTranscriptV1,
        maker_partial: [u8; 32],
    ) -> Result<[u8; 32], XmrAgreementV1Error> {
        validate_session_transcript(&self.claim_context, transcript)?;
        verify_adaptor_partial_signature(
            &self.claim_context,
            SigningRole::Maker,
            transcript.maker_public_nonce,
            transcript.taker_public_nonce,
            maker_partial,
        )?;
        Ok(claim_partial_context_binding(
            self.record.agreement_commitment,
            self.claim_context.durable_context_binding(),
            transcript,
            maker_partial,
        ))
    }

    /// Commits the owner-local Taker claim partial after verifying both partials.
    ///
    /// This proves consistency at publication, not pre-funding validity to the
    /// Maker. Production requires reviewed verifiable-encryption/ZK evidence or
    /// explicit acceptance of the residual grief/penalty model.
    ///
    /// # Errors
    ///
    /// Rejects changed nonce openings or either invalid claim partial.
    pub fn commit_taker_claim_partial(
        &self,
        transcript: &XmrSessionTranscriptV1,
        maker_partial: [u8; 32],
        taker_partial: [u8; 32],
    ) -> Result<[u8; 32], XmrAgreementV1Error> {
        let context_binding = self.claim_partial_context_binding(transcript, maker_partial)?;
        verify_adaptor_partial_signature(
            &self.claim_context,
            SigningRole::Taker,
            transcript.maker_public_nonce,
            transcript.taker_public_nonce,
            taker_partial,
        )?;
        Ok(claim_partial_commitment(context_binding, taker_partial))
    }
}

/// Complete nonce commitment/opening transcript for one adaptor session.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct XmrSessionTranscriptV1 {
    maker_nonce_commitment: [u8; 32],
    taker_nonce_commitment: [u8; 32],
    maker_public_nonce: [u8; 66],
    taker_public_nonce: [u8; 66],
}

impl XmrSessionTranscriptV1 {
    /// Creates untrusted fixed-width transcript fields.
    #[must_use]
    pub const fn new(
        maker_nonce_commitment: [u8; 32],
        taker_nonce_commitment: [u8; 32],
        maker_public_nonce: [u8; 66],
        taker_public_nonce: [u8; 66],
    ) -> Self {
        Self {
            maker_nonce_commitment,
            taker_nonce_commitment,
            maker_public_nonce,
            taker_public_nonce,
        }
    }
    /// Maker nonce commitment persisted before either nonce opening.
    #[must_use]
    pub const fn maker_nonce_commitment(&self) -> [u8; 32] {
        self.maker_nonce_commitment
    }
    /// Taker nonce commitment persisted before either nonce opening.
    #[must_use]
    pub const fn taker_nonce_commitment(&self) -> [u8; 32] {
        self.taker_nonce_commitment
    }
    /// Maker public nonce opening for this exact session.
    #[must_use]
    pub const fn maker_public_nonce(&self) -> [u8; 66] {
        self.maker_public_nonce
    }
    /// Taker public nonce opening for this exact session.
    #[must_use]
    pub const fn taker_public_nonce(&self) -> [u8; 66] {
        self.taker_public_nonce
    }

    fn encode_into(&self, bytes: &mut Vec<u8>) {
        bytes.extend_from_slice(&self.maker_nonce_commitment);
        bytes.extend_from_slice(&self.taker_nonce_commitment);
        bytes.extend_from_slice(&self.maker_public_nonce);
        bytes.extend_from_slice(&self.taker_public_nonce);
    }
}

/// Canonical activation body countersigned after both nonce rounds.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct XmrActivationBodyV1 {
    base_agreement_commitment: [u8; 32],
    claim_context_binding: [u8; 32],
    claim_transcript: XmrSessionTranscriptV1,
    maker_claim_partial: [u8; 32],
    claim_partial_context_binding: [u8; 32],
    claim_partial_commitment: [u8; 32],
    refund_context_binding: [u8; 32],
    refund_transcript: XmrSessionTranscriptV1,
    maker_refund_partial: [u8; 32],
    taker_refund_partial: [u8; 32],
    refund_presignature: [u8; 65],
}

impl XmrActivationBodyV1 {
    /// Creates untrusted activation fields.
    #[allow(clippy::too_many_arguments)]
    #[must_use]
    pub const fn new(
        base_agreement_commitment: [u8; 32],
        claim_context_binding: [u8; 32],
        claim_transcript: XmrSessionTranscriptV1,
        maker_claim_partial: [u8; 32],
        claim_partial_context_binding: [u8; 32],
        claim_partial_commitment: [u8; 32],
        refund_context_binding: [u8; 32],
        refund_transcript: XmrSessionTranscriptV1,
        maker_refund_partial: [u8; 32],
        taker_refund_partial: [u8; 32],
        refund_presignature: [u8; 65],
    ) -> Self {
        Self {
            base_agreement_commitment,
            claim_context_binding,
            claim_transcript,
            maker_claim_partial,
            claim_partial_context_binding,
            claim_partial_commitment,
            refund_context_binding,
            refund_transcript,
            maker_refund_partial,
            taker_refund_partial,
            refund_presignature,
        }
    }

    /// Stage-A commitment to which this activation is bound.
    #[must_use]
    pub const fn base_agreement_commitment(&self) -> [u8; 32] {
        self.base_agreement_commitment
    }
    /// Durable claim-session context binding.
    #[must_use]
    pub const fn claim_context_binding(&self) -> [u8; 32] {
        self.claim_context_binding
    }
    /// Complete claim-session nonce commitment/opening transcript.
    #[must_use]
    pub const fn claim_transcript(&self) -> &XmrSessionTranscriptV1 {
        &self.claim_transcript
    }
    /// Verified Maker claim partial committed before the first chain lock.
    #[must_use]
    pub const fn maker_claim_partial(&self) -> [u8; 32] {
        self.maker_claim_partial
    }
    /// Context-bound commitment input for the private Taker claim partial.
    #[must_use]
    pub const fn claim_partial_context_binding(&self) -> [u8; 32] {
        self.claim_partial_context_binding
    }
    /// Commitment to the owner-private Taker claim partial.
    #[must_use]
    pub const fn claim_partial_commitment(&self) -> [u8; 32] {
        self.claim_partial_commitment
    }
    /// Durable refund-session context binding.
    #[must_use]
    pub const fn refund_context_binding(&self) -> [u8; 32] {
        self.refund_context_binding
    }
    /// Complete refund-session nonce commitment/opening transcript.
    #[must_use]
    pub const fn refund_transcript(&self) -> &XmrSessionTranscriptV1 {
        &self.refund_transcript
    }
    /// Verified Maker refund partial included in the presignature.
    #[must_use]
    pub const fn maker_refund_partial(&self) -> [u8; 32] {
        self.maker_refund_partial
    }
    /// Verified Taker refund partial included in the presignature.
    #[must_use]
    pub const fn taker_refund_partial(&self) -> [u8; 32] {
        self.taker_refund_partial
    }
    /// Aggregated signed-refund presignature available before the first lock.
    #[must_use]
    pub const fn refund_presignature(&self) -> [u8; 65] {
        self.refund_presignature
    }

    /// Domain-separated canonical activation commitment.
    #[must_use]
    pub fn commitment(&self) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(ACTIVATION_DOMAIN);
        hasher.update(self.encode_body());
        hasher.finalize().into()
    }

    fn encode_body(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(700);
        bytes.extend_from_slice(&self.base_agreement_commitment);
        bytes.extend_from_slice(&self.claim_context_binding);
        self.claim_transcript.encode_into(&mut bytes);
        bytes.extend_from_slice(&self.maker_claim_partial);
        bytes.extend_from_slice(&self.claim_partial_context_binding);
        bytes.extend_from_slice(&self.claim_partial_commitment);
        bytes.extend_from_slice(&self.refund_context_binding);
        self.refund_transcript.encode_into(&mut bytes);
        bytes.extend_from_slice(&self.maker_refund_partial);
        bytes.extend_from_slice(&self.taker_refund_partial);
        bytes.extend_from_slice(&self.refund_presignature);
        bytes
    }
}

/// Primitive activation record with both agreement-role signatures.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct XmrActivationRecordV1 {
    schema_version: u16,
    body: XmrActivationBodyV1,
    activation_commitment: [u8; 32],
    maker_signature: [u8; 64],
    taker_signature: [u8; 64],
}

impl XmrActivationRecordV1 {
    /// Assembles untrusted activation fields.
    #[must_use]
    pub const fn from_parts(
        schema_version: u16,
        body: XmrActivationBodyV1,
        activation_commitment: [u8; 32],
        maker_signature: [u8; 64],
        taker_signature: [u8; 64],
    ) -> Self {
        Self {
            schema_version,
            body,
            activation_commitment,
            maker_signature,
            taker_signature,
        }
    }

    /// Exact activation body purportedly signed by both actors.
    #[must_use]
    pub const fn body(&self) -> &XmrActivationBodyV1 {
        &self.body
    }

    /// Encodes the canonical fixed-width activation wire.
    ///
    /// # Errors
    ///
    /// Rejects an activation record larger than the fixed wire bound.
    pub fn encode_wire(&self) -> Result<Vec<u8>, XmrAgreementV1Error> {
        let mut bytes = encode_unsigned_activation_wire(
            self.schema_version,
            &self.body,
            self.activation_commitment,
        )?;
        bytes.extend_from_slice(&self.maker_signature);
        bytes.extend_from_slice(&self.taker_signature);
        Ok(bytes)
    }
}

/// Semantically validated unsigned Stage-B body ready for role countersigning.
///
/// The capability can only be created against an already validated Stage A and
/// a local private view key that opens the agreement's shared Monero address.
/// Its private fields prevent an untrusted activation body from reaching the
/// signature-attachment path without transcript, partial, and presignature checks.
#[must_use = "a validated Stage-B body must be countersigned or explicitly discarded"]
pub struct ValidatedXmrActivationBodyV1 {
    body: XmrActivationBodyV1,
    commitment: [u8; 32],
    maker_agreement_key: PublicKey,
    taker_agreement_key: PublicKey,
}

impl std::fmt::Debug for ValidatedXmrActivationBodyV1 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ValidatedXmrActivationBodyV1")
            .field(
                "base_agreement_commitment",
                &self.body.base_agreement_commitment,
            )
            .field("commitment", &self.commitment)
            .finish_non_exhaustive()
    }
}

impl ValidatedXmrActivationBodyV1 {
    /// Validates every unsigned Stage-B field before either role signs it.
    ///
    /// # Errors
    ///
    /// Rejects a wrong local view key, a different Stage-A base, crossed session
    /// bindings, invalid nonce transcripts or partials, and a refund
    /// presignature that does not aggregate from the validated refund session.
    pub fn validate(
        agreement: &XmrAgreementV1,
        body: XmrActivationBodyV1,
        local_view_key: &MoneroPrivateViewKey,
    ) -> Result<Self, XmrAgreementV1Error> {
        if local_view_key.public_key() != agreement.body().monero.public_view_key {
            return Err(XmrAgreementV1Error::LocalViewKeyMismatch);
        }
        if body.base_agreement_commitment != agreement.agreement_commitment()
            || body.claim_context_binding != agreement.claim_context.durable_context_binding()
            || body.refund_context_binding != agreement.refund_context.durable_context_binding()
            || body.claim_partial_commitment == [0; 32]
        {
            return Err(XmrAgreementV1Error::ActivationBindingMismatch);
        }
        validate_session_transcript(&agreement.claim_context, &body.claim_transcript)?;
        verify_adaptor_partial_signature(
            &agreement.claim_context,
            SigningRole::Maker,
            body.claim_transcript.maker_public_nonce,
            body.claim_transcript.taker_public_nonce,
            body.maker_claim_partial,
        )?;
        let expected_partial_context = claim_partial_context_binding(
            agreement.agreement_commitment(),
            agreement.claim_context.durable_context_binding(),
            &body.claim_transcript,
            body.maker_claim_partial,
        );
        if body.claim_partial_context_binding != expected_partial_context {
            return Err(XmrAgreementV1Error::ActivationBindingMismatch);
        }

        validate_session_transcript(&agreement.refund_context, &body.refund_transcript)?;
        verify_adaptor_partial_signature(
            &agreement.refund_context,
            SigningRole::Maker,
            body.refund_transcript.maker_public_nonce,
            body.refund_transcript.taker_public_nonce,
            body.maker_refund_partial,
        )?;
        verify_adaptor_partial_signature(
            &agreement.refund_context,
            SigningRole::Taker,
            body.refund_transcript.maker_public_nonce,
            body.refund_transcript.taker_public_nonce,
            body.taker_refund_partial,
        )?;
        let refund_presignature = aggregate_adaptor_presignature(
            &agreement.refund_context,
            body.refund_transcript.maker_public_nonce,
            body.refund_transcript.taker_public_nonce,
            body.maker_refund_partial,
            body.taker_refund_partial,
        )?;
        if refund_presignature != body.refund_presignature {
            return Err(XmrAgreementV1Error::RefundPresignatureMismatch);
        }

        let participants = &agreement.record.body.participants;
        let maker_agreement_key = parse_key(
            participants.maker.agreement_public_key,
            XmrRoleV1::Maker,
            "agreement",
        )?;
        let taker_agreement_key = parse_key(
            participants.taker.agreement_public_key,
            XmrRoleV1::Taker,
            "agreement",
        )?;
        let commitment = body.commitment();
        Ok(Self {
            body,
            commitment,
            maker_agreement_key,
            taker_agreement_key,
        })
    }

    /// Parses and validates a canonical unsigned Stage-B wire against Stage A.
    ///
    /// The accepted bytes are exactly the signed activation prefix: schema,
    /// existing activation body, and its domain-separated commitment.
    ///
    /// # Errors
    ///
    /// Rejects oversized, malformed, trailing, noncanonical, unsupported, or
    /// semantically invalid bytes, the wrong local view key, and commitment
    /// drift from the decoded body.
    pub fn from_unsigned_wire(
        agreement: &XmrAgreementV1,
        bytes: &[u8],
        local_view_key: &MoneroPrivateViewKey,
    ) -> Result<Self, XmrAgreementV1Error> {
        if bytes.len() > MAX_XMR_UNSIGNED_STAGE_B_WIRE_BYTES {
            return Err(XmrAgreementV1Error::OversizedActivationWire);
        }
        let (schema_version, body, activation_commitment) = decode_unsigned_activation_wire(bytes)?;
        if schema_version != XMR_ACTIVATION_SCHEMA_V1 {
            return Err(XmrAgreementV1Error::UnsupportedActivationSchema(
                schema_version,
            ));
        }
        let validated = Self::validate(agreement, body, local_view_key)?;
        if activation_commitment != validated.commitment {
            return Err(XmrAgreementV1Error::ActivationCommitmentMismatch);
        }
        if validated.encode_unsigned_wire()?.as_slice() != bytes {
            return Err(XmrAgreementV1Error::NonCanonicalActivationWire);
        }
        Ok(validated)
    }

    /// Encodes the canonical signed Stage-B prefix without role signatures.
    ///
    /// # Errors
    ///
    /// Returns an error if the complete unsigned prefix exceeds its fixed
    /// network bound.
    pub fn encode_unsigned_wire(&self) -> Result<Vec<u8>, XmrAgreementV1Error> {
        encode_unsigned_activation_wire(XMR_ACTIVATION_SCHEMA_V1, &self.body, self.commitment)
    }

    /// Exact semantically validated activation body both roles must sign.
    #[must_use]
    pub const fn body(&self) -> &XmrActivationBodyV1 {
        &self.body
    }

    /// Domain-separated commitment that both agreement-role keys must sign.
    #[must_use]
    pub const fn commitment(&self) -> [u8; 32] {
        self.commitment
    }

    /// Attaches role-indexed signatures and returns the existing validated Stage B.
    ///
    /// # Errors
    ///
    /// Rejects malformed, wrong, or crossed Maker/Taker signatures.
    pub fn attach_signatures(
        self,
        maker_signature: [u8; 64],
        taker_signature: [u8; 64],
    ) -> Result<XmrActivatedAgreementV1, XmrAgreementV1Error> {
        verify_role_signature(
            XmrRoleV1::Maker,
            self.maker_agreement_key,
            maker_signature,
            self.commitment,
        )?;
        verify_role_signature(
            XmrRoleV1::Taker,
            self.taker_agreement_key,
            taker_signature,
            self.commitment,
        )?;
        Ok(XmrActivatedAgreementV1 {
            record: XmrActivationRecordV1 {
                schema_version: XMR_ACTIVATION_SCHEMA_V1,
                body: self.body,
                activation_commitment: self.commitment,
                maker_signature,
                taker_signature,
            },
        })
    }
}

/// Fully validated Stage-B activation. Stage A exposes no LEZ init plan.
pub struct XmrActivatedAgreementV1 {
    record: XmrActivationRecordV1,
}

impl std::fmt::Debug for XmrActivatedAgreementV1 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("XmrActivatedAgreementV1")
            .field(
                "base_agreement_commitment",
                &self.record.body.base_agreement_commitment,
            )
            .field("activation_commitment", &self.record.activation_commitment)
            .finish_non_exhaustive()
    }
}

impl XmrActivatedAgreementV1 {
    /// Validates Stage B against Stage A and the caller's local private view key.
    ///
    /// # Errors
    ///
    /// Rejects any schema, base, view-key, transcript, partial, presignature,
    /// commitment, or role-signature mismatch.
    pub fn validate(
        agreement: &XmrAgreementV1,
        record: XmrActivationRecordV1,
        local_view_key: &MoneroPrivateViewKey,
    ) -> Result<Self, XmrAgreementV1Error> {
        let XmrActivationRecordV1 {
            schema_version,
            body,
            activation_commitment,
            maker_signature,
            taker_signature,
        } = record;
        if schema_version != XMR_ACTIVATION_SCHEMA_V1 {
            return Err(XmrAgreementV1Error::UnsupportedActivationSchema(
                schema_version,
            ));
        }
        let validated = ValidatedXmrActivationBodyV1::validate(agreement, body, local_view_key)?;
        if activation_commitment != validated.commitment() {
            return Err(XmrAgreementV1Error::ActivationCommitmentMismatch);
        }
        validated.attach_signatures(maker_signature, taker_signature)
    }

    /// Parses and validates the canonical fixed-width activation wire.
    ///
    /// # Errors
    ///
    /// Rejects oversized, malformed, noncanonical, or semantically invalid
    /// activation records.
    pub fn from_wire(
        agreement: &XmrAgreementV1,
        bytes: &[u8],
        local_view_key: &MoneroPrivateViewKey,
    ) -> Result<Self, XmrAgreementV1Error> {
        if bytes.len() > MAX_XMR_ACTIVATION_WIRE_BYTES {
            return Err(XmrAgreementV1Error::OversizedActivationWire);
        }
        let record = decode_activation_record(bytes)?;
        let activation = Self::validate(agreement, record, local_view_key)?;
        if activation.encode_wire()?.as_slice() != bytes {
            return Err(XmrAgreementV1Error::NonCanonicalActivationWire);
        }
        Ok(activation)
    }

    /// Canonical activation wire.
    ///
    /// # Errors
    ///
    /// Rejects an activation record larger than the fixed wire bound.
    pub fn encode_wire(&self) -> Result<Vec<u8>, XmrAgreementV1Error> {
        self.record.encode_wire()
    }

    /// Exact activation commitment stored as the guest terms hash.
    #[must_use]
    pub const fn activation_commitment(&self) -> [u8; 32] {
        self.record.activation_commitment
    }

    /// Exact validated activation body countersigned by both roles.
    #[must_use]
    pub const fn body(&self) -> &XmrActivationBodyV1 {
        &self.record.body
    }

    /// Derives the exact pair-neutral application coordinator from countersigned Stage B.
    ///
    /// Stage A deliberately exposes no equivalent method: no executable lifecycle exists until
    /// both roles countersign the claim/refund transcripts in this activation. The LEZ refund
    /// timestamp is rounded up so millisecond precision can never make recovery available early.
    ///
    /// # Errors
    ///
    /// Rejects an activation crossed with another Stage A or an invalid application projection.
    pub fn initial_coordinator(
        &self,
        agreement: &XmrAgreementV1,
    ) -> Result<SwapCoordinator, XmrAgreementV1Error> {
        self.require_base(agreement)?;
        let body = agreement.body();
        let taker_confirmations = ConfirmationPolicy::new(body.lez().required_finality_units())
            .map_err(|_| XmrAgreementV1Error::InvalidInitialCoordinator)?;
        let maker_confirmations = ConfirmationPolicy::new(body.monero().required_confirmations())
            .map_err(|_| XmrAgreementV1Error::InvalidInitialCoordinator)?;
        let refund_at = LezUnixMilliseconds::new(body.windows().refund_at_ms())
            .to_unix_seconds_ceil()
            .value();
        let recovery = RecoverySchedule::xmr_lez_first(
            ChainPosition::timestamp(Chain::Lez, refund_at),
            body.lez().required_finality_units(),
        )
        .map_err(|_| XmrAgreementV1Error::InvalidInitialCoordinator)?;
        let swap_id = SwapId::new(hex::encode(body.swap_id()))
            .map_err(|_| XmrAgreementV1Error::InvalidInitialCoordinator)?;
        Ok(SwapCoordinator::new_with_confirmation_policies(
            swap_id,
            Pair::Monero,
            SwapDirection::TakerSellsLez,
            taker_confirmations,
            maker_confirmations,
            recovery,
        ))
    }

    /// Verifies the Taker partial later retrieved from the LEZ publication.
    ///
    /// This is verification only. The SDK intentionally exposes no method that
    /// turns a caller-constructed Monero status object into publication authority.
    ///
    /// # Errors
    ///
    /// Rejects a different Stage-A base, invalid Taker partial, or mismatch with
    /// the exact guest-stored context-bound commitment.
    pub fn verify_published_taker_claim_partial(
        &self,
        agreement: &XmrAgreementV1,
        taker_partial: [u8; 32],
    ) -> Result<(), XmrAgreementV1Error> {
        self.require_base(agreement)?;
        let body = &self.record.body;
        verify_adaptor_partial_signature(
            &agreement.claim_context,
            SigningRole::Taker,
            body.claim_transcript.maker_public_nonce,
            body.claim_transcript.taker_public_nonce,
            taker_partial,
        )?;
        let actual = claim_partial_commitment(body.claim_partial_context_binding, taker_partial);
        if actual != body.claim_partial_commitment {
            return Err(XmrAgreementV1Error::PublishedClaimPartialMismatch);
        }
        Ok(())
    }

    /// Exact version-3 native-XMR guest initialization derived only from Stage B.
    ///
    /// # Errors
    ///
    /// Rejects use with a Stage-A agreement other than the activated base.
    pub fn lez_initialize_plan(
        &self,
        agreement: &XmrAgreementV1,
    ) -> Result<XmrLezInitializePlanV1, XmrAgreementV1Error> {
        self.require_base(agreement)?;
        Ok(XmrLezInitializePlanV1::derive(agreement, self))
    }

    /// Structurally validates a caller-supplied version-3 LEZ lock candidate.
    ///
    /// This checkpoint proves only that the supplied fields agree with Stage B,
    /// satisfy the named finality depth, and remain at or before the exact Maker
    /// funding cutoff. It does not authenticate an RPC response, prove canonical
    /// chain inclusion, or authorize Monero funding. A trusted finalized-chain
    /// adapter must promote this candidate into lifecycle evidence; that boundary
    /// remains pending.
    ///
    /// # Errors
    ///
    /// Rejects a different Stage-A base, any metadata/custody field mismatch,
    /// empty candidate identifiers, non-funded status, insufficient reported
    /// finality, or a finalized timestamp after the funding cutoff.
    pub fn validate_lez_lock_candidate(
        &self,
        agreement: &XmrAgreementV1,
        candidate: &XmrLezLockCandidateV1,
    ) -> Result<XmrValidatedLezLockCandidateV1, XmrAgreementV1Error> {
        let expected = self.lez_initialize_plan(agreement)?;
        if !candidate.matches_plan(&expected)
            || candidate.metadata_version != XMR_METADATA_VERSION_V3
            || candidate.status != XmrLezLockStatusV1::Funded
            || candidate.finalized_consensus_timestamp_ms == 0
            || candidate.funding_transaction_id == [0; 32]
            || candidate.containing_block_hash == [0; 32]
        {
            return Err(XmrAgreementV1Error::LezLockCandidateMismatch);
        }
        let required = agreement.body().lez.required_finality_units;
        if candidate.finality_units < required {
            return Err(XmrAgreementV1Error::InsufficientLezCandidateFinality {
                actual: candidate.finality_units,
                required,
            });
        }
        let cutoff_ms = expected.maker_xmr_funding_cutoff_ms();
        if candidate.finalized_consensus_timestamp_ms > cutoff_ms {
            return Err(XmrAgreementV1Error::LezLockCandidateAfterFundingCutoff {
                finalized_consensus_timestamp_ms: candidate.finalized_consensus_timestamp_ms,
                cutoff_ms,
            });
        }
        Ok(XmrValidatedLezLockCandidateV1 {
            activation_commitment: self.record.activation_commitment,
            funding_transaction_id: candidate.funding_transaction_id,
            containing_block_hash: candidate.containing_block_hash,
            finalized_consensus_timestamp_ms: candidate.finalized_consensus_timestamp_ms,
        })
    }

    fn require_base(&self, agreement: &XmrAgreementV1) -> Result<(), XmrAgreementV1Error> {
        if self.record.body.base_agreement_commitment == agreement.agreement_commitment() {
            Ok(())
        } else {
            Err(XmrAgreementV1Error::ActivationBindingMismatch)
        }
    }
}

/// Exact native-LEZ v3 initialization parameters derived from Stage B.
///
/// This type deliberately has no public constructor. Callers cannot create an
/// executable initialization plan from the Stage-A agreement alone.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct XmrLezInitializePlanV1 {
    channel_id: [u8; 32],
    genesis_hash: [u8; 32],
    escrow_program_id: [u32; 8],
    authenticated_transfer_program_id: [u32; 8],
    metadata_version: u8,
    metadata_account: [u8; 32],
    custody_account: [u8; 32],
    depositor_account: [u8; 32],
    claimant_account: [u8; 32],
    claim_aggregate_x_only_key: [u8; 32],
    claim_authority_account: [u8; 32],
    refund_aggregate_x_only_key: [u8; 32],
    refund_authority_account: [u8; 32],
    swap_id: [u8; 32],
    activation_commitment: [u8; 32],
    claim_partial_context_binding: [u8; 32],
    claim_partial_commitment: [u8; 32],
    maker_dleq_transcript_commitment: [u8; 32],
    taker_dleq_transcript_commitment: [u8; 32],
    amount: u128,
    maker_xmr_funding_cutoff_ms: u64,
    refund_at_ms: u64,
    punish_at_ms: u64,
    claim_message_hash: [u8; 32],
    refund_message_hash: [u8; 32],
    punish_message_hash: [u8; 32],
}

impl XmrLezInitializePlanV1 {
    fn derive(agreement: &XmrAgreementV1, activation: &XmrActivatedAgreementV1) -> Self {
        let body = agreement.body();
        let lez = &body.lez;
        let activation_body = &activation.record.body;
        Self {
            channel_id: lez.channel_id,
            genesis_hash: lez.genesis_hash,
            escrow_program_id: lez.escrow_program_id,
            authenticated_transfer_program_id: lez.authenticated_transfer_program_id,
            metadata_version: XMR_METADATA_VERSION_V3,
            metadata_account: lez.metadata_account,
            custody_account: lez.custody_account,
            depositor_account: lez.depositor_account,
            claimant_account: lez.claimant_account,
            claim_aggregate_x_only_key: lez.claim_aggregate_x_only_key,
            claim_authority_account: lez.claim_authority_account,
            refund_aggregate_x_only_key: lez.refund_aggregate_x_only_key,
            refund_authority_account: lez.refund_authority_account,
            swap_id: body.swap_id,
            activation_commitment: activation.record.activation_commitment,
            claim_partial_context_binding: activation_body.claim_partial_context_binding,
            claim_partial_commitment: activation_body.claim_partial_commitment,
            maker_dleq_transcript_commitment: lez.maker_dleq_transcript_commitment,
            taker_dleq_transcript_commitment: lez.taker_dleq_transcript_commitment,
            amount: lez.amount,
            maker_xmr_funding_cutoff_ms: body.windows.maker_xmr_funding_cutoff_ms,
            refund_at_ms: body.windows.refund_at_ms,
            punish_at_ms: body.windows.punish_at_ms,
            claim_message_hash: body.messages.claim,
            refund_message_hash: body.messages.refund,
            punish_message_hash: body.messages.punish,
        }
    }

    /// Exact LEZ channel identifier.
    #[must_use]
    pub const fn channel_id(&self) -> [u8; 32] {
        self.channel_id
    }

    /// Exact LEZ genesis identity.
    #[must_use]
    pub const fn genesis_hash(&self) -> [u8; 32] {
        self.genesis_hash
    }

    /// Exact native-XMR escrow program ID.
    #[must_use]
    pub const fn escrow_program_id(&self) -> [u32; 8] {
        self.escrow_program_id
    }

    /// Exact authenticated-transfer program ID used for native custody.
    #[must_use]
    pub const fn authenticated_transfer_program_id(&self) -> [u32; 8] {
        self.authenticated_transfer_program_id
    }

    /// Exact native-XMR guest metadata schema.
    #[must_use]
    pub const fn metadata_version(&self) -> u8 {
        self.metadata_version
    }

    /// Exact metadata account.
    #[must_use]
    pub const fn metadata_account(&self) -> [u8; 32] {
        self.metadata_account
    }

    /// Exact native custody account.
    #[must_use]
    pub const fn custody_account(&self) -> [u8; 32] {
        self.custody_account
    }

    /// Taker owner account that deposits LEZ.
    #[must_use]
    pub const fn depositor_account(&self) -> [u8; 32] {
        self.depositor_account
    }

    /// Maker owner account that receives the successful claim.
    #[must_use]
    pub const fn claimant_account(&self) -> [u8; 32] {
        self.claimant_account
    }

    /// Exact claim aggregate x-only key.
    #[must_use]
    pub const fn claim_aggregate_x_only_key(&self) -> [u8; 32] {
        self.claim_aggregate_x_only_key
    }

    /// LEZ claim authority account derived from the aggregate key.
    #[must_use]
    pub const fn claim_authority_account(&self) -> [u8; 32] {
        self.claim_authority_account
    }

    /// Exact signed-refund aggregate x-only key.
    #[must_use]
    pub const fn refund_aggregate_x_only_key(&self) -> [u8; 32] {
        self.refund_aggregate_x_only_key
    }

    /// LEZ refund authority account derived from the aggregate key.
    #[must_use]
    pub const fn refund_authority_account(&self) -> [u8; 32] {
        self.refund_authority_account
    }

    /// Exact swap identifier stored by the guest.
    #[must_use]
    pub const fn swap_id(&self) -> [u8; 32] {
        self.swap_id
    }

    /// Exact Stage-B activation commitment stored by the guest.
    #[must_use]
    pub const fn activation_commitment(&self) -> [u8; 32] {
        self.activation_commitment
    }

    /// Guest-stored binding for the hidden Taker claim partial.
    #[must_use]
    pub const fn claim_partial_context_binding(&self) -> [u8; 32] {
        self.claim_partial_context_binding
    }

    /// Guest-stored commitment to the hidden Taker claim partial.
    #[must_use]
    pub const fn claim_partial_commitment(&self) -> [u8; 32] {
        self.claim_partial_commitment
    }

    /// Maker DLEQ envelope commitment stored by the guest.
    #[must_use]
    pub const fn maker_dleq_transcript_commitment(&self) -> [u8; 32] {
        self.maker_dleq_transcript_commitment
    }

    /// Taker DLEQ envelope commitment stored by the guest.
    #[must_use]
    pub const fn taker_dleq_transcript_commitment(&self) -> [u8; 32] {
        self.taker_dleq_transcript_commitment
    }

    /// Exact native LEZ principal.
    #[must_use]
    pub const fn amount(&self) -> u128 {
        self.amount
    }

    /// Latest finalized LEZ consensus time at which Maker XMR funding remains permitted.
    #[must_use]
    pub const fn maker_xmr_funding_cutoff_ms(&self) -> u64 {
        self.maker_xmr_funding_cutoff_ms
    }

    /// Earliest signed-refund guest time in milliseconds.
    #[must_use]
    pub const fn refund_at_ms(&self) -> u64 {
        self.refund_at_ms
    }

    /// Earliest punishment guest time in milliseconds.
    #[must_use]
    pub const fn punish_at_ms(&self) -> u64 {
        self.punish_at_ms
    }

    /// Exact successful-claim message hash.
    #[must_use]
    pub const fn claim_message_hash(&self) -> [u8; 32] {
        self.claim_message_hash
    }

    /// Exact signed-refund message hash.
    #[must_use]
    pub const fn refund_message_hash(&self) -> [u8; 32] {
        self.refund_message_hash
    }

    /// Exact punishment message hash.
    #[must_use]
    pub const fn punish_message_hash(&self) -> [u8; 32] {
        self.punish_message_hash
    }
}

/// Exact lifecycle state returned by the LEZ v3 metadata adapter.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum XmrLezLockStatusV1 {
    /// Metadata exists but native custody has not been funded.
    Initialized,
    /// Exact native custody funding has finalized.
    Funded,
    /// A terminal branch has already consumed custody.
    Closed,
}

/// Caller-supplied structural candidate for an exact finalized LEZ v3 lock.
///
/// This value is not authenticated chain evidence and is not lifecycle
/// authority. Its public constructor exists for adapter parsing and tests; a
/// caller can fabricate every field.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct XmrLezLockCandidateV1 {
    plan: XmrLezInitializePlanV1,
    metadata_version: u8,
    status: XmrLezLockStatusV1,
    finality_units: u32,
    finalized_consensus_timestamp_ms: u64,
    funding_transaction_id: [u8; 32],
    containing_block_hash: [u8; 32],
}

impl XmrLezLockCandidateV1 {
    /// Creates a wholly untrusted candidate for structural validation.
    ///
    /// This constructor performs no node access, provenance check, or finality
    /// proof and must not be treated as funding authorization.
    #[must_use]
    pub const fn new(
        plan: XmrLezInitializePlanV1,
        metadata_version: u8,
        status: XmrLezLockStatusV1,
        finality_units: u32,
        finalized_consensus_timestamp_ms: u64,
        funding_transaction_id: [u8; 32],
        containing_block_hash: [u8; 32],
    ) -> Self {
        Self {
            plan,
            metadata_version,
            status,
            finality_units,
            finalized_consensus_timestamp_ms,
            funding_transaction_id,
            containing_block_hash,
        }
    }

    fn matches_plan(&self, expected: &XmrLezInitializePlanV1) -> bool {
        self.plan == *expected
    }
}

/// Structurally validated LEZ-lock candidate.
///
/// This remains unauthenticated caller data. It is suitable for adapter
/// diagnostics and comparison only, not as authority to fund Monero.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct XmrValidatedLezLockCandidateV1 {
    activation_commitment: [u8; 32],
    funding_transaction_id: [u8; 32],
    containing_block_hash: [u8; 32],
    finalized_consensus_timestamp_ms: u64,
}

impl XmrValidatedLezLockCandidateV1 {
    /// Stage-B commitment matched by the untrusted candidate.
    #[must_use]
    pub const fn activation_commitment(&self) -> [u8; 32] {
        self.activation_commitment
    }

    /// Candidate LEZ transaction reported as funding native custody.
    #[must_use]
    pub const fn funding_transaction_id(&self) -> [u8; 32] {
        self.funding_transaction_id
    }

    /// Candidate LEZ block reported as containing the funding transaction.
    #[must_use]
    pub const fn containing_block_hash(&self) -> [u8; 32] {
        self.containing_block_hash
    }

    /// Candidate finalized consensus timestamp checked against the cutoff.
    #[must_use]
    pub const fn finalized_consensus_timestamp_ms(&self) -> u64 {
        self.finalized_consensus_timestamp_ms
    }
}

/// Fail-closed agreement, wire, derivation, signature, and lock errors.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum XmrAgreementV1Error {
    /// Agreement schema is unsupported.
    #[error("unsupported XMR agreement schema {0}")]
    UnsupportedSchema(u16),
    /// Only Taker-sells-LEZ is implemented by version 1.
    #[error("unsupported XMR swap direction")]
    UnsupportedDirection,
    /// Complete wire exceeded the fixed network bound.
    #[error("XMR agreement wire exceeds the fixed bound")]
    OversizedWire,
    /// A variable field exceeded its independent fixed bound.
    #[error("XMR agreement field exceeds its fixed bound")]
    OversizedField,
    /// Wire was malformed, truncated, or contained trailing bytes.
    #[error("malformed XMR agreement wire")]
    MalformedWire,
    /// Parsed wire did not exactly match its canonical re-encoding.
    #[error("noncanonical XMR agreement wire")]
    NonCanonicalWire,
    /// Swap ID was all zero.
    #[error("XMR swap ID is empty")]
    EmptySwapId,
    /// Role owner account was empty or reused by both roles.
    #[error("invalid or duplicate LEZ role owner account")]
    InvalidRoleOwners,
    /// One role/purpose secp256k1 key was malformed or noncanonical.
    #[error("invalid {role:?} {purpose} public key")]
    InvalidParticipantKey {
        /// Role owning the key.
        role: XmrRoleV1,
        /// Stable key-purpose label.
        purpose: &'static str,
    },
    /// A secp256k1 key was reused across roles or purposes.
    #[error("agreement, claim, and refund keys must not alias")]
    AliasedParticipantKeys,
    /// Monero chain identity, amount, or confirmation policy was empty.
    #[error("invalid Monero chain or amount policy")]
    InvalidMoneroPolicy,
    /// Maker and Taker supplied the same spend share.
    #[error("Maker and Taker Monero spend shares must be distinct")]
    DuplicateSpendShares,
    /// Re-derived public keys/address differ from countersigned terms.
    #[error("shared Monero address derivation differs from agreement")]
    MoneroAddressDerivationMismatch,
    /// LEZ program, escrow accounts, authority accounts, or amount was invalid.
    #[error("invalid or aliased LEZ deployment terms")]
    InvalidLezTerms,
    /// Taker depositor or Maker claimant does not match role identity.
    #[error("LEZ depositor/claimant mapping differs from fixed direction")]
    LezRoleMismatch,
    /// Aggregate keys/accounts differ from purpose-separated `MuSig2` derivation.
    #[error("LEZ claim/refund aggregate authority derivation mismatch")]
    LezAuthorityMismatch,
    /// Guest DLEQ commitments differ from the verified proof envelopes.
    #[error("LEZ DLEQ commitment differs from the verified proof")]
    LezDleqCommitmentMismatch,
    /// A DLEQ adaptor point aliases an agreement or session signing key.
    #[error("DLEQ adaptor points must not reuse agreement or session signing keys")]
    DleqSigningKeyReuse,
    /// Claim, refund, and punishment messages were empty or not distinct.
    #[error("LEZ claim/refund/punishment messages must be nonzero and distinct")]
    InvalidMessages,
    /// Refund and punishment windows were empty, equal, or out of order.
    #[error("LEZ refund window must precede punishment window")]
    InvalidWindows,
    /// Body commitment differs from the record.
    #[error("XMR agreement commitment mismatch")]
    CommitmentMismatch,
    /// Role signature bytes were malformed.
    #[error("invalid {0:?} BIP340 signature encoding")]
    InvalidSignatureEncoding(XmrRoleV1),
    /// Role signature did not authenticate the body commitment.
    #[error("{0:?} BIP340 signature mismatch")]
    SignatureMismatch(XmrRoleV1),
    /// Cross-curve proof validation failed.
    #[error("cross-curve proof validation failed: {0}")]
    CrossCurve(#[from] CrossCurveDleqError),
    /// Shared-address validation failed.
    #[error("shared Monero address validation failed: {0}")]
    SharedSpend(#[from] MoneroSharedSpendError),
    /// Pair-neutral adaptor context construction failed.
    #[error("adaptor context validation failed: {0}")]
    Adaptor(#[from] AdaptorSessionError),
    /// Purpose-separated claim/refund contexts unexpectedly collided.
    #[error("claim and refund adaptor contexts are not distinct")]
    AdaptorContextsNotDistinct,
    /// A public adaptor descriptor no longer matches its validated session.
    #[error("XMR adaptor session descriptor does not match its validated context")]
    AdaptorSessionDescriptorMismatch,
    /// Stage-B activation schema is unsupported.
    #[error("unsupported XMR activation schema {0}")]
    UnsupportedActivationSchema(u16),
    /// Activation wire exceeded its fixed bound.
    #[error("XMR activation wire exceeds the fixed bound")]
    OversizedActivationWire,
    /// Parsed activation did not exactly match canonical re-encoding.
    #[error("noncanonical XMR activation wire")]
    NonCanonicalActivationWire,
    /// Stage-B fields do not bind exactly to Stage A or their adaptor sessions.
    #[error("XMR activation binding mismatch")]
    ActivationBindingMismatch,
    /// Caller does not possess the private view key committed by Stage A.
    #[error("local Monero private view key does not match the agreement")]
    LocalViewKeyMismatch,
    /// Stored refund presignature differs from aggregation of verified partials.
    #[error("XMR refund presignature mismatch")]
    RefundPresignatureMismatch,
    /// Activation body commitment differs from the signed record.
    #[error("XMR activation commitment mismatch")]
    ActivationCommitmentMismatch,
    /// Published Taker claim partial differs from its exact guest commitment.
    #[error("published Taker claim partial differs from activation commitment")]
    PublishedClaimPartialMismatch,
    /// Valid Stage-B terms could not project into the pair-neutral application lifecycle.
    #[error("XMR initial application coordinator is invalid")]
    InvalidInitialCoordinator,
    /// LEZ candidate differs from the Stage-B init plan or uses empty facts.
    #[error("LEZ lock candidate differs from exact Stage-B terms")]
    LezLockCandidateMismatch,
    /// LEZ candidate reports less than the named profile's finality depth.
    #[error("LEZ lock candidate has {actual} finality units; {required} required")]
    InsufficientLezCandidateFinality {
        /// Current finalized observation depth.
        actual: u32,
        /// Exact named-profile requirement.
        required: u32,
    },
    /// Candidate lock finalized after the exact Maker funding cutoff.
    #[error(
        "LEZ lock candidate finalized at {finalized_consensus_timestamp_ms} ms after funding cutoff {cutoff_ms} ms"
    )]
    LezLockCandidateAfterFundingCutoff {
        /// Candidate finalized consensus timestamp.
        finalized_consensus_timestamp_ms: u64,
        /// Exact Stage-A Maker funding cutoff.
        cutoff_ms: u64,
    },
}

fn validate_wire_field_bounds(body: &XmrAgreementBodyV1) -> Result<(), XmrAgreementV1Error> {
    if body.monero.maker_dleq_proof_wire.is_empty()
        || body.monero.maker_dleq_proof_wire.len() > MAX_DLEQ_WIRE_BYTES
        || body.monero.taker_dleq_proof_wire.is_empty()
        || body.monero.taker_dleq_proof_wire.len() > MAX_DLEQ_WIRE_BYTES
        || body.monero.address.is_empty()
        || body.monero.address.len() > MAX_MONERO_ADDRESS_BYTES
    {
        return Err(XmrAgreementV1Error::OversizedField);
    }
    Ok(())
}

fn validate_cheap_body_invariants(body: &XmrAgreementBodyV1) -> Result<(), XmrAgreementV1Error> {
    if body.direction != XmrSwapDirectionV1::TakerSellsLez {
        return Err(XmrAgreementV1Error::UnsupportedDirection);
    }
    if body.swap_id == [0; 32] {
        return Err(XmrAgreementV1Error::EmptySwapId);
    }
    let maker_owner = body.participants.maker.lez_owner_account;
    let taker_owner = body.participants.taker.lez_owner_account;
    if maker_owner == [0; 32] || taker_owner == [0; 32] || maker_owner == taker_owner {
        return Err(XmrAgreementV1Error::InvalidRoleOwners);
    }
    if body.monero.network != body.profile.network()
        || body.monero.genesis_hash == [0; 32]
        || body.monero.amount_piconero == 0
        || body.monero.required_confirmations != body.profile.required_monero_confirmations()
    {
        return Err(XmrAgreementV1Error::InvalidMoneroPolicy);
    }
    if body.lez.channel_id == [0; 32]
        || body.lez.genesis_hash == [0; 32]
        || body.lez.escrow_program_id == [0; 8]
        || body.lez.authenticated_transfer_program_id == [0; 8]
        || body.lez.escrow_program_id == body.lez.authenticated_transfer_program_id
        || body.lez.required_finality_units != body.profile.required_lez_finality_units()
        || body.lez.maker_dleq_transcript_commitment == [0; 32]
        || body.lez.taker_dleq_transcript_commitment == [0; 32]
        || body.lez.maker_dleq_transcript_commitment == body.lez.taker_dleq_transcript_commitment
        || body.lez.amount == 0
    {
        return Err(XmrAgreementV1Error::InvalidLezTerms);
    }
    let accounts = [
        body.lez.metadata_account,
        body.lez.custody_account,
        body.lez.depositor_account,
        body.lez.claimant_account,
        body.lez.claim_authority_account,
        body.lez.refund_authority_account,
    ];
    if accounts.contains(&[0; 32]) || !all_distinct(&accounts) {
        return Err(XmrAgreementV1Error::InvalidLezTerms);
    }
    if body.lez.depositor_account != taker_owner || body.lez.claimant_account != maker_owner {
        return Err(XmrAgreementV1Error::LezRoleMismatch);
    }
    if body.messages.claim == [0; 32]
        || body.messages.refund == [0; 32]
        || body.messages.punish == [0; 32]
        || body.messages.claim == body.messages.refund
        || body.messages.claim == body.messages.punish
        || body.messages.refund == body.messages.punish
    {
        return Err(XmrAgreementV1Error::InvalidMessages);
    }
    if body.windows.maker_xmr_funding_cutoff_ms == 0
        || body.windows.refund_at_ms == 0
        || body.windows.punish_at_ms == 0
        || !body
            .windows
            .maker_xmr_funding_cutoff_ms
            .is_multiple_of(1_000)
        || !body.windows.refund_at_ms.is_multiple_of(1_000)
        || !body.windows.punish_at_ms.is_multiple_of(1_000)
        || body.windows.maker_xmr_funding_cutoff_ms >= body.windows.refund_at_ms
        || body.windows.refund_at_ms >= body.windows.punish_at_ms
        || body.windows.refund_at_ms - body.windows.maker_xmr_funding_cutoff_ms
            < body.profile.minimum_funding_to_refund_ms()
        || body.windows.punish_at_ms - body.windows.refund_at_ms
            < body.profile.minimum_refund_to_punish_ms()
    {
        return Err(XmrAgreementV1Error::InvalidWindows);
    }
    Ok(())
}

fn validate_participant_keys(
    participants: &XmrParticipantsV1,
) -> Result<(PublicKey, PublicKey), XmrAgreementV1Error> {
    let encoded = [
        participants.maker.agreement_public_key,
        participants.taker.agreement_public_key,
        participants.maker.claim_session_public_key,
        participants.taker.claim_session_public_key,
        participants.maker.refund_session_public_key,
        participants.taker.refund_session_public_key,
    ];
    if !all_distinct(&encoded) {
        return Err(XmrAgreementV1Error::AliasedParticipantKeys);
    }
    let maker_agreement = parse_key(
        participants.maker.agreement_public_key,
        XmrRoleV1::Maker,
        "agreement",
    )?;
    let taker_agreement = parse_key(
        participants.taker.agreement_public_key,
        XmrRoleV1::Taker,
        "agreement",
    )?;
    let mut parsed = vec![maker_agreement, taker_agreement];
    for (bytes, role, purpose) in [
        (
            participants.maker.claim_session_public_key,
            XmrRoleV1::Maker,
            "claim-session",
        ),
        (
            participants.taker.claim_session_public_key,
            XmrRoleV1::Taker,
            "claim-session",
        ),
        (
            participants.maker.refund_session_public_key,
            XmrRoleV1::Maker,
            "refund-session",
        ),
        (
            participants.taker.refund_session_public_key,
            XmrRoleV1::Taker,
            "refund-session",
        ),
    ] {
        parsed.push(parse_key(bytes, role, purpose)?);
    }
    let x_only = parsed
        .iter()
        .map(|key| key.x_only_public_key().0.serialize())
        .collect::<Vec<_>>();
    if !all_distinct(&x_only) {
        return Err(XmrAgreementV1Error::AliasedParticipantKeys);
    }
    Ok((parsed[0], parsed[1]))
}

fn reject_dleq_signing_key_reuse(
    participants: &XmrParticipantsV1,
    dleq_points: [[u8; 33]; 2],
) -> Result<(), XmrAgreementV1Error> {
    let signing_x_only = [
        participants.maker.agreement_public_key,
        participants.taker.agreement_public_key,
        participants.maker.claim_session_public_key,
        participants.taker.claim_session_public_key,
        participants.maker.refund_session_public_key,
        participants.taker.refund_session_public_key,
    ]
    .map(|bytes| {
        PublicKey::from_slice(&bytes)
            .expect("participant keys were validated before DLEQ reuse")
            .x_only_public_key()
            .0
            .serialize()
    });
    for dleq in dleq_points {
        let dleq_x_only = PublicKey::from_slice(&dleq)
            .map_err(|_| XmrAgreementV1Error::DleqSigningKeyReuse)?
            .x_only_public_key()
            .0
            .serialize();
        if signing_x_only.contains(&dleq_x_only) {
            return Err(XmrAgreementV1Error::DleqSigningKeyReuse);
        }
    }
    Ok(())
}

fn parse_key(
    bytes: [u8; 33],
    role: XmrRoleV1,
    purpose: &'static str,
) -> Result<PublicKey, XmrAgreementV1Error> {
    let key = PublicKey::from_slice(&bytes)
        .map_err(|_| XmrAgreementV1Error::InvalidParticipantKey { role, purpose })?;
    if key.serialize() != bytes {
        return Err(XmrAgreementV1Error::InvalidParticipantKey { role, purpose });
    }
    Ok(key)
}

fn aggregate_x_only(ordered_keys: [[u8; 33]; 2]) -> Result<[u8; 32], XmrAgreementV1Error> {
    let maker = MusigPoint::from_slice(&ordered_keys[0]).map_err(|_| {
        XmrAgreementV1Error::InvalidParticipantKey {
            role: XmrRoleV1::Maker,
            purpose: "session",
        }
    })?;
    let taker = MusigPoint::from_slice(&ordered_keys[1]).map_err(|_| {
        XmrAgreementV1Error::InvalidParticipantKey {
            role: XmrRoleV1::Taker,
            purpose: "session",
        }
    })?;
    KeyAggContext::new([maker, taker])
        .map(|context| context.aggregated_pubkey::<MusigPoint>().serialize_xonly())
        .map_err(|_| XmrAgreementV1Error::AliasedParticipantKeys)
}

fn validate_lez_authorities(
    lez: &XmrLezTermsV1,
    claim: &AdaptorSessionContext,
    refund: &AdaptorSessionContext,
) -> Result<(), XmrAgreementV1Error> {
    let claim_key = claim.output_key();
    let refund_key = refund.output_key();
    if claim_key == refund_key
        || lez.claim_aggregate_x_only_key != claim_key
        || lez.refund_aggregate_x_only_key != refund_key
        || lez.claim_authority_account != witnessed_account_id(claim_key)
        || lez.refund_authority_account != witnessed_account_id(refund_key)
    {
        return Err(XmrAgreementV1Error::LezAuthorityMismatch);
    }
    Ok(())
}

fn verify_role_signature(
    role: XmrRoleV1,
    public_key: PublicKey,
    signature: [u8; 64],
    commitment: [u8; 32],
) -> Result<(), XmrAgreementV1Error> {
    let signature = SchnorrSignature::from_slice(&signature)
        .map_err(|_| XmrAgreementV1Error::InvalidSignatureEncoding(role))?;
    let x_only: XOnlyPublicKey = public_key.x_only_public_key().0;
    Secp256k1::verification_only()
        .verify_schnorr(&signature, &Message::from_digest(commitment), &x_only)
        .map_err(|_| XmrAgreementV1Error::SignatureMismatch(role))
}

fn session_id(commitment: [u8; 32], purpose: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(SESSION_DOMAIN);
    hasher.update(commitment);
    hasher.update(purpose);
    hasher.finalize().into()
}

fn validate_session_transcript(
    context: &AdaptorSessionContext,
    transcript: &XmrSessionTranscriptV1,
) -> Result<(), XmrAgreementV1Error> {
    verify_nonce_commitment(
        context,
        SigningRole::Maker,
        transcript.maker_nonce_commitment,
        transcript.maker_public_nonce,
    )?;
    verify_nonce_commitment(
        context,
        SigningRole::Taker,
        transcript.taker_nonce_commitment,
        transcript.taker_public_nonce,
    )?;
    Ok(())
}

fn claim_partial_context_binding(
    base_agreement_commitment: [u8; 32],
    claim_session_context_binding: [u8; 32],
    transcript: &XmrSessionTranscriptV1,
    maker_claim_partial: [u8; 32],
) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(CLAIM_PARTIAL_CONTEXT_DOMAIN);
    hasher.update(base_agreement_commitment);
    hasher.update(claim_session_context_binding);
    let mut transcript_bytes = Vec::with_capacity(196);
    transcript.encode_into(&mut transcript_bytes);
    hasher.update(transcript_bytes);
    hasher.update(maker_claim_partial);
    hasher.finalize().into()
}

fn claim_partial_commitment(
    claim_partial_context_binding: [u8; 32],
    taker_claim_partial: [u8; 32],
) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(CLAIM_PARTIAL_COMMITMENT_DOMAIN);
    hasher.update(claim_partial_context_binding);
    hasher.update(taker_claim_partial);
    hasher.finalize().into()
}

fn witnessed_account_id(x_only_public_key: [u8; 32]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(PUBLIC_ACCOUNT_ID_PREFIX);
    hasher.update(x_only_public_key);
    hasher.finalize().into()
}

const fn network_tag(network: MoneroAddressNetworkV1) -> u8 {
    match network {
        MoneroAddressNetworkV1::Regtest => 0,
        MoneroAddressNetworkV1::Stagenet => 1,
    }
}

fn network_from_tag(tag: u8) -> Result<MoneroAddressNetworkV1, XmrAgreementV1Error> {
    match tag {
        0 => Ok(MoneroAddressNetworkV1::Regtest),
        1 => Ok(MoneroAddressNetworkV1::Stagenet),
        _ => Err(XmrAgreementV1Error::MalformedWire),
    }
}

fn encode_identity(bytes: &mut Vec<u8>, identity: &XmrParticipantIdentityV1) {
    bytes.extend_from_slice(&identity.lez_owner_account);
    bytes.extend_from_slice(&identity.agreement_public_key);
    bytes.extend_from_slice(&identity.claim_session_public_key);
    bytes.extend_from_slice(&identity.refund_session_public_key);
}

fn encode_vec(bytes: &mut Vec<u8>, value: &[u8]) {
    let length = u32::try_from(value.len()).expect("bounded fields fit u32");
    bytes.extend_from_slice(&length.to_le_bytes());
    bytes.extend_from_slice(value);
}

fn all_distinct<T: Eq>(values: &[T]) -> bool {
    values
        .iter()
        .enumerate()
        .all(|(index, value)| !values[index + 1..].contains(value))
}

fn encode_unsigned_agreement_wire(
    schema_version: u16,
    body: &XmrAgreementBodyV1,
    agreement_commitment: [u8; 32],
) -> Result<Vec<u8>, XmrAgreementV1Error> {
    validate_wire_field_bounds(body)?;
    let body = body.encode_body();
    let mut bytes = Vec::with_capacity(2 + body.len() + 32);
    bytes.extend_from_slice(&schema_version.to_le_bytes());
    bytes.extend_from_slice(&body);
    bytes.extend_from_slice(&agreement_commitment);
    if bytes.len() > MAX_XMR_UNSIGNED_STAGE_A_WIRE_BYTES {
        return Err(XmrAgreementV1Error::OversizedWire);
    }
    Ok(bytes)
}

fn encode_unsigned_activation_wire(
    schema_version: u16,
    body: &XmrActivationBodyV1,
    activation_commitment: [u8; 32],
) -> Result<Vec<u8>, XmrAgreementV1Error> {
    let body = body.encode_body();
    let mut bytes = Vec::with_capacity(2 + body.len() + 32);
    bytes.extend_from_slice(&schema_version.to_le_bytes());
    bytes.extend_from_slice(&body);
    bytes.extend_from_slice(&activation_commitment);
    if bytes.len() > MAX_XMR_UNSIGNED_STAGE_B_WIRE_BYTES {
        return Err(XmrAgreementV1Error::OversizedActivationWire);
    }
    Ok(bytes)
}

fn decode_agreement_prefix(
    reader: &mut Reader<'_>,
) -> Result<(u16, XmrAgreementBodyV1, [u8; 32]), XmrAgreementV1Error> {
    let schema = reader.u16()?;
    let direction = XmrSwapDirectionV1::from_tag(reader.u8()?)?;
    let profile = XmrNamedProfileV1::from_tag(reader.u8()?)?;
    let swap_id = reader.fixed()?;
    let participants = XmrParticipantsV1::new(decode_identity(reader)?, decode_identity(reader)?);
    let network = network_from_tag(reader.u8()?)?;
    let genesis_hash = reader.fixed()?;
    let amount_piconero = reader.u64()?;
    let confirmations = reader.u32()?;
    let maker_proof = reader.bounded_vec(MAX_DLEQ_WIRE_BYTES)?;
    let taker_proof = reader.bounded_vec(MAX_DLEQ_WIRE_BYTES)?;
    let public_view_key = reader.fixed()?;
    let public_spend_key = reader.fixed()?;
    let address = reader.bounded_string(MAX_MONERO_ADDRESS_BYTES)?;
    let channel_id = reader.fixed()?;
    let lez_genesis_hash = reader.fixed()?;
    let mut program = [0_u32; 8];
    for word in &mut program {
        *word = reader.u32()?;
    }
    let mut authenticated_transfer_program = [0_u32; 8];
    for word in &mut authenticated_transfer_program {
        *word = reader.u32()?;
    }
    let required_finality_units = reader.u32()?;
    let lez = XmrLezTermsV1::new(
        channel_id,
        lez_genesis_hash,
        program,
        authenticated_transfer_program,
        required_finality_units,
        reader.fixed()?,
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
    );
    let messages = XmrMessagesV1::new(reader.fixed()?, reader.fixed()?, reader.fixed()?);
    let windows = XmrWindowsV1::new(reader.u64()?, reader.u64()?, reader.u64()?);
    let body = XmrAgreementBodyV1::new(
        direction,
        profile,
        swap_id,
        participants,
        XmrMoneroTermsV1::new(
            network,
            genesis_hash,
            amount_piconero,
            confirmations,
            maker_proof,
            taker_proof,
            public_view_key,
            public_spend_key,
            address,
        ),
        lez,
        messages,
        windows,
    );
    Ok((schema, body, reader.fixed()?))
}

fn decode_unsigned_agreement_wire(
    bytes: &[u8],
) -> Result<(u16, XmrAgreementBodyV1, [u8; 32]), XmrAgreementV1Error> {
    let mut reader = Reader::new(bytes);
    let decoded = decode_agreement_prefix(&mut reader)?;
    reader.finish()?;
    Ok(decoded)
}

fn decode_record(bytes: &[u8]) -> Result<XmrAgreementRecordV1, XmrAgreementV1Error> {
    let mut reader = Reader::new(bytes);
    let (schema, body, agreement_commitment) = decode_agreement_prefix(&mut reader)?;
    let record = XmrAgreementRecordV1::from_parts(
        schema,
        body,
        agreement_commitment,
        reader.fixed()?,
        reader.fixed()?,
    );
    reader.finish()?;
    Ok(record)
}

fn decode_activation_prefix(
    reader: &mut Reader<'_>,
) -> Result<(u16, XmrActivationBodyV1, [u8; 32]), XmrAgreementV1Error> {
    let schema = reader.u16()?;
    let body = XmrActivationBodyV1::new(
        reader.fixed()?,
        reader.fixed()?,
        decode_session_transcript(reader)?,
        reader.fixed()?,
        reader.fixed()?,
        reader.fixed()?,
        reader.fixed()?,
        decode_session_transcript(reader)?,
        reader.fixed()?,
        reader.fixed()?,
        reader.fixed()?,
    );
    Ok((schema, body, reader.fixed()?))
}

fn decode_unsigned_activation_wire(
    bytes: &[u8],
) -> Result<(u16, XmrActivationBodyV1, [u8; 32]), XmrAgreementV1Error> {
    let mut reader = Reader::new(bytes);
    let decoded = decode_activation_prefix(&mut reader)?;
    reader.finish()?;
    Ok(decoded)
}

fn decode_activation_record(bytes: &[u8]) -> Result<XmrActivationRecordV1, XmrAgreementV1Error> {
    let mut reader = Reader::new(bytes);
    let (schema, body, activation_commitment) = decode_activation_prefix(&mut reader)?;
    let record = XmrActivationRecordV1::from_parts(
        schema,
        body,
        activation_commitment,
        reader.fixed()?,
        reader.fixed()?,
    );
    reader.finish()?;
    Ok(record)
}

fn decode_session_transcript(
    reader: &mut Reader<'_>,
) -> Result<XmrSessionTranscriptV1, XmrAgreementV1Error> {
    Ok(XmrSessionTranscriptV1::new(
        reader.fixed()?,
        reader.fixed()?,
        reader.fixed()?,
        reader.fixed()?,
    ))
}

fn decode_identity(
    reader: &mut Reader<'_>,
) -> Result<XmrParticipantIdentityV1, XmrAgreementV1Error> {
    Ok(XmrParticipantIdentityV1::new(
        reader.fixed()?,
        reader.fixed()?,
        reader.fixed()?,
        reader.fixed()?,
    ))
}

struct Reader<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> Reader<'a> {
    const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, position: 0 }
    }

    fn fixed<const N: usize>(&mut self) -> Result<[u8; N], XmrAgreementV1Error> {
        let end = self
            .position
            .checked_add(N)
            .ok_or(XmrAgreementV1Error::MalformedWire)?;
        let value = self
            .bytes
            .get(self.position..end)
            .and_then(|value| value.try_into().ok())
            .ok_or(XmrAgreementV1Error::MalformedWire)?;
        self.position = end;
        Ok(value)
    }

    fn u8(&mut self) -> Result<u8, XmrAgreementV1Error> {
        Ok(self.fixed::<1>()?[0])
    }
    fn u16(&mut self) -> Result<u16, XmrAgreementV1Error> {
        Ok(u16::from_le_bytes(self.fixed()?))
    }
    fn u32(&mut self) -> Result<u32, XmrAgreementV1Error> {
        Ok(u32::from_le_bytes(self.fixed()?))
    }
    fn u64(&mut self) -> Result<u64, XmrAgreementV1Error> {
        Ok(u64::from_le_bytes(self.fixed()?))
    }
    fn u128(&mut self) -> Result<u128, XmrAgreementV1Error> {
        Ok(u128::from_le_bytes(self.fixed()?))
    }

    fn bounded_vec(&mut self, maximum: usize) -> Result<Vec<u8>, XmrAgreementV1Error> {
        let length =
            usize::try_from(self.u32()?).map_err(|_| XmrAgreementV1Error::MalformedWire)?;
        if length > maximum {
            return Err(XmrAgreementV1Error::OversizedField);
        }
        let end = self
            .position
            .checked_add(length)
            .ok_or(XmrAgreementV1Error::MalformedWire)?;
        let value = self
            .bytes
            .get(self.position..end)
            .ok_or(XmrAgreementV1Error::MalformedWire)?;
        self.position = end;
        Ok(value.to_vec())
    }

    fn bounded_string(&mut self, maximum: usize) -> Result<String, XmrAgreementV1Error> {
        String::from_utf8(self.bounded_vec(maximum)?)
            .map_err(|_| XmrAgreementV1Error::MalformedWire)
    }

    fn finish(self) -> Result<(), XmrAgreementV1Error> {
        if self.position == self.bytes.len() {
            Ok(())
        } else {
            Err(XmrAgreementV1Error::MalformedWire)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::OnceLock;

    use lez_adaptor_signature::AdaptorSigner;
    use rand_chacha::{ChaCha20Rng, rand_core::SeedableRng as _};
    use secp256k1::{Keypair, SecretKey};

    use super::*;
    use crate::{CrossCurveScalar, MoneroPrivateViewKey};

    const MAKER_AGREEMENT_SECRET: [u8; 32] = [7; 32];
    const TAKER_AGREEMENT_SECRET: [u8; 32] = [8; 32];
    const MAKER_CLAIM_SECRET: [u8; 32] = [9; 32];
    const TAKER_CLAIM_SECRET: [u8; 32] = [10; 32];
    const MAKER_REFUND_SECRET: [u8; 32] = [11; 32];
    const TAKER_REFUND_SECRET: [u8; 32] = [12; 32];
    const VIEW_KEY_BYTES: [u8; 32] = {
        let mut bytes = [0; 32];
        bytes[0] = 17;
        bytes
    };

    struct ProofFixture {
        maker_wire: Vec<u8>,
        taker_wire: Vec<u8>,
        view_public: [u8; 32],
        spend_public: [u8; 32],
        address: String,
        maker_transcript_commitment: [u8; 32],
        taker_transcript_commitment: [u8; 32],
        maker_secp_public: [u8; 33],
    }

    fn proofs() -> &'static ProofFixture {
        static FIXTURE: OnceLock<ProofFixture> = OnceLock::new();
        FIXTURE.get_or_init(|| {
            let maker = scalar(11);
            let taker = scalar(13);
            let maker_proof =
                CrossCurveDleqProofV1::prove(&maker, &mut ChaCha20Rng::from_seed([71; 32]))
                    .expect("Maker proof");
            let taker_proof =
                CrossCurveDleqProofV1::prove(&taker, &mut ChaCha20Rng::from_seed([72; 32]))
                    .expect("Taker proof");
            let view = MoneroPrivateViewKey::from_monero_little_endian(VIEW_KEY_BYTES)
                .expect("private view key");
            let address = MoneroSharedAddressV1::derive(
                MoneroAddressNetworkV1::Regtest,
                &maker_proof,
                &taker_proof,
                &view,
            )
            .expect("shared address");
            ProofFixture {
                maker_wire: maker_proof.to_wire_bytes().expect("Maker wire"),
                taker_wire: taker_proof.to_wire_bytes().expect("Taker wire"),
                view_public: address.public_view_key(),
                spend_public: address.public_spend_key(),
                address: address.address_string(),
                maker_transcript_commitment: maker_proof.transcript_commitment(),
                taker_transcript_commitment: taker_proof.transcript_commitment(),
                maker_secp_public: maker_proof.secp256k1_public_key(),
            }
        })
    }

    fn scalar(value: u8) -> CrossCurveScalar {
        let mut bytes = [0_u8; 32];
        bytes[0] = value;
        CrossCurveScalar::from_monero_little_endian(bytes).expect("fixture scalar")
    }

    fn public_key(secret: [u8; 32]) -> [u8; 33] {
        let secret = SecretKey::from_slice(&secret).expect("fixture secret");
        PublicKey::from_secret_key(&Secp256k1::new(), &secret).serialize()
    }

    fn participants() -> XmrParticipantsV1 {
        XmrParticipantsV1::new(
            XmrParticipantIdentityV1::new(
                [21; 32],
                public_key(MAKER_AGREEMENT_SECRET),
                public_key(MAKER_CLAIM_SECRET),
                public_key(MAKER_REFUND_SECRET),
            ),
            XmrParticipantIdentityV1::new(
                [22; 32],
                public_key(TAKER_AGREEMENT_SECRET),
                public_key(TAKER_CLAIM_SECRET),
                public_key(TAKER_REFUND_SECRET),
            ),
        )
    }

    fn body() -> XmrAgreementBodyV1 {
        let proof = proofs();
        let participants = participants();
        let claim_key = participants
            .claim_aggregate_x_only_key()
            .expect("claim aggregate");
        let refund_key = participants
            .refund_aggregate_x_only_key()
            .expect("refund aggregate");
        XmrAgreementBodyV1::new(
            XmrSwapDirectionV1::TakerSellsLez,
            XmrNamedProfileV1::AcceleratedRegtest,
            [19; 32],
            participants,
            XmrMoneroTermsV1::new(
                MoneroAddressNetworkV1::Regtest,
                [31; 32],
                1_000_000_000_000,
                10,
                proof.maker_wire.clone(),
                proof.taker_wire.clone(),
                proof.view_public,
                proof.spend_public,
                proof.address.clone(),
            ),
            XmrLezTermsV1::new(
                [40; 32],
                [41; 32],
                [42; 8],
                [43; 8],
                LEZ_FINALITY_UNITS,
                [44; 32],
                [45; 32],
                [22; 32],
                [21; 32],
                claim_key,
                XmrLezTermsV1::authority_account_for_key(claim_key),
                refund_key,
                XmrLezTermsV1::authority_account_for_key(refund_key),
                proof.maker_transcript_commitment,
                proof.taker_transcript_commitment,
                500,
            ),
            XmrMessagesV1::new([51; 32], [52; 32], [53; 32]),
            XmrWindowsV1::new(10_000, 20_000, 30_000),
        )
    }

    fn view_key() -> MoneroPrivateViewKey {
        MoneroPrivateViewKey::from_monero_little_endian(VIEW_KEY_BYTES).expect("private view key")
    }

    fn signer_round(
        context: &AdaptorSessionContext,
        maker_secret: [u8; 32],
        taker_secret: [u8; 32],
    ) -> (XmrSessionTranscriptV1, [u8; 32], [u8; 32], [u8; 65]) {
        let mut maker = AdaptorSigner::new(context.clone(), SigningRole::Maker, maker_secret)
            .expect("Maker signer");
        let mut taker = AdaptorSigner::new(context.clone(), SigningRole::Taker, taker_secret)
            .expect("Taker signer");
        let maker_commitment = maker.nonce_commitment();
        let taker_commitment = taker.nonce_commitment();
        maker
            .accept_peer_commitment(taker_commitment)
            .expect("Maker accepts commitment");
        taker
            .accept_peer_commitment(maker_commitment)
            .expect("Taker accepts commitment");
        let maker_nonce = maker.public_nonce().expect("Maker public nonce");
        let taker_nonce = taker.public_nonce().expect("Taker public nonce");
        maker
            .accept_peer_nonce(taker_nonce)
            .expect("Maker accepts nonce opening");
        taker
            .accept_peer_nonce(maker_nonce)
            .expect("Taker accepts nonce opening");
        let maker_partial = maker.create_partial_signature().expect("Maker partial");
        let taker_partial = taker.create_partial_signature().expect("Taker partial");
        maker
            .accept_peer_partial_signature(taker_partial)
            .expect("Maker verifies Taker partial");
        taker
            .accept_peer_partial_signature(maker_partial)
            .expect("Taker verifies Maker partial");
        let maker_presignature = maker.presignature().expect("Maker presignature");
        assert_eq!(
            maker_presignature,
            taker.presignature().expect("Taker presignature")
        );
        (
            XmrSessionTranscriptV1::new(
                maker_commitment,
                taker_commitment,
                maker_nonce,
                taker_nonce,
            ),
            maker_partial,
            taker_partial,
            maker_presignature,
        )
    }

    fn activation_record(agreement: &XmrAgreementV1) -> (XmrActivationRecordV1, [u8; 32]) {
        let (claim_transcript, maker_claim_partial, taker_claim_partial, _) = signer_round(
            &agreement.claim_context,
            MAKER_CLAIM_SECRET,
            TAKER_CLAIM_SECRET,
        );
        let (refund_transcript, maker_refund_partial, taker_refund_partial, refund_presignature) =
            signer_round(
                &agreement.refund_context,
                MAKER_REFUND_SECRET,
                TAKER_REFUND_SECRET,
            );
        let partial_context = agreement
            .claim_partial_context_binding(&claim_transcript, maker_claim_partial)
            .expect("claim partial context");
        let partial_commitment = agreement
            .commit_taker_claim_partial(&claim_transcript, maker_claim_partial, taker_claim_partial)
            .expect("Taker partial commitment");
        let activation_body = XmrActivationBodyV1::new(
            agreement.agreement_commitment(),
            agreement.claim_context_binding(),
            claim_transcript,
            maker_claim_partial,
            partial_context,
            partial_commitment,
            agreement.refund_context_binding(),
            refund_transcript,
            maker_refund_partial,
            taker_refund_partial,
            refund_presignature,
        );
        let commitment = activation_body.commitment();
        (
            XmrActivationRecordV1::from_parts(
                XMR_ACTIVATION_SCHEMA_V1,
                activation_body,
                commitment,
                sign(MAKER_AGREEMENT_SECRET, commitment),
                sign(TAKER_AGREEMENT_SECRET, commitment),
            ),
            taker_claim_partial,
        )
    }

    fn resign_activation(record: &mut XmrActivationRecordV1) {
        let commitment = record.body.commitment();
        record.activation_commitment = commitment;
        record.maker_signature = sign(MAKER_AGREEMENT_SECRET, commitment);
        record.taker_signature = sign(TAKER_AGREEMENT_SECRET, commitment);
    }

    fn lez_lock_candidate(
        activation: &XmrActivatedAgreementV1,
        agreement: &XmrAgreementV1,
    ) -> XmrLezLockCandidateV1 {
        XmrLezLockCandidateV1::new(
            activation
                .lez_initialize_plan(agreement)
                .expect("init plan"),
            XMR_METADATA_VERSION_V3,
            XmrLezLockStatusV1::Funded,
            LEZ_FINALITY_UNITS,
            agreement.body().windows().maker_xmr_funding_cutoff_ms(),
            [61; 32],
            [62; 32],
        )
    }

    fn sign(role_secret: [u8; 32], commitment: [u8; 32]) -> [u8; 64] {
        let secret = SecretKey::from_slice(&role_secret).expect("fixture secret");
        let secp = Secp256k1::new();
        secp.sign_schnorr_no_aux_rand(
            &Message::from_digest(commitment),
            &Keypair::from_secret_key(&secp, &secret),
        )
        .serialize()
    }

    fn signed_record(body: XmrAgreementBodyV1) -> XmrAgreementRecordV1 {
        let commitment = body.commitment();
        XmrAgreementRecordV1::from_parts(
            XMR_AGREEMENT_SCHEMA_V1,
            body,
            commitment,
            sign(MAKER_AGREEMENT_SECRET, commitment),
            sign(TAKER_AGREEMENT_SECRET, commitment),
        )
    }

    #[test]
    fn unsigned_stage_a_wire_is_the_checked_signed_prefix_and_fails_closed() {
        let expected_record = signed_record(body());
        let signed_wire = expected_record.encode_wire().expect("signed Stage-A wire");
        let validated = ValidatedXmrAgreementBodyV1::validate(expected_record.body.clone())
            .expect("validated unsigned Stage A");
        let unsigned_wire = validated
            .encode_unsigned_wire()
            .expect("canonical unsigned Stage-A wire");

        assert_eq!(
            unsigned_wire,
            signed_wire[..signed_wire.len() - 128],
            "unsigned Stage A must reuse the signed record prefix exactly"
        );
        assert!(unsigned_wire.len() <= MAX_XMR_UNSIGNED_STAGE_A_WIRE_BYTES);
        let decoded = ValidatedXmrAgreementBodyV1::from_unsigned_wire(&unsigned_wire)
            .expect("checked unsigned Stage-A wire");
        assert_eq!(decoded.body(), expected_record.body());
        assert_eq!(decoded.commitment(), expected_record.body().commitment());
        assert_eq!(
            decoded
                .encode_unsigned_wire()
                .expect("canonical round trip"),
            unsigned_wire
        );

        let mut wrong_schema = unsigned_wire.clone();
        wrong_schema[..2].copy_from_slice(&2_u16.to_le_bytes());
        assert_eq!(
            ValidatedXmrAgreementBodyV1::from_unsigned_wire(&wrong_schema)
                .expect_err("unknown unsigned Stage-A schema"),
            XmrAgreementV1Error::UnsupportedSchema(2)
        );

        let mut wrong_commitment = unsigned_wire.clone();
        *wrong_commitment.last_mut().expect("commitment byte") ^= 1;
        assert_eq!(
            ValidatedXmrAgreementBodyV1::from_unsigned_wire(&wrong_commitment)
                .expect_err("changed unsigned Stage-A commitment"),
            XmrAgreementV1Error::CommitmentMismatch
        );

        let mut trailing = unsigned_wire.clone();
        trailing.push(0);
        assert!(matches!(
            ValidatedXmrAgreementBodyV1::from_unsigned_wire(&trailing),
            Err(XmrAgreementV1Error::MalformedWire)
        ));
        assert!(matches!(
            ValidatedXmrAgreementBodyV1::from_unsigned_wire(
                &unsigned_wire[..unsigned_wire.len() - 1]
            ),
            Err(XmrAgreementV1Error::MalformedWire)
        ));
        assert!(matches!(
            ValidatedXmrAgreementBodyV1::from_unsigned_wire(&signed_wire),
            Err(XmrAgreementV1Error::MalformedWire)
        ));
        assert_eq!(
            ValidatedXmrAgreementBodyV1::from_unsigned_wire(&vec![
                0;
                MAX_XMR_UNSIGNED_STAGE_A_WIRE_BYTES
                    + 1
            ])
            .expect_err("oversized unsigned Stage-A wire"),
            XmrAgreementV1Error::OversizedWire
        );
    }

    #[test]
    fn unsigned_stage_b_exposes_exact_role_comparison_fields() {
        let agreement = XmrAgreementV1::validate(signed_record(body())).expect("agreement");
        let (expected_record, _) = activation_record(&agreement);
        let validated = ValidatedXmrActivationBodyV1::validate(
            &agreement,
            expected_record.body.clone(),
            &view_key(),
        )
        .expect("validated unsigned Stage B");
        let activation = validated.body();
        assert_eq!(
            activation.base_agreement_commitment(),
            agreement.agreement_commitment()
        );
        assert_eq!(
            activation.claim_context_binding(),
            expected_record.body.claim_context_binding
        );
        assert_eq!(
            activation.claim_transcript(),
            &expected_record.body.claim_transcript
        );
        assert_eq!(
            activation.claim_transcript().maker_nonce_commitment(),
            expected_record.body.claim_transcript.maker_nonce_commitment
        );
        assert_eq!(
            activation.claim_transcript().taker_nonce_commitment(),
            expected_record.body.claim_transcript.taker_nonce_commitment
        );
        assert_eq!(
            activation.claim_transcript().maker_public_nonce(),
            expected_record.body.claim_transcript.maker_public_nonce
        );
        assert_eq!(
            activation.claim_transcript().taker_public_nonce(),
            expected_record.body.claim_transcript.taker_public_nonce
        );
        assert_eq!(
            activation.maker_claim_partial(),
            expected_record.body.maker_claim_partial
        );
        assert_eq!(
            activation.claim_partial_context_binding(),
            expected_record.body.claim_partial_context_binding
        );
        assert_eq!(
            activation.claim_partial_commitment(),
            expected_record.body.claim_partial_commitment
        );
        assert_eq!(
            activation.refund_context_binding(),
            expected_record.body.refund_context_binding
        );
        assert_eq!(
            activation.refund_transcript(),
            &expected_record.body.refund_transcript
        );
        assert_eq!(
            activation.maker_refund_partial(),
            expected_record.body.maker_refund_partial
        );
        assert_eq!(
            activation.taker_refund_partial(),
            expected_record.body.taker_refund_partial
        );
        assert_eq!(
            activation.refund_presignature(),
            expected_record.body.refund_presignature
        );
    }

    #[test]
    fn unsigned_stage_b_wire_is_the_checked_signed_prefix_and_fails_closed() {
        let agreement = XmrAgreementV1::validate(signed_record(body())).expect("agreement");
        let (expected_record, _) = activation_record(&agreement);
        let signed_wire = expected_record.encode_wire().expect("signed Stage-B wire");
        let validated = ValidatedXmrActivationBodyV1::validate(
            &agreement,
            expected_record.body.clone(),
            &view_key(),
        )
        .expect("validated unsigned Stage B");
        let unsigned_wire = validated
            .encode_unsigned_wire()
            .expect("canonical unsigned Stage-B wire");

        assert_eq!(unsigned_wire, signed_wire[..signed_wire.len() - 128]);
        assert!(unsigned_wire.len() <= MAX_XMR_UNSIGNED_STAGE_B_WIRE_BYTES);
        let decoded = ValidatedXmrActivationBodyV1::from_unsigned_wire(
            &agreement,
            &unsigned_wire,
            &view_key(),
        )
        .expect("checked unsigned Stage-B wire");
        assert_eq!(decoded.body(), expected_record.body());
        assert_eq!(decoded.commitment(), expected_record.body.commitment());

        for invalid in [
            unsigned_wire[..unsigned_wire.len() - 1].to_vec(),
            signed_wire,
        ] {
            assert!(matches!(
                ValidatedXmrActivationBodyV1::from_unsigned_wire(&agreement, &invalid, &view_key(),),
                Err(XmrAgreementV1Error::MalformedWire)
            ));
        }
        let mut wrong_commitment = unsigned_wire;
        *wrong_commitment.last_mut().expect("commitment byte") ^= 1;
        assert_eq!(
            ValidatedXmrActivationBodyV1::from_unsigned_wire(
                &agreement,
                &wrong_commitment,
                &view_key(),
            )
            .expect_err("changed unsigned Stage-B commitment"),
            XmrAgreementV1Error::ActivationCommitmentMismatch
        );

        let mut wrong_schema = wrong_commitment;
        wrong_schema[..2].copy_from_slice(&2_u16.to_le_bytes());
        assert_eq!(
            ValidatedXmrActivationBodyV1::from_unsigned_wire(
                &agreement,
                &wrong_schema,
                &view_key(),
            )
            .expect_err("unknown unsigned Stage-B schema"),
            XmrAgreementV1Error::UnsupportedActivationSchema(2)
        );
        let mut trailing = validated
            .encode_unsigned_wire()
            .expect("canonical unsigned Stage-B wire");
        trailing.push(0);
        assert!(matches!(
            ValidatedXmrActivationBodyV1::from_unsigned_wire(&agreement, &trailing, &view_key(),),
            Err(XmrAgreementV1Error::MalformedWire)
        ));
        assert_eq!(
            ValidatedXmrActivationBodyV1::from_unsigned_wire(
                &agreement,
                &vec![0; MAX_XMR_UNSIGNED_STAGE_B_WIRE_BYTES + 1],
                &view_key(),
            )
            .expect_err("oversized unsigned Stage-B wire"),
            XmrAgreementV1Error::OversizedActivationWire
        );
    }

    #[test]
    fn unsigned_stage_a_is_semantically_validated_before_role_signatures_are_attached() {
        let unsigned = body();
        let expected_record = signed_record(unsigned.clone());
        let expected_wire = expected_record.encode_wire().expect("signed Stage-A wire");
        let validated =
            ValidatedXmrAgreementBodyV1::validate(unsigned).expect("validated unsigned Stage A");
        let commitment = validated.commitment();

        assert_eq!(validated.body(), expected_record.body());
        assert_eq!(commitment, expected_record.body().commitment());
        let agreement = validated
            .attach_signatures(
                sign(MAKER_AGREEMENT_SECRET, commitment),
                sign(TAKER_AGREEMENT_SECRET, commitment),
            )
            .expect("role-correct countersignatures");
        assert_eq!(
            agreement.encode_wire().expect("canonical Stage A"),
            expected_wire
        );

        let mut invalid = body();
        invalid.monero.public_spend_key[0] ^= 1;
        assert_eq!(
            ValidatedXmrAgreementBodyV1::validate(invalid)
                .expect_err("derived-address mutation before signing"),
            XmrAgreementV1Error::MoneroAddressDerivationMismatch
        );

        let wrong =
            ValidatedXmrAgreementBodyV1::validate(body()).expect("validated unsigned Stage A");
        let commitment = wrong.commitment();
        assert_eq!(
            wrong
                .attach_signatures(
                    sign(MAKER_AGREEMENT_SECRET, [99; 32]),
                    sign(TAKER_AGREEMENT_SECRET, commitment),
                )
                .expect_err("wrong Maker agreement signature"),
            XmrAgreementV1Error::SignatureMismatch(XmrRoleV1::Maker)
        );

        let crossed =
            ValidatedXmrAgreementBodyV1::validate(body()).expect("validated unsigned Stage A");
        let commitment = crossed.commitment();
        assert_eq!(
            crossed
                .attach_signatures(
                    sign(TAKER_AGREEMENT_SECRET, commitment),
                    sign(MAKER_AGREEMENT_SECRET, commitment),
                )
                .expect_err("crossed agreement-role signatures"),
            XmrAgreementV1Error::SignatureMismatch(XmrRoleV1::Maker)
        );
    }

    #[test]
    fn unsigned_stage_b_requires_validated_stage_a_and_view_key_before_countersigning() {
        let stage_a =
            ValidatedXmrAgreementBodyV1::validate(body()).expect("validated unsigned Stage A");
        let stage_a_commitment = stage_a.commitment();
        let agreement = stage_a
            .attach_signatures(
                sign(MAKER_AGREEMENT_SECRET, stage_a_commitment),
                sign(TAKER_AGREEMENT_SECRET, stage_a_commitment),
            )
            .expect("validated Stage A");
        let (expected_record, _) = activation_record(&agreement);
        let expected_wire = expected_record.encode_wire().expect("signed Stage-B wire");
        let validated = ValidatedXmrActivationBodyV1::validate(
            &agreement,
            expected_record.body.clone(),
            &view_key(),
        )
        .expect("validated unsigned Stage B");
        let commitment = validated.commitment();

        assert_eq!(validated.body(), &expected_record.body);
        assert_eq!(commitment, expected_record.body.commitment());
        let activation = validated
            .attach_signatures(
                sign(MAKER_AGREEMENT_SECRET, commitment),
                sign(TAKER_AGREEMENT_SECRET, commitment),
            )
            .expect("role-correct activation countersignatures");
        assert_eq!(
            activation.encode_wire().expect("canonical Stage B"),
            expected_wire
        );

        let mut invalid = expected_record.body.clone();
        invalid.refund_presignature[0] ^= 1;
        assert_eq!(
            ValidatedXmrActivationBodyV1::validate(&agreement, invalid, &view_key())
                .expect_err("refund mutation before signing"),
            XmrAgreementV1Error::RefundPresignatureMismatch
        );

        let mut other_view_bytes = [0; 32];
        other_view_bytes[0] = 18;
        let other_view = MoneroPrivateViewKey::from_monero_little_endian(other_view_bytes)
            .expect("other private view key");
        assert_eq!(
            ValidatedXmrActivationBodyV1::validate(
                &agreement,
                expected_record.body.clone(),
                &other_view,
            )
            .expect_err("wrong local view key before signing"),
            XmrAgreementV1Error::LocalViewKeyMismatch
        );

        let mut other_body = body();
        other_body.swap_id[0] ^= 1;
        let other_stage_a =
            ValidatedXmrAgreementBodyV1::validate(other_body).expect("other validated Stage A");
        let other_commitment = other_stage_a.commitment();
        let other_agreement = other_stage_a
            .attach_signatures(
                sign(MAKER_AGREEMENT_SECRET, other_commitment),
                sign(TAKER_AGREEMENT_SECRET, other_commitment),
            )
            .expect("other countersigned Stage A");
        assert_eq!(
            ValidatedXmrActivationBodyV1::validate(
                &other_agreement,
                expected_record.body.clone(),
                &view_key(),
            )
            .expect_err("activation body crossed with another validated Stage A"),
            XmrAgreementV1Error::ActivationBindingMismatch
        );

        let wrong = ValidatedXmrActivationBodyV1::validate(
            &agreement,
            expected_record.body.clone(),
            &view_key(),
        )
        .expect("validated unsigned Stage B");
        let commitment = wrong.commitment();
        assert_eq!(
            wrong
                .attach_signatures(
                    sign(MAKER_AGREEMENT_SECRET, [99; 32]),
                    sign(TAKER_AGREEMENT_SECRET, commitment),
                )
                .expect_err("wrong Maker activation signature"),
            XmrAgreementV1Error::SignatureMismatch(XmrRoleV1::Maker)
        );

        let crossed =
            ValidatedXmrActivationBodyV1::validate(&agreement, expected_record.body, &view_key())
                .expect("validated unsigned Stage B");
        let commitment = crossed.commitment();
        assert_eq!(
            crossed
                .attach_signatures(
                    sign(TAKER_AGREEMENT_SECRET, commitment),
                    sign(MAKER_AGREEMENT_SECRET, commitment),
                )
                .expect_err("crossed activation-role signatures"),
            XmrAgreementV1Error::SignatureMismatch(XmrRoleV1::Maker)
        );
    }

    #[test]
    fn adaptor_session_descriptors_reconstruct_exact_contexts_and_fail_closed() {
        let agreement = XmrAgreementV1::validate(signed_record(body())).expect("agreement");
        let claim = agreement.claim_session_descriptor();
        let refund = agreement.refund_session_descriptor();

        assert_eq!(claim.purpose(), XmrAdaptorSessionPurposeV1::Claim);
        assert_eq!(refund.purpose(), XmrAdaptorSessionPurposeV1::Refund);
        assert_ne!(claim, refund);
        assert_ne!(claim.session_id(), refund.session_id());
        assert_ne!(claim.exact_message(), refund.exact_message());
        assert_ne!(claim.adaptor_point(), refund.adaptor_point());
        assert_ne!(claim.ordered_public_keys(), refund.ordered_public_keys());
        assert_ne!(claim.context_binding(), refund.context_binding());

        for (descriptor, retained) in [
            (&claim, &agreement.claim_context),
            (&refund, &agreement.refund_context),
        ] {
            let reconstructed = descriptor.context().expect("checked reconstruction");
            assert_eq!(descriptor.session_id(), retained.session_id());
            assert_eq!(descriptor.exact_message(), retained.message());
            assert_eq!(descriptor.adaptor_point(), retained.adaptor_point());
            assert_eq!(
                descriptor.ordered_public_keys(),
                retained.ordered_public_keys()
            );
            assert_eq!(
                descriptor.context_binding(),
                retained.durable_context_binding()
            );
            assert_eq!(reconstructed.session_id(), retained.session_id());
            assert_eq!(reconstructed.message(), retained.message());
            assert_eq!(reconstructed.adaptor_point(), retained.adaptor_point());
            assert_eq!(
                reconstructed.ordered_public_keys(),
                retained.ordered_public_keys()
            );
            assert_eq!(
                reconstructed.durable_context_binding(),
                retained.durable_context_binding()
            );
        }

        let mut wrong_purpose = claim;
        wrong_purpose.purpose = XmrAdaptorSessionPurposeV1::Refund;
        assert!(matches!(
            wrong_purpose.context(),
            Err(XmrAgreementV1Error::AdaptorSessionDescriptorMismatch)
        ));

        let mut changed_message = agreement.claim_session_descriptor();
        changed_message.exact_message[0] ^= 1;
        assert!(matches!(
            changed_message.context(),
            Err(XmrAgreementV1Error::AdaptorSessionDescriptorMismatch)
        ));

        let mut changed_session_id = agreement.claim_session_descriptor();
        changed_session_id.session_id[0] ^= 1;
        assert!(matches!(
            changed_session_id.context(),
            Err(XmrAgreementV1Error::AdaptorSessionDescriptorMismatch)
        ));

        let mut changed_adaptor_point = agreement.claim_session_descriptor();
        changed_adaptor_point.adaptor_point = refund.adaptor_point;
        assert!(matches!(
            changed_adaptor_point.context(),
            Err(XmrAgreementV1Error::AdaptorSessionDescriptorMismatch)
        ));

        let mut changed_key_order = agreement.claim_session_descriptor();
        changed_key_order.ordered_public_keys.swap(0, 1);
        assert!(matches!(
            changed_key_order.context(),
            Err(XmrAgreementV1Error::AdaptorSessionDescriptorMismatch)
        ));

        let mut changed_binding = agreement.claim_session_descriptor();
        changed_binding.context_binding[0] ^= 1;
        assert!(matches!(
            changed_binding.context(),
            Err(XmrAgreementV1Error::AdaptorSessionDescriptorMismatch)
        ));

        let mut crosswired = agreement.claim_session_descriptor();
        let refund = agreement.refund_session_descriptor();
        crosswired.session_id = refund.session_id;
        crosswired.exact_message = refund.exact_message;
        crosswired.adaptor_point = refund.adaptor_point;
        crosswired.ordered_public_keys = refund.ordered_public_keys;
        crosswired.context_binding = refund.context_binding;
        assert!(matches!(
            crosswired.context(),
            Err(XmrAgreementV1Error::AdaptorSessionDescriptorMismatch)
        ));
    }
    #[test]
    #[allow(clippy::too_many_lines)] // One canonical fixture proves Stage-B projection and LEZ boundaries.
    fn canonical_stage_b_activation_enables_lez_init_and_candidate_validation() {
        let record = signed_record(body());
        let wire = record.encode_wire().expect("bounded wire");
        let agreement = XmrAgreementV1::from_wire(&wire).expect("validated agreement");

        assert_eq!(agreement.encode_wire().expect("canonical wire"), wire);
        assert_eq!(
            agreement.claim_context.adaptor_point(),
            agreement.maker_proof().secp256k1_public_key()
        );
        assert_eq!(
            agreement.refund_context.adaptor_point(),
            agreement.taker_proof().secp256k1_public_key()
        );
        assert_eq!(
            agreement.claim_context.message(),
            agreement.body().messages().claim()
        );
        assert_eq!(
            agreement.refund_context.message(),
            agreement.body().messages().refund()
        );
        assert_ne!(
            agreement.claim_context.ordered_public_keys(),
            agreement.refund_context.ordered_public_keys()
        );
        assert_ne!(
            agreement.claim_context_binding(),
            agreement.refund_context_binding()
        );
        assert_eq!(
            agreement.claim_context.output_key(),
            agreement.body().lez().claim_aggregate_x_only_key()
        );
        assert_eq!(
            agreement.refund_context.output_key(),
            agreement.body().lez().refund_aggregate_x_only_key()
        );

        let (record, taker_claim_partial) = activation_record(&agreement);
        let activation_wire = record.encode_wire().expect("activation wire");
        let activation =
            XmrActivatedAgreementV1::from_wire(&agreement, &activation_wire, &view_key())
                .expect("validated Stage B");
        assert_eq!(
            activation.encode_wire().expect("canonical activation"),
            activation_wire
        );
        let initial = activation
            .initial_coordinator(&agreement)
            .expect("Stage-B application projection");
        assert_eq!(initial.id().as_str(), hex::encode([19; 32]));
        assert_eq!(initial.pair(), Pair::Monero);
        assert_eq!(initial.direction(), SwapDirection::TakerSellsLez);
        assert_eq!(
            initial.required_confirmations(lez_swap_core::Participant::Taker),
            LEZ_FINALITY_UNITS
        );
        assert_eq!(
            initial.required_confirmations(lez_swap_core::Participant::Maker),
            XMR_CONFIRMATIONS
        );
        assert_eq!(
            initial.recovery_schedule().deadline_for_chain(Chain::Lez),
            Some(ChainPosition::timestamp(Chain::Lez, 20)),
            "validated whole-second refund projects without weakening its boundary"
        );
        let mut crossed_body = body();
        crossed_body.swap_id[0] ^= 1;
        let crossed =
            XmrAgreementV1::validate(signed_record(crossed_body)).expect("crossed agreement");
        assert_eq!(
            activation
                .initial_coordinator(&crossed)
                .expect_err("Stage B crossed with another Stage A"),
            XmrAgreementV1Error::ActivationBindingMismatch
        );
        activation
            .verify_published_taker_claim_partial(&agreement, taker_claim_partial)
            .expect("exact guest publication");

        let plan = activation
            .lez_initialize_plan(&agreement)
            .expect("Stage-B-only init plan");
        assert_eq!(
            plan.activation_commitment(),
            activation.activation_commitment()
        );
        assert_eq!(
            plan.maker_xmr_funding_cutoff_ms(),
            agreement.body().windows().maker_xmr_funding_cutoff_ms()
        );
        assert_eq!(
            plan.claim_partial_context_binding(),
            activation.record.body.claim_partial_context_binding
        );
        assert_eq!(
            plan.claim_partial_commitment(),
            activation.record.body.claim_partial_commitment
        );

        let candidate = lez_lock_candidate(&activation, &agreement);
        let validated_candidate = activation
            .validate_lez_lock_candidate(&agreement, &candidate)
            .expect("structurally valid LEZ lock candidate");
        assert_eq!(
            validated_candidate.activation_commitment(),
            activation.activation_commitment()
        );
        assert_eq!(validated_candidate.funding_transaction_id(), [61; 32]);
        assert_eq!(validated_candidate.containing_block_hash(), [62; 32]);
        assert_eq!(
            validated_candidate.finalized_consensus_timestamp_ms(),
            agreement.body().windows().maker_xmr_funding_cutoff_ms()
        );
    }

    #[test]
    #[allow(clippy::too_many_lines)] // One table-like test covers exact Stage-A boundaries.
    fn agreement_mutations_fail_closed_at_their_exact_boundary() {
        let mut changed = body();
        changed.direction = XmrSwapDirectionV1::TakerSellsXmr;
        assert_eq!(
            XmrAgreementV1::validate(signed_record(changed)).expect_err("direction"),
            XmrAgreementV1Error::UnsupportedDirection
        );

        let mut changed = body();
        changed.participants.taker.claim_session_public_key =
            changed.participants.maker.agreement_public_key;
        assert_eq!(
            XmrAgreementV1::validate(signed_record(changed)).expect_err("key alias"),
            XmrAgreementV1Error::AliasedParticipantKeys
        );

        let mut changed = body();
        let mut parity_alias = changed.participants.maker.claim_session_public_key;
        parity_alias[0] = if parity_alias[0] == 2 { 3 } else { 2 };
        changed.participants.taker.claim_session_public_key = parity_alias;
        assert_eq!(
            XmrAgreementV1::validate(signed_record(changed)).expect_err("x-only parity alias"),
            XmrAgreementV1Error::AliasedParticipantKeys
        );

        let mut changed = body();
        changed.participants.maker.claim_session_public_key = proofs().maker_secp_public;
        assert_eq!(
            XmrAgreementV1::validate(signed_record(changed)).expect_err("DLEQ key reuse"),
            XmrAgreementV1Error::DleqSigningKeyReuse
        );

        let mut changed = body();
        changed.lez.maker_dleq_transcript_commitment[0] ^= 1;
        assert_eq!(
            XmrAgreementV1::validate(signed_record(changed)).expect_err("DLEQ commitment"),
            XmrAgreementV1Error::LezDleqCommitmentMismatch
        );

        let mut changed = body();
        changed.monero.required_confirmations -= 1;
        assert_eq!(
            XmrAgreementV1::validate(signed_record(changed)).expect_err("named confirmations"),
            XmrAgreementV1Error::InvalidMoneroPolicy
        );

        let mut changed = body();
        changed.profile = XmrNamedProfileV1::PublicStagenet;
        assert_eq!(
            XmrAgreementV1::validate(signed_record(changed)).expect_err("named network"),
            XmrAgreementV1Error::InvalidMoneroPolicy
        );

        let mut changed = body();
        changed.lez.required_finality_units -= 1;
        assert_eq!(
            XmrAgreementV1::validate(signed_record(changed)).expect_err("named finality"),
            XmrAgreementV1Error::InvalidLezTerms
        );

        let mut changed = body();
        changed.lez.depositor_account = [23; 32];
        assert_eq!(
            XmrAgreementV1::validate(signed_record(changed)).expect_err("role mapping"),
            XmrAgreementV1Error::LezRoleMismatch
        );

        let mut changed = body();
        changed.messages.refund = changed.messages.claim;
        assert_eq!(
            XmrAgreementV1::validate(signed_record(changed)).expect_err("messages"),
            XmrAgreementV1Error::InvalidMessages
        );

        let mut changed = body();
        changed.windows.punish_at_ms = changed.windows.refund_at_ms;
        assert_eq!(
            XmrAgreementV1::validate(signed_record(changed)).expect_err("windows"),
            XmrAgreementV1Error::InvalidWindows
        );

        let mut changed = body();
        changed.monero.public_spend_key[0] ^= 1;
        assert_eq!(
            XmrAgreementV1::validate(signed_record(changed)).expect_err("address derivation"),
            XmrAgreementV1Error::MoneroAddressDerivationMismatch
        );

        let mut changed = body();
        changed.lez.claim_aggregate_x_only_key[0] ^= 1;
        assert_eq!(
            XmrAgreementV1::validate(signed_record(changed)).expect_err("claim authority"),
            XmrAgreementV1Error::LezAuthorityMismatch
        );

        let valid = signed_record(body());
        let mut oversized = body();
        oversized.monero.address = "x".repeat(MAX_MONERO_ADDRESS_BYTES + 1);
        assert_eq!(
            signed_record(oversized)
                .encode_wire()
                .expect_err("oversized address"),
            XmrAgreementV1Error::OversizedField
        );

        let mut bad_signature = valid.clone();
        bad_signature.maker_signature[0] ^= 1;
        assert_eq!(
            XmrAgreementV1::validate(bad_signature).expect_err("signature"),
            XmrAgreementV1Error::SignatureMismatch(XmrRoleV1::Maker)
        );

        let mut trailing = valid.encode_wire().expect("valid wire");
        trailing.push(0);
        assert_eq!(
            XmrAgreementV1::from_wire(&trailing).expect_err("trailing byte"),
            XmrAgreementV1Error::MalformedWire
        );
    }

    #[test]
    fn stage_b_activation_and_private_view_key_mutations_fail_closed() {
        let agreement = XmrAgreementV1::validate(signed_record(body())).expect("agreement");
        let (valid, _) = activation_record(&agreement);

        let mut other_view_bytes = [0; 32];
        other_view_bytes[0] = 18;
        let other_view = MoneroPrivateViewKey::from_monero_little_endian(other_view_bytes)
            .expect("other private view key");
        assert_eq!(
            XmrActivatedAgreementV1::validate(&agreement, valid.clone(), &other_view)
                .expect_err("view-key possession"),
            XmrAgreementV1Error::LocalViewKeyMismatch
        );

        let mut changed = valid.clone();
        changed.body.base_agreement_commitment[0] ^= 1;
        resign_activation(&mut changed);
        assert_eq!(
            XmrActivatedAgreementV1::validate(&agreement, changed, &view_key())
                .expect_err("base binding"),
            XmrAgreementV1Error::ActivationBindingMismatch
        );

        let mut changed = valid.clone();
        changed.body.refund_presignature[0] ^= 1;
        resign_activation(&mut changed);
        assert_eq!(
            XmrActivatedAgreementV1::validate(&agreement, changed, &view_key())
                .expect_err("refund presignature"),
            XmrAgreementV1Error::RefundPresignatureMismatch
        );

        let mut changed = valid.clone();
        changed.maker_signature[0] ^= 1;
        assert_eq!(
            XmrActivatedAgreementV1::validate(&agreement, changed, &view_key())
                .expect_err("activation signature"),
            XmrAgreementV1Error::SignatureMismatch(XmrRoleV1::Maker)
        );

        let mut trailing = valid.encode_wire().expect("activation wire");
        trailing.push(0);
        assert_eq!(
            XmrActivatedAgreementV1::from_wire(&agreement, &trailing, &view_key())
                .expect_err("trailing activation byte"),
            XmrAgreementV1Error::MalformedWire
        );
    }

    #[test]
    fn lez_lock_candidate_checks_exact_terms_finality_and_funding_cutoff() {
        let agreement = XmrAgreementV1::validate(signed_record(body())).expect("agreement");
        let (record, taker_partial) = activation_record(&agreement);
        let activation = XmrActivatedAgreementV1::validate(&agreement, record.clone(), &view_key())
            .expect("activation");
        let valid = lez_lock_candidate(&activation, &agreement);

        activation
            .validate_lez_lock_candidate(&agreement, &valid)
            .expect("funding cutoff is inclusive");

        let mut changed = valid.clone();
        changed.finality_units -= 1;
        assert_eq!(
            activation
                .validate_lez_lock_candidate(&agreement, &changed)
                .expect_err("LEZ finality"),
            XmrAgreementV1Error::InsufficientLezCandidateFinality {
                actual: 1,
                required: 2,
            }
        );

        let mut changed = valid.clone();
        changed.status = XmrLezLockStatusV1::Closed;
        assert_eq!(
            activation
                .validate_lez_lock_candidate(&agreement, &changed)
                .expect_err("LEZ status"),
            XmrAgreementV1Error::LezLockCandidateMismatch
        );

        let mut changed = valid.clone();
        changed.plan.activation_commitment[0] ^= 1;
        assert_eq!(
            activation
                .validate_lez_lock_candidate(&agreement, &changed)
                .expect_err("exact metadata"),
            XmrAgreementV1Error::LezLockCandidateMismatch
        );

        let mut changed = valid.clone();
        changed.finalized_consensus_timestamp_ms += 1;
        assert_eq!(
            activation
                .validate_lez_lock_candidate(&agreement, &changed)
                .expect_err("Maker funding cutoff"),
            XmrAgreementV1Error::LezLockCandidateAfterFundingCutoff {
                finalized_consensus_timestamp_ms: 10_001,
                cutoff_ms: 10_000,
            }
        );

        let mut changed = valid;
        changed.funding_transaction_id = [0; 32];
        assert_eq!(
            activation
                .validate_lez_lock_candidate(&agreement, &changed)
                .expect_err("candidate identifier"),
            XmrAgreementV1Error::LezLockCandidateMismatch
        );

        let mut committed_to_other_partial = record;
        committed_to_other_partial.body.claim_partial_commitment[0] ^= 1;
        resign_activation(&mut committed_to_other_partial);
        let changed_activation =
            XmrActivatedAgreementV1::validate(&agreement, committed_to_other_partial, &view_key())
                .expect("hidden commitment is countersigned");
        assert_eq!(
            changed_activation
                .verify_published_taker_claim_partial(&agreement, taker_partial)
                .expect_err("published claim partial"),
            XmrAgreementV1Error::PublishedClaimPartialMismatch
        );
    }
}
