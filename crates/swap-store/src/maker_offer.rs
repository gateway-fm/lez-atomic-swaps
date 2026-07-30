//! Durable maker offers and one-winner acceptance transitions.

use lez_bridge_protocol::RequestId;
use lez_btc_swap_sdk::{BtcAgreementV1, BtcMakerAgreementProposalV1};
use lez_swap_core::{Pair, Participant, Phase, SwapCoordinator, SwapDirection, SwapId};
use lez_xmr_swap_sdk::{
    MAX_XMR_ACTIVATION_WIRE_BYTES, MAX_XMR_AGREEMENT_WIRE_BYTES, XmrActivatedAgreementV1,
    XmrAgreementV1, XmrRoleV1, XmrSwapDirectionV1,
};
use rusqlite::{Connection, OptionalExtension as _, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use thiserror::Error;

use super::{
    BtcAgreementAcceptance, LocalPriceV1, MakerActorKindV1, MakerActorManifestV1,
    MakerPairConfigurationV1, MakerPriceSourceKind, MakerRouteV1, SWAP_PAYLOAD_VERSION,
    SqliteSwapStore, StoreError,
    maker_actor_process::{
        load_maker_actor_manifest_in_transaction, register_maker_actor_in_transaction,
        require_exact_maker_actor_in_transaction,
    },
};

const OFFER_PAYLOAD_VERSION: i64 = 1;
const MAX_LOGOS_QUOTE_AGE_SECONDS: u64 = 3_600;
const MAXIMUM_ZEC_PROPOSAL_BYTES: usize = 16 * 1024;
const MAXIMUM_BTC_PROPOSAL_BYTES: usize = 16 * 1024;
const MAXIMUM_XMR_STAGE_A_BYTES: usize = MAX_XMR_AGREEMENT_WIRE_BYTES;
const MAXIMUM_XMR_STAGE_B_BYTES: usize = MAX_XMR_ACTIVATION_WIRE_BYTES;
const ZEC_CHAT_SESSION_DOMAIN: &[u8] = b"lez-atomic-swaps/maker-zec-chat-session/v1";
const BTC_CHAT_SWAP_ID_DOMAIN: &[u8] = b"lez-atomic-swaps/maker-btc-chat-swap-id/v1";
const XMR_CHAT_SWAP_ID_DOMAIN: &[u8] = b"lez-atomic-swaps/maker-xmr-chat-swap-id/v1";

/// Invalid immutable maker-offer input.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum MakerOfferError {
    /// Offer identity was not bounded log-safe ASCII.
    #[error("offer ID must be 8..=64 safe ASCII bytes")]
    InvalidIdentifier,
    /// Trusted publication time or derived expiry was invalid or oversized.
    #[error("offer publication time or expiry is invalid")]
    InvalidTime,
    /// Price, policy, route, or revision snapshots were inconsistent.
    #[error("offer snapshot is internally inconsistent")]
    InvalidSnapshot,
    /// Selected foreign amount fell outside the signed inclusive offer bounds.
    #[error("selected foreign amount is outside the offer bounds")]
    AmountOutOfBounds,
    /// Exact integer-lot price could not represent the selected amount without rounding.
    #[error("selected amount is not exactly representable by the offer price")]
    NonIntegralPrice,
    /// Durable pair negotiation metadata was empty, oversized, aliased, or inconsistent.
    #[error("maker negotiation metadata is invalid")]
    InvalidNegotiation,
}

/// Bounded log-safe durable offer identity.
#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(try_from = "String", into = "String")]
pub struct MakerOfferId(String);

impl MakerOfferId {
    /// Validates and constructs an offer identifier.
    ///
    /// # Errors
    ///
    /// Rejects values outside 8..=64 bytes or the safe ASCII grammar.
    pub fn new(value: impl Into<String>) -> Result<Self, MakerOfferError> {
        let value = value.into();
        if (8..=64).contains(&value.len())
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        {
            Ok(Self(value))
        } else {
            Err(MakerOfferError::InvalidIdentifier)
        }
    }

    /// Borrows the validated identifier.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for MakerOfferId {
    type Error = MakerOfferError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<MakerOfferId> for String {
    fn from(value: MakerOfferId) -> Self {
        value.0
    }
}

/// Effective offer lifecycle state returned to operator and discovery clients.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MakerOfferStatus {
    /// Published, unexpired, and eligible for exactly one reservation.
    Active,
    /// Never reserved and no longer discoverable at the caller's trusted time.
    Expired,
    /// Accepted by one negotiation identity but not yet bound to a swap.
    Reserved,
    /// Bound to one durable swap identity.
    Consumed,
    /// Explicitly removed before reservation.
    Withdrawn,
}

/// Immutable offer terms snapshotted from one enabled route and exact price.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct MakerOfferV1 {
    id: MakerOfferId,
    pair_configuration: MakerPairConfigurationV1,
    price: LocalPriceV1,
    pair_configuration_revision: u64,
    price_source_revision: u64,
    price_observed_at_unix_seconds: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    price_source_identity_sha256: Option<[u8; 32]>,
    created_at_unix_seconds: u64,
    expires_at_unix_seconds: u64,
}

impl MakerOfferV1 {
    /// Durable offer identity.
    #[must_use]
    pub const fn id(&self) -> &MakerOfferId {
        &self.id
    }

    /// Exact pair and direction.
    #[must_use]
    pub const fn route(&self) -> MakerRouteV1 {
        self.pair_configuration.route()
    }

    /// Complete route policy snapshot used for publication.
    #[must_use]
    pub const fn pair_configuration(&self) -> &MakerPairConfigurationV1 {
        &self.pair_configuration
    }

    /// Inclusive smallest foreign atomic-unit amount.
    #[must_use]
    pub const fn minimum_foreign_units(&self) -> u64 {
        self.pair_configuration.minimum_foreign_units()
    }

    /// Inclusive largest foreign atomic-unit amount.
    #[must_use]
    pub const fn maximum_foreign_units(&self) -> u64 {
        self.pair_configuration.maximum_foreign_units()
    }

    /// Exact reduced-integer price snapshot.
    #[must_use]
    pub const fn price(&self) -> &LocalPriceV1 {
        &self.price
    }

    /// Route-policy revision used to publish this offer.
    #[must_use]
    pub const fn pair_configuration_revision(&self) -> u64 {
        self.pair_configuration_revision
    }

    /// Price-source revision used to publish this offer.
    #[must_use]
    pub const fn price_source_revision(&self) -> u64 {
        self.price_source_revision
    }

    /// Pinned external module identity, absent only for a local price.
    #[must_use]
    pub const fn price_source_identity_sha256(&self) -> Option<[u8; 32]> {
        self.price_source_identity_sha256
    }

    /// Trusted time at which the selected source was observed.
    #[must_use]
    pub const fn price_observed_at_unix_seconds(&self) -> u64 {
        self.price_observed_at_unix_seconds
    }

    /// Trusted daemon publication time.
    #[must_use]
    pub const fn created_at_unix_seconds(&self) -> u64 {
        self.created_at_unix_seconds
    }

    /// Exclusive trusted-time discovery/reservation boundary.
    #[must_use]
    pub const fn expires_at_unix_seconds(&self) -> u64 {
        self.expires_at_unix_seconds
    }

    /// Converts one selected foreign amount through the exact signed integer-lot price.
    ///
    /// # Errors
    ///
    /// Rejects amounts outside the inclusive offer bounds or any result that
    /// would require fractional LEZ atomic units. No rounding is performed.
    pub fn quote_foreign_amount(&self, foreign_units: u64) -> Result<u128, MakerOfferError> {
        if foreign_units < self.minimum_foreign_units()
            || foreign_units > self.maximum_foreign_units()
        {
            return Err(MakerOfferError::AmountOutOfBounds);
        }
        let numerator = u128::from(foreign_units)
            .checked_mul(u128::from(self.price.lez_units_per_lot()))
            .ok_or(MakerOfferError::InvalidSnapshot)?;
        let denominator = u128::from(self.price.foreign_units_per_lot());
        if numerator % denominator != 0 {
            return Err(MakerOfferError::NonIntegralPrice);
        }
        Ok(numerator / denominator)
    }

    /// Revalidates a deserialized offer snapshot at an untrusted boundary.
    ///
    /// # Errors
    ///
    /// Rejects inconsistent identity, route, policy, price, revision, or time fields.
    pub fn validate(&self) -> Result<(), MakerOfferError> {
        MakerOfferId::new(self.id.as_str())?;
        let validated_policy = MakerPairConfigurationV1::new(
            self.pair_configuration.route(),
            self.pair_configuration.enabled(),
            self.pair_configuration.price_source(),
            self.pair_configuration.minimum_foreign_units(),
            self.pair_configuration.maximum_foreign_units(),
            self.pair_configuration.offer_ttl_seconds(),
        )
        .map_err(|_| MakerOfferError::InvalidSnapshot)?;
        let valid_observation_time = match self.pair_configuration.price_source() {
            MakerPriceSourceKind::Local => {
                self.price_source_identity_sha256.is_none()
                    && self.price_observed_at_unix_seconds == self.created_at_unix_seconds
            }
            MakerPriceSourceKind::LogosCApi => {
                self.price_source_identity_sha256
                    .is_some_and(|identity| identity != [0; 32])
                    && self.price_observed_at_unix_seconds > 0
                    && self.price_observed_at_unix_seconds <= self.created_at_unix_seconds
                    && self.created_at_unix_seconds - self.price_observed_at_unix_seconds
                        <= MAX_LOGOS_QUOTE_AGE_SECONDS
            }
        };
        if validated_policy != self.pair_configuration
            || !self.pair_configuration.enabled()
            || self.price.route() != self.route()
            || self.pair_configuration_revision == 0
            || self.price_source_revision == 0
            || !valid_observation_time
        {
            return Err(MakerOfferError::InvalidSnapshot);
        }
        if self.created_at_unix_seconds >= self.expires_at_unix_seconds
            || self.expires_at_unix_seconds > i64::MAX as u64
            || self.expires_at_unix_seconds - self.created_at_unix_seconds
                != self.pair_configuration.offer_ttl_seconds()
        {
            return Err(MakerOfferError::InvalidTime);
        }
        LocalPriceV1::new(
            self.price.route(),
            self.price.lez_units_per_lot(),
            self.price.foreign_units_per_lot(),
        )
        .map_err(|_| MakerOfferError::InvalidSnapshot)?;
        Ok(())
    }
}

/// Durable offer view with its one-winner state and revision.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct MakerOfferRecordV1 {
    revision: u64,
    status: MakerOfferStatus,
    offer: MakerOfferV1,
    reservation_id: Option<RequestId>,
    swap_id: Option<Box<str>>,
}

impl MakerOfferRecordV1 {
    /// Monotonic offer-local transition revision.
    #[must_use]
    pub const fn revision(&self) -> u64 {
        self.revision
    }

    /// Effective state at the trusted time supplied to the read.
    #[must_use]
    pub const fn status(&self) -> MakerOfferStatus {
        self.status
    }

    /// Immutable published terms.
    #[must_use]
    pub const fn offer(&self) -> &MakerOfferV1 {
        &self.offer
    }

    /// Winning negotiation identity, if accepted.
    #[must_use]
    pub const fn reservation_id(&self) -> Option<&RequestId> {
        self.reservation_id.as_ref()
    }

    /// Swap identity bound after negotiation, if consumed.
    #[must_use]
    pub fn swap_id(&self) -> Option<&str> {
        self.swap_id.as_deref()
    }
}

/// Result of one atomic offer mutation.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct MakerOfferCommit {
    revision: u64,
    was_replay: bool,
}

impl MakerOfferCommit {
    /// Durable offer-local revision.
    #[must_use]
    pub const fn revision(self) -> u64 {
        self.revision
    }

    /// Whether the exact request and result were already committed.
    #[must_use]
    pub const fn was_replay(self) -> bool {
        self.was_replay
    }
}

/// Result of one atomic XMR activation, coordinator, and actor commit.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct MakerXmrAcceptanceCommit {
    offer_revision: u64,
    was_replay: bool,
}

impl MakerXmrAcceptanceCommit {
    /// Durable offer revision after activation.
    #[must_use]
    pub const fn offer_revision(self) -> u64 {
        self.offer_revision
    }

    /// Whether the exact globally idempotent completion already committed.
    #[must_use]
    pub const fn was_replay(self) -> bool {
        self.was_replay
    }
}

/// Result of one atomic Bitcoin negotiation, coordinator, and actor commit.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct MakerBtcAcceptanceCommit {
    offer_revision: u64,
    was_replay: bool,
}

impl MakerBtcAcceptanceCommit {
    /// Durable offer revision after acceptance.
    #[must_use]
    pub const fn offer_revision(self) -> u64 {
        self.offer_revision
    }

    /// Whether the exact globally idempotent completion already committed.
    #[must_use]
    pub const fn was_replay(self) -> bool {
        self.was_replay
    }
}

/// Exact durable Bitcoin completion recovered before filesystem provisioning.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MakerBtcAcceptanceReplay {
    offer_revision: u64,
    swap_id: SwapId,
    actor: MakerActorManifestV1,
}

impl MakerBtcAcceptanceReplay {
    /// Durable consumed offer revision.
    #[must_use]
    pub const fn offer_revision(&self) -> u64 {
        self.offer_revision
    }

    /// Agreement-derived application swap identity.
    #[must_use]
    pub const fn swap_id(&self) -> &SwapId {
        &self.swap_id
    }

    /// Exact immutable actor manifest already committed with acceptance.
    #[must_use]
    pub const fn actor(&self) -> &MakerActorManifestV1 {
        &self.actor
    }
}

/// Read-before-effect result for one idempotent offer publication.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MakerOfferPublicationPreflight {
    /// The exact caller request already committed; no price source may run.
    Replayed(MakerOfferCommit),
    /// A fresh quote may be fetched for this exact policy snapshot.
    Pending {
        /// Route-policy revision the final transaction must still observe.
        pair_configuration_revision: u64,
        /// Price source selected by that policy revision.
        price_source: MakerPriceSourceKind,
    },
}

impl MakerOfferPublicationPreflight {
    /// Returns the durable replay result, if publication already committed.
    #[must_use]
    pub const fn replayed(self) -> Option<MakerOfferCommit> {
        match self {
            Self::Replayed(commit) => Some(commit),
            Self::Pending { .. } => None,
        }
    }
}

/// Durable pre-lock Chat negotiation phase.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MakerZecNegotiationStatus {
    /// Maker proposal is durable and awaits the exact taker countersignature.
    Proposed,
    /// Final countersigned agreement and initial coordinator are durable.
    Completed,
}

/// Exact durable ZEC proposal state bound to one reserved offer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MakerZecNegotiationV1 {
    reservation_id: RequestId,
    offer_commitment: [u8; 32],
    maker_chat_identity: [u8; 33],
    taker_chat_identity: [u8; 33],
    foreign_units: u64,
    lez_units: u128,
    reserved_at_unix_seconds: u64,
    agreement_commitment: [u8; 32],
    maker_proposal_wire: Vec<u8>,
    status: MakerZecNegotiationStatus,
    final_agreement_wire: Option<Vec<u8>>,
    swap_id: Option<Box<str>>,
}

impl MakerZecNegotiationV1 {
    /// Constructs one bounded proposal staged after all Chat and SDK validation.
    ///
    /// # Errors
    ///
    /// Rejects empty/aliased identities, empty commitments, zero amounts,
    /// invalid time, or empty/oversized proposal bytes.
    #[allow(clippy::too_many_arguments)]
    pub fn proposed(
        reservation_id: RequestId,
        offer_commitment: [u8; 32],
        maker_chat_identity: [u8; 33],
        taker_chat_identity: [u8; 33],
        foreign_units: u64,
        lez_units: u128,
        reserved_at_unix_seconds: u64,
        agreement_commitment: [u8; 32],
        maker_proposal_wire: Vec<u8>,
    ) -> Result<Self, MakerOfferError> {
        let value = Self {
            reservation_id,
            offer_commitment,
            maker_chat_identity,
            taker_chat_identity,
            foreign_units,
            lez_units,
            reserved_at_unix_seconds,
            agreement_commitment,
            maker_proposal_wire,
            status: MakerZecNegotiationStatus::Proposed,
            final_agreement_wire: None,
            swap_id: None,
        };
        value.validate()?;
        Ok(value)
    }

    /// Stable reservation/session identity.
    pub const fn reservation_id(&self) -> &RequestId {
        &self.reservation_id
    }

    /// Exact signed Delivery envelope commitment.
    #[must_use]
    pub const fn offer_commitment(&self) -> &[u8; 32] {
        &self.offer_commitment
    }

    /// Authenticated maker Chat identity linked to the Delivery publisher.
    #[must_use]
    pub const fn maker_chat_identity(&self) -> &[u8; 33] {
        &self.maker_chat_identity
    }

    /// Authenticated taker Chat identity.
    #[must_use]
    pub const fn taker_chat_identity(&self) -> &[u8; 33] {
        &self.taker_chat_identity
    }

    /// Exact selected foreign-chain atomic units.
    #[must_use]
    pub const fn foreign_units(&self) -> u64 {
        self.foreign_units
    }

    /// Exact no-rounding LEZ atomic units.
    #[must_use]
    pub const fn lez_units(&self) -> u128 {
        self.lez_units
    }

    /// Trusted maker reservation time.
    #[must_use]
    pub const fn reserved_at_unix_seconds(&self) -> u64 {
        self.reserved_at_unix_seconds
    }

    /// Canonical pair-agreement body commitment signed by the maker.
    #[must_use]
    pub const fn agreement_commitment(&self) -> &[u8; 32] {
        &self.agreement_commitment
    }

    /// Exact bounded maker-proposal wire sent to the taker.
    #[must_use]
    pub fn maker_proposal_wire(&self) -> &[u8] {
        &self.maker_proposal_wire
    }

    /// Durable negotiation phase.
    #[must_use]
    pub const fn status(&self) -> MakerZecNegotiationStatus {
        self.status
    }

    /// Exact final countersigned wire after completion.
    #[must_use]
    pub fn final_agreement_wire(&self) -> Option<&[u8]> {
        self.final_agreement_wire.as_deref()
    }

    /// Application swap identity after completion.
    #[must_use]
    pub fn swap_id(&self) -> Option<&str> {
        self.swap_id.as_deref()
    }

    fn validate(&self) -> Result<(), MakerOfferError> {
        if self.offer_commitment == [0; 32]
            || self.agreement_commitment == [0; 32]
            || self.maker_chat_identity == [0; 33]
            || self.taker_chat_identity == [0; 33]
            || self.maker_chat_identity == self.taker_chat_identity
            || self.foreign_units == 0
            || self.lez_units == 0
            || self.reserved_at_unix_seconds == 0
            || self.reserved_at_unix_seconds > i64::MAX as u64
            || self.maker_proposal_wire.is_empty()
            || self.maker_proposal_wire.len() > MAXIMUM_ZEC_PROPOSAL_BYTES
            || self
                .final_agreement_wire
                .as_ref()
                .is_some_and(|wire| wire.len() > MAXIMUM_ZEC_PROPOSAL_BYTES)
        {
            return Err(MakerOfferError::InvalidNegotiation);
        }
        match self.status {
            MakerZecNegotiationStatus::Proposed
                if self.final_agreement_wire.is_none() && self.swap_id.is_none() => {}
            MakerZecNegotiationStatus::Completed
                if self
                    .final_agreement_wire
                    .as_ref()
                    .is_some_and(|wire| !wire.is_empty())
                    && self.swap_id.is_some() => {}
            _ => return Err(MakerOfferError::InvalidNegotiation),
        }
        Ok(())
    }
}

/// Validated Maker-side XMR Stage-B acceptance bound to one coordinator.
///
/// Debug output deliberately excludes the activation wire.
#[derive(Clone, Eq, PartialEq)]
pub struct MakerXmrActivationAcceptance {
    local_role: Participant,
    agreement_commitment: [u8; 32],
    activation_wire: Vec<u8>,
    activation_commitment: [u8; 32],
    initial_snapshot_digest: [u8; 32],
    accepted_at_unix_seconds: u64,
}

impl std::fmt::Debug for MakerXmrActivationAcceptance {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MakerXmrActivationAcceptance")
            .field("local_role", &self.local_role)
            .field("agreement_commitment", &self.agreement_commitment)
            .field("activation_wire", &"<redacted>")
            .field("activation_commitment", &self.activation_commitment)
            .field("initial_snapshot_digest", &self.initial_snapshot_digest)
            .field("accepted_at_unix_seconds", &self.accepted_at_unix_seconds)
            .finish()
    }
}

impl MakerXmrActivationAcceptance {
    /// Binds canonical Stage B to its exact Stage A and SDK-derived coordinator.
    ///
    /// # Errors
    ///
    /// Rejects a wrong role, wire, base agreement, coordinator, or trusted time.
    pub fn new(
        initial: &SwapCoordinator,
        local_role: Participant,
        agreement: &XmrAgreementV1,
        activation: &XmrActivatedAgreementV1,
        activation_wire: Vec<u8>,
        accepted_at_unix_seconds: u64,
    ) -> Result<Self, MakerOfferError> {
        let canonical_wire = activation
            .encode_wire()
            .map_err(|_| MakerOfferError::InvalidNegotiation)?;
        let expected_initial = activation
            .initial_coordinator(agreement)
            .map_err(|_| MakerOfferError::InvalidNegotiation)?;
        let initial_json =
            serde_json::to_vec(initial).map_err(|_| MakerOfferError::InvalidNegotiation)?;
        let accepted_at_ms = accepted_at_unix_seconds
            .checked_mul(1_000)
            .ok_or(MakerOfferError::InvalidNegotiation)?;
        if local_role != Participant::Maker
            || activation_wire.is_empty()
            || activation_wire.len() > MAXIMUM_XMR_STAGE_B_BYTES
            || activation_wire != canonical_wire
            || activation.body().base_agreement_commitment() != agreement.agreement_commitment()
            || &expected_initial != initial
            || accepted_at_unix_seconds == 0
            || accepted_at_unix_seconds > i64::MAX as u64
            || accepted_at_ms > agreement.body().windows().maker_xmr_funding_cutoff_ms()
        {
            return Err(MakerOfferError::InvalidNegotiation);
        }
        Ok(Self {
            local_role,
            agreement_commitment: agreement.agreement_commitment(),
            activation_wire,
            activation_commitment: activation.activation_commitment(),
            initial_snapshot_digest: Sha256::digest(initial_json).into(),
            accepted_at_unix_seconds,
        })
    }

    const fn local_role(&self) -> Participant {
        self.local_role
    }

    const fn agreement_commitment(&self) -> [u8; 32] {
        self.agreement_commitment
    }

    fn activation_wire(&self) -> &[u8] {
        &self.activation_wire
    }

    const fn activation_commitment(&self) -> [u8; 32] {
        self.activation_commitment
    }

    const fn initial_snapshot_digest(&self) -> [u8; 32] {
        self.initial_snapshot_digest
    }

    const fn accepted_at_unix_seconds(&self) -> u64 {
        self.accepted_at_unix_seconds
    }
}

/// Durable XMR negotiation phase persisted before executable Stage B.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MakerXmrNegotiationStatus {
    /// The exact dual-signed Stage-A agreement reserved the offer.
    StageAAccepted,
    /// Stage B activated the agreement and atomically scheduled execution.
    Activated,
}

/// Exact durable, non-executable XMR Stage-A agreement bound to one offer.
///
/// Debug output deliberately excludes the potentially large agreement wire.
#[derive(Clone, Eq, PartialEq)]
pub struct MakerXmrNegotiationV1 {
    reservation_id: RequestId,
    offer_commitment: [u8; 32],
    foreign_units: u64,
    lez_units: u128,
    reserved_at_unix_seconds: u64,
    stage_a_wire: Vec<u8>,
    activation_wire: Option<Vec<u8>>,
    activation_commitment: Option<[u8; 32]>,
    activated_at_unix_seconds: Option<u64>,
    coordinator_swap_id: Option<Box<str>>,
    status: MakerXmrNegotiationStatus,
}

impl std::fmt::Debug for MakerXmrNegotiationV1 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MakerXmrNegotiationV1")
            .field("reservation_id", &self.reservation_id)
            .field("offer_commitment", &self.offer_commitment)
            .field("foreign_units", &self.foreign_units)
            .field("lez_units", &self.lez_units)
            .field("reserved_at_unix_seconds", &self.reserved_at_unix_seconds)
            .field("stage_a_wire", &"<redacted>")
            .field(
                "activation_wire",
                &self.activation_wire.as_ref().map(|_| "<redacted>"),
            )
            .field("activation_commitment", &self.activation_commitment)
            .field("activated_at_unix_seconds", &self.activated_at_unix_seconds)
            .field("coordinator_swap_id", &self.coordinator_swap_id)
            .field("status", &self.status)
            .finish()
    }
}

impl MakerXmrNegotiationV1 {
    /// Constructs one bounded untrusted Stage-A candidate.
    ///
    /// Semantic agreement validation is deferred to the transactional store
    /// boundary so malformed peer input can be rejected with zero durable write.
    ///
    /// # Errors
    ///
    /// Rejects invalid metadata or an empty/oversized wire.
    pub fn stage_a(
        reservation_id: RequestId,
        offer_commitment: [u8; 32],
        foreign_units: u64,
        lez_units: u128,
        reserved_at_unix_seconds: u64,
        stage_a_wire: Vec<u8>,
    ) -> Result<Self, MakerOfferError> {
        let value = Self {
            reservation_id,
            offer_commitment,
            foreign_units,
            lez_units,
            reserved_at_unix_seconds,
            stage_a_wire,
            activation_wire: None,
            activation_commitment: None,
            activated_at_unix_seconds: None,
            coordinator_swap_id: None,
            status: MakerXmrNegotiationStatus::StageAAccepted,
        };
        value.validate_metadata()?;
        Ok(value)
    }

    /// Winning Delivery/Chat reservation identity.
    pub const fn reservation_id(&self) -> &RequestId {
        &self.reservation_id
    }

    /// Exact authenticated Delivery envelope commitment.
    #[must_use]
    pub const fn offer_commitment(&self) -> &[u8; 32] {
        &self.offer_commitment
    }

    /// Selected Monero amount in piconero.
    #[must_use]
    pub const fn foreign_units(&self) -> u64 {
        self.foreign_units
    }

    /// Selected LEZ atomic-unit amount.
    #[must_use]
    pub const fn lez_units(&self) -> u128 {
        self.lez_units
    }

    /// Trusted reservation time.
    #[must_use]
    pub const fn reserved_at_unix_seconds(&self) -> u64 {
        self.reserved_at_unix_seconds
    }

    /// Exact canonical dual-signed Stage-A wire.
    #[must_use]
    pub fn stage_a_wire(&self) -> &[u8] {
        &self.stage_a_wire
    }

    /// Exact canonical dual-signed Stage-B activation wire, after completion.
    #[must_use]
    pub fn activation_wire(&self) -> Option<&[u8]> {
        self.activation_wire.as_deref()
    }

    /// Exact Stage-B activation commitment, after completion.
    #[must_use]
    pub const fn activation_commitment(&self) -> Option<[u8; 32]> {
        self.activation_commitment
    }

    /// Trusted Stage-B acceptance time, after completion.
    #[must_use]
    pub const fn activated_at_unix_seconds(&self) -> Option<u64> {
        self.activated_at_unix_seconds
    }

    /// Application coordinator identity, after completion.
    #[must_use]
    pub fn coordinator_swap_id(&self) -> Option<&str> {
        self.coordinator_swap_id.as_deref()
    }

    /// Durable XMR negotiation phase.
    #[must_use]
    pub const fn status(&self) -> MakerXmrNegotiationStatus {
        self.status
    }

    /// Delivery-and-reservation-derived binary agreement swap identity.
    #[must_use]
    pub fn swap_id(&self) -> [u8; 32] {
        maker_xmr_chat_swap_id(&self.offer_commitment, &self.reservation_id)
    }

    fn validate_metadata(&self) -> Result<(), MakerOfferError> {
        if self.offer_commitment == [0; 32]
            || self.foreign_units == 0
            || self.lez_units == 0
            || self.reserved_at_unix_seconds == 0
            || self.reserved_at_unix_seconds > i64::MAX as u64
            || self.stage_a_wire.is_empty()
            || self.stage_a_wire.len() > MAXIMUM_XMR_STAGE_A_BYTES
        {
            return Err(MakerOfferError::InvalidNegotiation);
        }
        match self.status {
            MakerXmrNegotiationStatus::StageAAccepted
                if self.activation_wire.is_none()
                    && self.activation_commitment.is_none()
                    && self.activated_at_unix_seconds.is_none()
                    && self.coordinator_swap_id.is_none() => {}
            MakerXmrNegotiationStatus::Activated
                if self.activation_wire.as_ref().is_some_and(|wire| {
                    !wire.is_empty() && wire.len() <= MAXIMUM_XMR_STAGE_B_BYTES
                }) && self
                    .activation_commitment
                    .is_some_and(|value| value != [0; 32])
                    && self
                        .activated_at_unix_seconds
                        .is_some_and(|value| value > 0 && i64::try_from(value).is_ok())
                    && self.coordinator_swap_id.is_some() => {}
            _ => return Err(MakerOfferError::InvalidNegotiation),
        }
        Ok(())
    }
}

/// Durable pre-lock Bitcoin Chat negotiation phase.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MakerBtcNegotiationStatus {
    /// Maker proposal is durable and awaits the exact Taker countersignature.
    Proposed,
    /// Final countersigned agreement and initial coordinator are durable.
    Completed,
}

/// Exact durable BTC proposal state bound to one reserved offer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MakerBtcNegotiationV1 {
    reservation_id: RequestId,
    offer_commitment: [u8; 32],
    maker_agreement_identity: [u8; 33],
    taker_agreement_identity: [u8; 33],
    foreign_units: u64,
    lez_units: u128,
    reserved_at_unix_seconds: u64,
    agreement_commitment: [u8; 32],
    maker_proposal_wire: Vec<u8>,
    status: MakerBtcNegotiationStatus,
    final_agreement_wire: Option<Vec<u8>>,
    swap_id: Option<Box<str>>,
}

impl MakerBtcNegotiationV1 {
    /// Constructs one canonical Maker proposal after application validation.
    ///
    /// The proposal itself binds the reservation-derived swap ID, both role
    /// identities, and the exact Bitcoin/LEZ amounts. The caller must use the
    /// SDK's policy-pinned proposal constructor before providing these bytes.
    ///
    /// # Errors
    ///
    /// Rejects invalid metadata or a proposal that changes any bound field.
    #[allow(clippy::too_many_arguments)]
    pub fn proposed(
        reservation_id: RequestId,
        offer_commitment: [u8; 32],
        maker_agreement_identity: [u8; 33],
        taker_agreement_identity: [u8; 33],
        foreign_units: u64,
        lez_units: u128,
        reserved_at_unix_seconds: u64,
        agreement_commitment: [u8; 32],
        maker_proposal_wire: Vec<u8>,
    ) -> Result<Self, MakerOfferError> {
        let value = Self {
            reservation_id,
            offer_commitment,
            maker_agreement_identity,
            taker_agreement_identity,
            foreign_units,
            lez_units,
            reserved_at_unix_seconds,
            agreement_commitment,
            maker_proposal_wire,
            status: MakerBtcNegotiationStatus::Proposed,
            final_agreement_wire: None,
            swap_id: None,
        };
        value.validate()?;
        Ok(value)
    }

    /// Stable winning reservation identity.
    pub const fn reservation_id(&self) -> &RequestId {
        &self.reservation_id
    }

    /// Exact authenticated Delivery envelope commitment.
    #[must_use]
    pub const fn offer_commitment(&self) -> &[u8; 32] {
        &self.offer_commitment
    }

    /// Maker agreement signing identity selected by the draft.
    #[must_use]
    pub const fn maker_agreement_identity(&self) -> &[u8; 33] {
        &self.maker_agreement_identity
    }

    /// Taker agreement signing identity selected by the draft.
    #[must_use]
    pub const fn taker_agreement_identity(&self) -> &[u8; 33] {
        &self.taker_agreement_identity
    }

    /// Exact selected Bitcoin amount in satoshis.
    #[must_use]
    pub const fn foreign_units(&self) -> u64 {
        self.foreign_units
    }

    /// Exact selected LEZ atomic units.
    #[must_use]
    pub const fn lez_units(&self) -> u128 {
        self.lez_units
    }

    /// Trusted Maker reservation time.
    #[must_use]
    pub const fn reserved_at_unix_seconds(&self) -> u64 {
        self.reserved_at_unix_seconds
    }

    /// Canonical body commitment signed by the Maker.
    #[must_use]
    pub const fn agreement_commitment(&self) -> &[u8; 32] {
        &self.agreement_commitment
    }

    /// Exact bounded Maker proposal wire sent to the Taker.
    #[must_use]
    pub fn maker_proposal_wire(&self) -> &[u8] {
        &self.maker_proposal_wire
    }

    /// Durable negotiation phase.
    #[must_use]
    pub const fn status(&self) -> MakerBtcNegotiationStatus {
        self.status
    }

    /// Exact final countersigned wire after completion.
    #[must_use]
    pub fn final_agreement_wire(&self) -> Option<&[u8]> {
        self.final_agreement_wire.as_deref()
    }

    /// Application swap identity after completion.
    #[must_use]
    pub fn swap_id(&self) -> Option<&str> {
        self.swap_id.as_deref()
    }

    fn validate(&self) -> Result<(), MakerOfferError> {
        if self.offer_commitment == [0; 32]
            || self.agreement_commitment == [0; 32]
            || self.maker_agreement_identity == [0; 33]
            || self.taker_agreement_identity == [0; 33]
            || self.maker_agreement_identity == self.taker_agreement_identity
            || self.foreign_units == 0
            || self.lez_units == 0
            || self.reserved_at_unix_seconds == 0
            || self.reserved_at_unix_seconds > i64::MAX as u64
            || self.maker_proposal_wire.is_empty()
            || self.maker_proposal_wire.len() > MAXIMUM_BTC_PROPOSAL_BYTES
            || self
                .final_agreement_wire
                .as_ref()
                .is_some_and(|wire| wire.len() > MAXIMUM_BTC_PROPOSAL_BYTES)
        {
            return Err(MakerOfferError::InvalidNegotiation);
        }
        let proposal = BtcMakerAgreementProposalV1::from_wire(&self.maker_proposal_wire)
            .map_err(|_| MakerOfferError::InvalidNegotiation)?;
        let body = proposal.body();
        if proposal.commitment() != self.agreement_commitment
            || body.swap_id()
                != &maker_btc_chat_swap_id(&self.offer_commitment, &self.reservation_id)
            || body
                .participants()
                .for_participant(lez_swap_core::Participant::Maker)
                .musig2_public_key()
                != &self.maker_agreement_identity
            || body
                .participants()
                .for_participant(lez_swap_core::Participant::Taker)
                .musig2_public_key()
                != &self.taker_agreement_identity
            || body.funding_terms().value_sat() != self.foreign_units
            || body.lez_terms().amount() != self.lez_units
        {
            return Err(MakerOfferError::InvalidNegotiation);
        }
        if self.status == MakerBtcNegotiationStatus::Completed {
            let final_wire = self
                .final_agreement_wire
                .as_deref()
                .ok_or(MakerOfferError::InvalidNegotiation)?;
            let agreement = BtcAgreementV1::from_wire(final_wire)
                .map_err(|_| MakerOfferError::InvalidNegotiation)?;
            if agreement.body() != proposal.body()
                || agreement.agreement_commitment() != &self.agreement_commitment
                || proposal.maker_signature() != agreement.record().signature(Participant::Maker)
                || self.swap_id.as_deref() != Some(agreement.coordinator().id().as_str())
            {
                return Err(MakerOfferError::InvalidNegotiation);
            }
        }
        match self.status {
            MakerBtcNegotiationStatus::Proposed
                if self.final_agreement_wire.is_none() && self.swap_id.is_none() => {}
            MakerBtcNegotiationStatus::Completed
                if self
                    .final_agreement_wire
                    .as_ref()
                    .is_some_and(|wire| !wire.is_empty())
                    && self.swap_id.is_some() => {}
            _ => return Err(MakerOfferError::InvalidNegotiation),
        }
        Ok(())
    }
}

#[derive(Deserialize, Serialize)]
struct StoredOfferCommitV1 {
    schema_version: u16,
    revision: u64,
}

#[derive(Serialize)]
struct PublishRequest<'a> {
    offer_id: &'a MakerOfferId,
    route: MakerRouteV1,
}

#[derive(Deserialize, Eq, PartialEq)]
struct ReplayPublishRequest {
    offer_id: MakerOfferId,
    route: MakerRouteV1,
}

#[derive(Serialize)]
struct ReserveRequest<'a> {
    offer_id: &'a MakerOfferId,
    expected_revision: u64,
    reservation_id: &'a RequestId,
    now_unix_seconds: u64,
}

#[derive(Serialize)]
struct ConsumeRequest<'a> {
    offer_id: &'a MakerOfferId,
    expected_revision: u64,
    reservation_id: &'a RequestId,
    swap: &'a SwapCoordinator,
}

#[derive(Serialize)]
struct StageZecNegotiationRequest<'a> {
    offer_id: &'a MakerOfferId,
    expected_revision: u64,
    reservation_id: &'a RequestId,
    offer_commitment: &'a [u8],
    maker_chat_identity: &'a [u8],
    taker_chat_identity: &'a [u8],
    foreign_units: u64,
    lez_units: String,
    agreement_commitment: &'a [u8],
    maker_proposal_sha256: [u8; 32],
}

#[derive(Serialize)]
struct StageXmrNegotiationRequest<'a> {
    offer_id: &'a MakerOfferId,
    expected_revision: u64,
    reservation_id: &'a RequestId,
    offer_commitment: &'a [u8],
    maker_agreement_identity: &'a [u8],
    taker_agreement_identity: &'a [u8],
    foreign_units: u64,
    lez_units: String,
    agreement_commitment: [u8; 32],
    stage_a_wire_sha256: [u8; 32],
}

#[derive(Serialize)]
struct StageBtcNegotiationRequest<'a> {
    offer_id: &'a MakerOfferId,
    expected_revision: u64,
    reservation_id: &'a RequestId,
    offer_commitment: &'a [u8],
    maker_agreement_identity: &'a [u8],
    taker_agreement_identity: &'a [u8],
    foreign_units: u64,
    lez_units: String,
    agreement_commitment: &'a [u8],
    maker_proposal_sha256: [u8; 32],
}

#[derive(Serialize)]
struct CompleteXmrNegotiationRequest<'a> {
    offer_id: &'a MakerOfferId,
    expected_offer_revision: u64,
    reservation_id: &'a RequestId,
    agreement_commitment: [u8; 32],
    activation_wire_sha256: [u8; 32],
    activation_commitment: [u8; 32],
    accepted_at_unix_seconds: u64,
    initial_snapshot_sha256: [u8; 32],
    actor: &'a MakerActorManifestV1,
}

#[derive(Serialize)]
struct CompleteBtcNegotiationRequest<'a> {
    offer_id: &'a MakerOfferId,
    expected_offer_revision: u64,
    reservation_id: &'a RequestId,
    agreement_wire_sha256: [u8; 32],
    agreement_commitment: [u8; 32],
    initial_snapshot_sha256: [u8; 32],
    actor: &'a MakerActorManifestV1,
}

#[derive(Serialize)]
struct WithdrawRequest<'a> {
    offer_id: &'a MakerOfferId,
    expected_revision: u64,
}

impl SqliteSwapStore {
    /// Resolves replay or snapshots the policy selected before a price effect.
    ///
    /// The returned revision must be supplied to external-price publication.
    /// An exact durable replay is returned before inspecting current policy, so
    /// retry never depends on a feed that may now be unavailable.
    ///
    /// # Errors
    ///
    /// Returns an error for request-ID conflicts, duplicate offer identity,
    /// disabled/missing policy, corrupt state, or a `SQLite` failure.
    pub fn prepare_maker_offer_publication(
        &mut self,
        request_id: &RequestId,
        offer_id: &MakerOfferId,
        route: MakerRouteV1,
    ) -> Result<MakerOfferPublicationPreflight, StoreError> {
        let request_json = serde_json::to_string(&PublishRequest { offer_id, route })?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(commit) =
            replay_offer_mutation(&transaction, request_id, "offer_publish", &request_json)?
        {
            transaction.commit()?;
            return Ok(MakerOfferPublicationPreflight::Replayed(commit));
        }
        let offer_exists: bool = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM maker_offers WHERE offer_id = ?1)",
            params![offer_id.as_str()],
            |row| row.get(0),
        )?;
        if offer_exists {
            return Err(StoreError::MakerOfferAlreadyExists);
        }
        let (policy, pair_configuration_revision) =
            load_pair(&transaction, route)?.ok_or(StoreError::MissingMakerPair)?;
        if !policy.enabled() {
            return Err(StoreError::MakerRouteDisabled);
        }
        let result = MakerOfferPublicationPreflight::Pending {
            pair_configuration_revision,
            price_source: policy.price_source(),
        };
        transaction.commit()?;
        Ok(result)
    }

    /// Publishes one local-price offer and snapshots policy and price revisions atomically.
    ///
    /// # Errors
    ///
    /// Returns an error for a disabled/non-local route, missing/corrupt source
    /// records, duplicate identity, request conflict, time overflow, or `SQLite` failure.
    pub fn publish_local_offer(
        &mut self,
        request_id: &RequestId,
        offer_id: &MakerOfferId,
        route: MakerRouteV1,
        now_unix_seconds: u64,
    ) -> Result<MakerOfferCommit, StoreError> {
        let request_json = serde_json::to_string(&PublishRequest { offer_id, route })?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(commit) =
            replay_offer_mutation(&transaction, request_id, "offer_publish", &request_json)?
        {
            transaction.commit()?;
            return Ok(commit);
        }
        if now_unix_seconds > i64::MAX as u64 {
            return Err(MakerOfferError::InvalidTime.into());
        }
        let offer_exists: bool = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM maker_offers WHERE offer_id = ?1)",
            params![offer_id.as_str()],
            |row| row.get(0),
        )?;
        if offer_exists {
            return Err(StoreError::MakerOfferAlreadyExists);
        }
        let (policy, policy_revision) =
            load_pair(&transaction, route)?.ok_or(StoreError::MissingMakerPair)?;
        if !policy.enabled() {
            return Err(StoreError::MakerRouteDisabled);
        }
        if policy.price_source() != MakerPriceSourceKind::Local {
            return Err(StoreError::MakerPriceSourceMismatch);
        }
        let (price, price_revision) =
            load_price(&transaction, route)?.ok_or(StoreError::MissingMakerLocalPrice)?;
        let expires_at_unix_seconds = now_unix_seconds
            .checked_add(policy.offer_ttl_seconds())
            .filter(|value| i64::try_from(*value).is_ok())
            .ok_or(MakerOfferError::InvalidTime)?;
        let offer = MakerOfferV1 {
            id: offer_id.clone(),
            pair_configuration: policy,
            price,
            pair_configuration_revision: policy_revision,
            price_source_revision: price_revision,
            price_observed_at_unix_seconds: now_unix_seconds,
            created_at_unix_seconds: now_unix_seconds,
            expires_at_unix_seconds,
            price_source_identity_sha256: None,
        };
        offer.validate()?;
        transaction.execute(
            "INSERT INTO maker_offers (
                 offer_id, pair, direction, payload_version, payload_json,
                 expires_at_unix_seconds, state, revision, updated_request_id
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'active', 1, ?7)",
            params![
                offer_id.as_str(),
                pair_name(route.pair()),
                direction_name(route.direction()),
                OFFER_PAYLOAD_VERSION,
                serde_json::to_string(&offer)?,
                u64_to_sql(expires_at_unix_seconds)?,
                request_id.as_str(),
            ],
        )?;
        persist_offer_mutation(&transaction, request_id, "offer_publish", &request_json, 1)?;
        transaction.commit()?;
        Ok(MakerOfferCommit {
            revision: 1,
            was_replay: false,
        })
    }

    /// Publishes one Logos-priced offer from an already validated exact quote.
    ///
    /// The route policy, quote, source revision, observation time, and request
    /// result commit in one transaction. Durable offer history prevents a
    /// source revision from rolling back or identifying different quote data.
    ///
    /// # Errors
    ///
    /// Returns an error for replay conflicts, disabled or non-Logos routes,
    /// invalid quote/time input, revision rollback/equivocation, duplicate offer
    /// identity, corrupt history, or a `SQLite` failure.
    #[allow(clippy::too_many_arguments)]
    pub fn publish_logos_offer(
        &mut self,
        request_id: &RequestId,
        offer_id: &MakerOfferId,
        route: MakerRouteV1,
        expected_pair_configuration_revision: u64,
        price: &LocalPriceV1,
        price_source_identity_sha256: [u8; 32],
        price_source_revision: u64,
        price_observed_at_unix_seconds: u64,
        now_unix_seconds: u64,
        max_age_seconds: u64,
    ) -> Result<MakerOfferCommit, StoreError> {
        let request_json = serde_json::to_string(&PublishRequest { offer_id, route })?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(commit) =
            replay_offer_mutation(&transaction, request_id, "offer_publish", &request_json)?
        {
            transaction.commit()?;
            return Ok(commit);
        }
        if price.route() != route
            || price_source_revision == 0
            || price_source_revision > i64::MAX as u64
            || price_source_identity_sha256 == [0; 32]
            || price_observed_at_unix_seconds == 0
            || price_observed_at_unix_seconds > now_unix_seconds
            || now_unix_seconds > i64::MAX as u64
            || !(1..=MAX_LOGOS_QUOTE_AGE_SECONDS).contains(&max_age_seconds)
            || now_unix_seconds - price_observed_at_unix_seconds > max_age_seconds
        {
            return Err(MakerOfferError::InvalidSnapshot.into());
        }
        let offer_exists: bool = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM maker_offers WHERE offer_id = ?1)",
            params![offer_id.as_str()],
            |row| row.get(0),
        )?;
        if offer_exists {
            return Err(StoreError::MakerOfferAlreadyExists);
        }
        let (policy, policy_revision) =
            load_pair(&transaction, route)?.ok_or(StoreError::MissingMakerPair)?;
        if policy_revision != expected_pair_configuration_revision {
            return Err(StoreError::StaleMakerConfiguration {
                expected: Some(expected_pair_configuration_revision),
                actual: Some(policy_revision),
            });
        }
        if !policy.enabled() {
            return Err(StoreError::MakerRouteDisabled);
        }
        if policy.price_source() != MakerPriceSourceKind::LogosCApi {
            return Err(StoreError::MakerPriceSourceMismatch);
        }
        let expires_at_unix_seconds = now_unix_seconds
            .checked_add(policy.offer_ttl_seconds())
            .filter(|value| i64::try_from(*value).is_ok())
            .ok_or(MakerOfferError::InvalidTime)?;
        let offer = MakerOfferV1 {
            id: offer_id.clone(),
            pair_configuration: policy,
            price: price.clone(),
            pair_configuration_revision: policy_revision,
            price_source_identity_sha256: Some(price_source_identity_sha256),
            price_source_revision,
            price_observed_at_unix_seconds,
            created_at_unix_seconds: now_unix_seconds,
            expires_at_unix_seconds,
        };
        offer.validate()?;
        update_external_price_head(
            &transaction,
            route,
            price_source_identity_sha256,
            price,
            price_source_revision,
            price_observed_at_unix_seconds,
        )?;
        transaction.execute(
            "INSERT INTO maker_offers (
                 offer_id, pair, direction, payload_version, payload_json,
                 expires_at_unix_seconds, state, revision, updated_request_id
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'active', 1, ?7)",
            params![
                offer_id.as_str(),
                pair_name(route.pair()),
                direction_name(route.direction()),
                OFFER_PAYLOAD_VERSION,
                serde_json::to_string(&offer)?,
                u64_to_sql(expires_at_unix_seconds)?,
                request_id.as_str(),
            ],
        )?;
        persist_offer_mutation(&transaction, request_id, "offer_publish", &request_json, 1)?;
        transaction.commit()?;
        Ok(MakerOfferCommit {
            revision: 1,
            was_replay: false,
        })
    }

    /// Atomically reserves one still-active and unexpired offer for one negotiation.
    ///
    /// # Errors
    ///
    /// Fails closed on expiry, non-active state, stale revision, request conflict,
    /// missing/corrupt state, time overflow, or `SQLite` failure.
    pub fn reserve_maker_offer(
        &mut self,
        request_id: &RequestId,
        offer_id: &MakerOfferId,
        expected_revision: u64,
        reservation_id: &RequestId,
        now_unix_seconds: u64,
    ) -> Result<MakerOfferCommit, StoreError> {
        let request_json = serde_json::to_string(&ReserveRequest {
            offer_id,
            expected_revision,
            reservation_id,
            now_unix_seconds,
        })?;
        transition_offer(
            self,
            OfferTransitionContext {
                request_id,
                operation: "offer_reserve",
                request_json: &request_json,
                offer_id,
                expected_revision,
                swap_to_insert: None,
            },
            |record| {
                if now_unix_seconds > i64::MAX as u64
                    || now_unix_seconds >= record.offer.expires_at_unix_seconds()
                {
                    return Err(StoreError::MakerOfferExpired);
                }
                if record.status != MakerOfferStatus::Active {
                    return Err(StoreError::MakerOfferUnavailable);
                }
                Ok(("reserved", Some(reservation_id.as_str().to_owned()), None))
            },
        )
    }

    /// Atomically reserves one ZEC offer and retains the exact maker proposal.
    ///
    /// This is the Chat midpoint linearization point. The proposal is durable
    /// before it can be sent to the taker, and exactly one competing stage can
    /// move the active offer to `reserved`.
    ///
    /// # Errors
    ///
    /// Fails closed on malformed negotiation metadata, non-ZEC route, price or
    /// amount mismatch, expiry, non-active state, stale revision, request
    /// conflict, missing/corrupt state, or `SQLite` failure.
    #[allow(clippy::too_many_lines)]
    pub fn stage_zec_maker_negotiation(
        &mut self,
        request_id: &RequestId,
        offer_id: &MakerOfferId,
        expected_revision: u64,
        negotiation: &MakerZecNegotiationV1,
    ) -> Result<MakerOfferCommit, StoreError> {
        negotiation.validate()?;
        if negotiation.status != MakerZecNegotiationStatus::Proposed {
            return Err(MakerOfferError::InvalidNegotiation.into());
        }
        let proposal_sha256: [u8; 32] = Sha256::digest(negotiation.maker_proposal_wire()).into();
        let request_json = serde_json::to_string(&StageZecNegotiationRequest {
            offer_id,
            expected_revision,
            reservation_id: negotiation.reservation_id(),
            offer_commitment: negotiation.offer_commitment(),
            maker_chat_identity: negotiation.maker_chat_identity(),
            taker_chat_identity: negotiation.taker_chat_identity(),
            foreign_units: negotiation.foreign_units(),
            lez_units: negotiation.lez_units().to_string(),
            agreement_commitment: negotiation.agreement_commitment(),
            maker_proposal_sha256: proposal_sha256,
        })?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(commit) = replay_offer_mutation(
            &transaction,
            request_id,
            "zec_negotiation_stage",
            &request_json,
        )? {
            transaction.commit()?;
            return Ok(commit);
        }
        let record = load_offer(
            &transaction,
            offer_id,
            negotiation.reserved_at_unix_seconds(),
        )?
        .ok_or(StoreError::MissingMakerOffer)?;
        if record.revision != expected_revision {
            return Err(StoreError::StaleMakerOffer {
                expected: expected_revision,
                actual: record.revision,
            });
        }
        if record.offer.route().pair() != Pair::Zcash {
            return Err(StoreError::MakerOfferSwapMismatch);
        }
        if negotiation.reserved_at_unix_seconds() >= record.offer.expires_at_unix_seconds() {
            return Err(StoreError::MakerOfferExpired);
        }
        if record.status != MakerOfferStatus::Active {
            return Err(StoreError::MakerOfferUnavailable);
        }
        if record
            .offer
            .quote_foreign_amount(negotiation.foreign_units())?
            != negotiation.lez_units()
        {
            return Err(MakerOfferError::InvalidNegotiation.into());
        }
        transaction.execute(
            "INSERT INTO maker_zec_negotiations (
                 offer_id, reservation_id, payload_version, offer_commitment,
                 maker_chat_identity, taker_chat_identity, foreign_units, lez_units,
                 reserved_at_unix_seconds, agreement_commitment, maker_proposal_wire,
                 state, final_agreement_wire, swap_id, updated_request_id
             ) VALUES (
                 ?1, ?2, 1, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10,
                 'proposed', NULL, NULL, ?11
             )",
            params![
                offer_id.as_str(),
                negotiation.reservation_id().as_str(),
                negotiation.offer_commitment().as_slice(),
                negotiation.maker_chat_identity().as_slice(),
                negotiation.taker_chat_identity().as_slice(),
                u64_to_sql(negotiation.foreign_units())?,
                negotiation.lez_units().to_be_bytes().as_slice(),
                u64_to_sql(negotiation.reserved_at_unix_seconds())?,
                negotiation.agreement_commitment().as_slice(),
                negotiation.maker_proposal_wire(),
                request_id.as_str(),
            ],
        )?;
        let revision = expected_revision
            .checked_add(1)
            .ok_or(StoreError::RevisionOverflow)?;
        let updated = transaction.execute(
            "UPDATE maker_offers SET state = 'reserved', revision = ?1,
                 reservation_id = ?2, updated_request_id = ?3
             WHERE offer_id = ?4 AND revision = ?5 AND state = 'active'",
            params![
                u64_to_sql(revision)?,
                negotiation.reservation_id().as_str(),
                request_id.as_str(),
                offer_id.as_str(),
                u64_to_sql(expected_revision)?,
            ],
        )?;
        if updated != 1 {
            return Err(StoreError::StaleMakerOffer {
                expected: expected_revision,
                actual: record.revision,
            });
        }
        persist_offer_mutation(
            &transaction,
            request_id,
            "zec_negotiation_stage",
            &request_json,
            revision,
        )?;
        transaction.commit()?;
        Ok(MakerOfferCommit {
            revision,
            was_replay: false,
        })
    }

    /// Atomically reserves one Monero offer with a dual-signed canonical Stage A.
    ///
    /// Stage A is deliberately non-executable: this transaction does not consume
    /// the offer, create a coordinator, or register an actor.
    ///
    /// # Errors
    ///
    /// Fails closed on malformed signatures/wire, wrong route, identity, derived
    /// swap ID, quote, time, replay, or durable-row drift.
    #[allow(clippy::too_many_lines)]
    pub fn stage_xmr_maker_negotiation(
        &mut self,
        request_id: &RequestId,
        offer_id: &MakerOfferId,
        expected_revision: u64,
        negotiation: &MakerXmrNegotiationV1,
    ) -> Result<MakerOfferCommit, StoreError> {
        let agreement = validate_xmr_stage_a(negotiation)?;
        let maker_identity = agreement
            .body()
            .participants()
            .for_role(XmrRoleV1::Maker)
            .agreement_public_key();
        let taker_identity = agreement
            .body()
            .participants()
            .for_role(XmrRoleV1::Taker)
            .agreement_public_key();
        let agreement_commitment = agreement.agreement_commitment();
        let stage_a_wire_sha256: [u8; 32] = Sha256::digest(negotiation.stage_a_wire()).into();
        let request_json = serde_json::to_string(&StageXmrNegotiationRequest {
            offer_id,
            expected_revision,
            reservation_id: negotiation.reservation_id(),
            offer_commitment: negotiation.offer_commitment(),
            maker_agreement_identity: &maker_identity,
            taker_agreement_identity: &taker_identity,
            foreign_units: negotiation.foreign_units(),
            lez_units: negotiation.lez_units().to_string(),
            agreement_commitment,
            stage_a_wire_sha256,
        })?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(commit) = replay_offer_mutation(
            &transaction,
            request_id,
            "xmr_negotiation_stage",
            &request_json,
        )? {
            let committed_revision = expected_revision
                .checked_add(1)
                .ok_or(StoreError::RevisionOverflow)?;
            let exact: bool = transaction.query_row(
                "SELECT EXISTS(
                     SELECT 1
                       FROM maker_offers o
                       JOIN maker_xmr_negotiations n USING (offer_id)
                      WHERE o.offer_id = ?1
                        AND o.state = 'reserved' AND o.revision = ?2
                        AND o.reservation_id = ?3 AND o.swap_id IS NULL
                        AND o.updated_request_id = ?12
                        AND n.payload_version = 1
                        AND n.reservation_id = ?3
                        AND n.offer_commitment = ?4
                        AND n.maker_agreement_identity = ?5
                        AND n.taker_agreement_identity = ?6
                        AND n.foreign_units = ?7 AND n.lez_units = ?8
                        AND n.reserved_at_unix_seconds = ?9
                        AND n.agreement_commitment = ?10
                        AND n.stage_a_wire = ?11
                        AND n.state = 'stage_a_accepted'
                        AND n.updated_request_id = ?12
                 )",
                params![
                    offer_id.as_str(),
                    u64_to_sql(committed_revision)?,
                    negotiation.reservation_id().as_str(),
                    negotiation.offer_commitment().as_slice(),
                    maker_identity.as_slice(),
                    taker_identity.as_slice(),
                    u64_to_sql(negotiation.foreign_units())?,
                    negotiation.lez_units().to_be_bytes().as_slice(),
                    u64_to_sql(negotiation.reserved_at_unix_seconds())?,
                    agreement_commitment.as_slice(),
                    negotiation.stage_a_wire(),
                    request_id.as_str(),
                ],
                |row| row.get(0),
            )?;
            let record =
                load_offer(&transaction, offer_id, 0)?.ok_or(StoreError::CorruptMakerOffer)?;
            if !exact
                || commit.revision() != committed_revision
                || record.offer.route()
                    != MakerRouteV1::new(Pair::Monero, SwapDirection::TakerSellsLez)?
                || negotiation.reserved_at_unix_seconds() < record.offer.created_at_unix_seconds()
                || negotiation.reserved_at_unix_seconds() >= record.offer.expires_at_unix_seconds()
                || record
                    .offer
                    .quote_foreign_amount(negotiation.foreign_units())?
                    != negotiation.lez_units()
            {
                return Err(StoreError::CorruptMakerOffer);
            }
            transaction.commit()?;
            return Ok(commit);
        }
        let record = load_offer(
            &transaction,
            offer_id,
            negotiation.reserved_at_unix_seconds(),
        )?
        .ok_or(StoreError::MissingMakerOffer)?;
        if record.revision != expected_revision {
            return Err(StoreError::StaleMakerOffer {
                expected: expected_revision,
                actual: record.revision,
            });
        }
        if record.offer.route().pair() != Pair::Monero
            || record.offer.route().direction() != SwapDirection::TakerSellsLez
        {
            return Err(StoreError::MakerOfferSwapMismatch);
        }
        if negotiation.reserved_at_unix_seconds() < record.offer.created_at_unix_seconds() {
            return Err(MakerOfferError::InvalidNegotiation.into());
        }
        if negotiation.reserved_at_unix_seconds() >= record.offer.expires_at_unix_seconds() {
            return Err(StoreError::MakerOfferExpired);
        }
        if record.status != MakerOfferStatus::Active {
            return Err(StoreError::MakerOfferUnavailable);
        }
        if record
            .offer
            .quote_foreign_amount(negotiation.foreign_units())?
            != negotiation.lez_units()
        {
            return Err(MakerOfferError::InvalidNegotiation.into());
        }
        transaction.execute(
            "INSERT INTO maker_xmr_negotiations (
                 offer_id, reservation_id, payload_version, offer_commitment,
                 maker_agreement_identity, taker_agreement_identity, foreign_units, lez_units,
                 reserved_at_unix_seconds, agreement_commitment, stage_a_wire,
                 state, updated_request_id
             ) VALUES (
                 ?1, ?2, 1, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10,
                 'stage_a_accepted', ?11
             )",
            params![
                offer_id.as_str(),
                negotiation.reservation_id().as_str(),
                negotiation.offer_commitment().as_slice(),
                maker_identity.as_slice(),
                taker_identity.as_slice(),
                u64_to_sql(negotiation.foreign_units())?,
                negotiation.lez_units().to_be_bytes().as_slice(),
                u64_to_sql(negotiation.reserved_at_unix_seconds())?,
                agreement_commitment.as_slice(),
                negotiation.stage_a_wire(),
                request_id.as_str(),
            ],
        )?;
        let revision = expected_revision
            .checked_add(1)
            .ok_or(StoreError::RevisionOverflow)?;
        let updated = transaction.execute(
            "UPDATE maker_offers SET state = 'reserved', revision = ?1,
                 reservation_id = ?2, updated_request_id = ?3
             WHERE offer_id = ?4 AND revision = ?5 AND state = 'active'",
            params![
                u64_to_sql(revision)?,
                negotiation.reservation_id().as_str(),
                request_id.as_str(),
                offer_id.as_str(),
                u64_to_sql(expected_revision)?,
            ],
        )?;
        if updated != 1 {
            return Err(StoreError::StaleMakerOffer {
                expected: expected_revision,
                actual: record.revision,
            });
        }
        persist_offer_mutation(
            &transaction,
            request_id,
            "xmr_negotiation_stage",
            &request_json,
            revision,
        )?;
        transaction.commit()?;
        Ok(MakerOfferCommit {
            revision,
            was_replay: false,
        })
    }

    /// Atomically reserves one Bitcoin offer and retains the exact Maker proposal.
    ///
    /// This is the pre-response linearization point: the signed proposal is
    /// durable before Chat can send it and exactly one reservation can win.
    ///
    /// # Errors
    ///
    /// Fails closed on malformed proposal metadata, a non-Bitcoin route,
    /// amount/price mismatch, expiry, stale revision, replay conflict, corrupt
    /// state, or a `SQLite` failure.
    #[allow(clippy::too_many_lines)]
    pub fn stage_btc_maker_negotiation(
        &mut self,
        request_id: &RequestId,
        offer_id: &MakerOfferId,
        expected_revision: u64,
        negotiation: &MakerBtcNegotiationV1,
    ) -> Result<MakerOfferCommit, StoreError> {
        negotiation.validate()?;
        let proposal = BtcMakerAgreementProposalV1::from_wire(negotiation.maker_proposal_wire())
            .map_err(|_| MakerOfferError::InvalidNegotiation)?;
        let proposal_direction = proposal.body().direction();
        if negotiation.status != MakerBtcNegotiationStatus::Proposed {
            return Err(MakerOfferError::InvalidNegotiation.into());
        }
        let proposal_sha256: [u8; 32] = Sha256::digest(negotiation.maker_proposal_wire()).into();
        let request_json = serde_json::to_string(&StageBtcNegotiationRequest {
            offer_id,
            expected_revision,
            reservation_id: negotiation.reservation_id(),
            offer_commitment: negotiation.offer_commitment(),
            maker_agreement_identity: negotiation.maker_agreement_identity(),
            taker_agreement_identity: negotiation.taker_agreement_identity(),
            foreign_units: negotiation.foreign_units(),
            lez_units: negotiation.lez_units().to_string(),
            agreement_commitment: negotiation.agreement_commitment(),
            maker_proposal_sha256: proposal_sha256,
        })?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(commit) = replay_offer_mutation(
            &transaction,
            request_id,
            "btc_negotiation_stage",
            &request_json,
        )? {
            let committed_revision = expected_revision
                .checked_add(1)
                .ok_or(StoreError::RevisionOverflow)?;
            let exact: bool = transaction.query_row(
                "SELECT EXISTS(
                     SELECT 1
                       FROM maker_offers o
                       JOIN maker_btc_negotiations n USING (offer_id)
                      WHERE o.offer_id = ?1
                        AND o.state = 'reserved' AND o.revision = ?2
                        AND o.reservation_id = ?3 AND o.swap_id IS NULL
                        AND o.updated_request_id = ?11
                        AND n.reservation_id = ?3 AND n.state = 'proposed'
                        AND n.offer_commitment = ?4
                        AND n.maker_agreement_identity = ?5
                        AND n.taker_agreement_identity = ?6
                        AND n.foreign_units = ?7 AND n.lez_units = ?8
                        AND n.agreement_commitment = ?9
                        AND n.maker_proposal_wire = ?10
                        AND n.final_agreement_wire IS NULL AND n.swap_id IS NULL
                        AND n.updated_request_id = ?11
                 )",
                params![
                    offer_id.as_str(),
                    u64_to_sql(committed_revision)?,
                    negotiation.reservation_id().as_str(),
                    negotiation.offer_commitment().as_slice(),
                    negotiation.maker_agreement_identity().as_slice(),
                    negotiation.taker_agreement_identity().as_slice(),
                    u64_to_sql(negotiation.foreign_units())?,
                    negotiation.lez_units().to_be_bytes().as_slice(),
                    negotiation.agreement_commitment().as_slice(),
                    negotiation.maker_proposal_wire(),
                    request_id.as_str(),
                ],
                |row| row.get(0),
            )?;
            let record =
                load_offer(&transaction, offer_id, 0)?.ok_or(StoreError::CorruptMakerOffer)?;
            let reserved_at_unix_seconds = transaction.query_row(
                "SELECT reserved_at_unix_seconds
                   FROM maker_btc_negotiations WHERE offer_id = ?1",
                params![offer_id.as_str()],
                |row| row.get::<_, i64>(0),
            )?;
            let reserved_at_unix_seconds = sql_to_u64(reserved_at_unix_seconds)?;
            if !exact
                || commit.revision() != committed_revision
                || record.offer.route().pair() != Pair::Bitcoin
                || record.offer.route().direction() != proposal_direction
                || reserved_at_unix_seconds < record.offer.created_at_unix_seconds()
                || reserved_at_unix_seconds >= record.offer.expires_at_unix_seconds()
                || record
                    .offer
                    .quote_foreign_amount(negotiation.foreign_units())?
                    != negotiation.lez_units()
            {
                return Err(StoreError::CorruptMakerOffer);
            }
            transaction.commit()?;
            return Ok(commit);
        }
        let record = load_offer(
            &transaction,
            offer_id,
            negotiation.reserved_at_unix_seconds(),
        )?
        .ok_or(StoreError::MissingMakerOffer)?;
        if record.revision != expected_revision {
            return Err(StoreError::StaleMakerOffer {
                expected: expected_revision,
                actual: record.revision,
            });
        }
        if record.offer.route().pair() != Pair::Bitcoin {
            return Err(StoreError::MakerOfferSwapMismatch);
        }
        if record.offer.route().direction() != proposal_direction {
            return Err(StoreError::MakerOfferSwapMismatch);
        }
        if negotiation.reserved_at_unix_seconds() < record.offer.created_at_unix_seconds() {
            return Err(MakerOfferError::InvalidNegotiation.into());
        }
        if negotiation.reserved_at_unix_seconds() >= record.offer.expires_at_unix_seconds() {
            return Err(StoreError::MakerOfferExpired);
        }
        if record.status != MakerOfferStatus::Active {
            return Err(StoreError::MakerOfferUnavailable);
        }
        if record
            .offer
            .quote_foreign_amount(negotiation.foreign_units())?
            != negotiation.lez_units()
        {
            return Err(MakerOfferError::InvalidNegotiation.into());
        }
        transaction.execute(
            "INSERT INTO maker_btc_negotiations (
                 offer_id, reservation_id, payload_version, offer_commitment,
                 maker_agreement_identity, taker_agreement_identity, foreign_units, lez_units,
                 reserved_at_unix_seconds, agreement_commitment, maker_proposal_wire,
                 state, final_agreement_wire, swap_id, updated_request_id
             ) VALUES (
                 ?1, ?2, 1, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10,
                 'proposed', NULL, NULL, ?11
             )",
            params![
                offer_id.as_str(),
                negotiation.reservation_id().as_str(),
                negotiation.offer_commitment().as_slice(),
                negotiation.maker_agreement_identity().as_slice(),
                negotiation.taker_agreement_identity().as_slice(),
                u64_to_sql(negotiation.foreign_units())?,
                negotiation.lez_units().to_be_bytes().as_slice(),
                u64_to_sql(negotiation.reserved_at_unix_seconds())?,
                negotiation.agreement_commitment().as_slice(),
                negotiation.maker_proposal_wire(),
                request_id.as_str(),
            ],
        )?;
        let revision = expected_revision
            .checked_add(1)
            .ok_or(StoreError::RevisionOverflow)?;
        let updated = transaction.execute(
            "UPDATE maker_offers SET state = 'reserved', revision = ?1,
                 reservation_id = ?2, updated_request_id = ?3
             WHERE offer_id = ?4 AND revision = ?5 AND state = 'active'",
            params![
                u64_to_sql(revision)?,
                negotiation.reservation_id().as_str(),
                request_id.as_str(),
                offer_id.as_str(),
                u64_to_sql(expected_revision)?,
            ],
        )?;
        if updated != 1 {
            return Err(StoreError::StaleMakerOffer {
                expected: expected_revision,
                actual: record.revision,
            });
        }
        persist_offer_mutation(
            &transaction,
            request_id,
            "btc_negotiation_stage",
            &request_json,
            revision,
        )?;
        transaction.commit()?;
        Ok(MakerOfferCommit {
            revision,
            was_replay: false,
        })
    }

    /// Recovers an exact committed Bitcoin completion before provisioning files.
    ///
    /// The durable mutation supplies the immutable actor manifest. This read-only
    /// transaction verifies the final agreement, consumed offer, completed
    /// negotiation, coordinator bytes, and actor row without changing schedule state.
    ///
    /// # Errors
    ///
    /// Fails closed on request conflicts, malformed signed wire, missing or drifted
    /// durable rows, actor mismatch, or a `SQLite` failure.
    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    pub fn preflight_maker_btc_scheduled_completion_replay(
        &mut self,
        request_id: &RequestId,
        offer_id: &MakerOfferId,
        expected_offer_revision: u64,
        reservation_id: &RequestId,
        final_agreement_wire: &[u8],
    ) -> Result<Option<MakerBtcAcceptanceReplay>, StoreError> {
        let agreement = BtcAgreementV1::from_wire(final_agreement_wire)
            .map_err(|_| StoreError::InvalidBtcApplicationState)?;
        let canonical_wire = agreement
            .encode_wire()
            .map_err(|_| StoreError::InvalidBtcApplicationState)?;
        if canonical_wire.as_slice() != final_agreement_wire {
            return Err(StoreError::InvalidBtcApplicationState);
        }
        let initial = agreement.coordinator();
        let initial_json = serde_json::to_string(initial)?;
        let initial_snapshot_sha256: [u8; 32] = Sha256::digest(initial_json.as_bytes()).into();
        let committed_revision = expected_offer_revision
            .checked_add(1)
            .ok_or(StoreError::RevisionOverflow)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Deferred)?;
        let mutation = transaction
            .query_row(
                "SELECT operation, request_payload_version, request_json, result_json
                   FROM maker_application_mutations WHERE request_id = ?1",
                params![request_id.as_str()],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                },
            )
            .optional()?;
        let Some((operation, payload_version, stored_request, stored_result)) = mutation else {
            transaction.commit()?;
            return Ok(None);
        };
        if operation != "btc_negotiation_complete" || payload_version != OFFER_PAYLOAD_VERSION {
            return Err(StoreError::MakerOfferRequestConflict);
        }
        let actor = load_maker_actor_manifest_in_transaction(&transaction, initial.id())
            .map_err(|_| StoreError::InvalidMakerActorRegistration)?
            .ok_or(StoreError::InvalidMakerActorRegistration)?;
        if actor.kind() != MakerActorKindV1::Bitcoin {
            return Err(StoreError::InvalidMakerActorRegistration);
        }
        let expected_request = serde_json::to_string(&CompleteBtcNegotiationRequest {
            offer_id,
            expected_offer_revision,
            reservation_id,
            agreement_wire_sha256: Sha256::digest(&canonical_wire).into(),
            agreement_commitment: *agreement.agreement_commitment(),
            initial_snapshot_sha256,
            actor: &actor,
        })?;
        if stored_request != expected_request {
            return Err(StoreError::MakerOfferRequestConflict);
        }
        let result: StoredOfferCommitV1 = serde_json::from_str(&stored_result)?;
        if result.schema_version != 1 || result.revision != committed_revision {
            return Err(StoreError::CorruptMakerOffer);
        }
        let negotiation = transaction
            .query_row(
                "SELECT reservation_id, payload_version, offer_commitment,
                        maker_agreement_identity, taker_agreement_identity, foreign_units,
                        lez_units, reserved_at_unix_seconds, agreement_commitment,
                        maker_proposal_wire, state, final_agreement_wire, swap_id
                   FROM maker_btc_negotiations WHERE offer_id = ?1",
                params![offer_id.as_str()],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                        row.get(7)?,
                        row.get(8)?,
                        row.get(9)?,
                        row.get(10)?,
                        row.get(11)?,
                        row.get(12)?,
                    ))
                },
            )
            .optional()?
            .map(decode_btc_negotiation_row)
            .transpose()?
            .ok_or(StoreError::CorruptMakerOffer)?;
        let proposal = BtcMakerAgreementProposalV1::from_wire(negotiation.maker_proposal_wire())
            .map_err(|_| StoreError::InvalidBtcApplicationState)?;
        let record = load_offer(&transaction, offer_id, 0)?.ok_or(StoreError::CorruptMakerOffer)?;
        let exact: bool = transaction.query_row(
            "SELECT EXISTS(
                 SELECT 1
                   FROM maker_offers o
                   JOIN maker_btc_negotiations n USING (offer_id)
                   JOIN swaps s ON s.id = n.swap_id
                  WHERE o.offer_id = ?1
                    AND o.state = 'consumed' AND o.revision = ?2
                    AND o.reservation_id = ?3 AND o.swap_id = ?4
                    AND n.state = 'completed' AND n.reservation_id = ?3
                    AND n.final_agreement_wire = ?5 AND n.swap_id = ?4
                    AND n.updated_request_id = ?6 AND s.state_json = ?7
             )",
            params![
                offer_id.as_str(),
                u64_to_sql(committed_revision)?,
                reservation_id.as_str(),
                initial.id().as_str(),
                canonical_wire,
                request_id.as_str(),
                initial_json,
            ],
            |row| row.get(0),
        )?;
        if !exact
            || negotiation.status() != MakerBtcNegotiationStatus::Completed
            || negotiation.reservation_id() != reservation_id
            || negotiation.final_agreement_wire() != Some(canonical_wire.as_slice())
            || negotiation.swap_id() != Some(initial.id().as_str())
            || record.status != MakerOfferStatus::Consumed
            || record.revision != committed_revision
            || record.reservation_id.as_ref() != Some(reservation_id)
            || record.swap_id.as_deref() != Some(initial.id().as_str())
            || record.offer.route().pair() != Pair::Bitcoin
            || record.offer.route().direction() != agreement.direction()
            || record
                .offer
                .quote_foreign_amount(negotiation.foreign_units())?
                != negotiation.lez_units()
            || proposal.body() != agreement.body()
            || proposal.commitment() != *agreement.agreement_commitment()
            || proposal.maker_signature() != agreement.record().signature(Participant::Maker)
            || negotiation.agreement_commitment() != agreement.agreement_commitment()
        {
            return Err(StoreError::CorruptMakerOffer);
        }
        require_exact_maker_actor_in_transaction(&transaction, &actor)
            .map_err(|_| StoreError::InvalidMakerActorRegistration)?;
        transaction.commit()?;
        Ok(Some(MakerBtcAcceptanceReplay {
            offer_revision: committed_revision,
            swap_id: initial.id().clone(),
            actor,
        }))
    }

    /// Atomically activates one XMR negotiation and schedules its Maker actor.
    ///
    /// Stage B, the SDK-derived coordinator, consumed offer, swap snapshot,
    /// actor registration, and replay result share one immediate transaction.
    ///
    /// # Errors
    ///
    /// Fails closed on any activation, coordinator, offer, actor, replay, or
    /// durable-state mismatch.
    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    pub fn complete_maker_xmr_negotiation_and_register_actor(
        &mut self,
        request_id: &RequestId,
        offer_id: &MakerOfferId,
        expected_offer_revision: u64,
        reservation_id: &RequestId,
        accepted: &MakerXmrActivationAcceptance,
        initial: &SwapCoordinator,
        actor: &MakerActorManifestV1,
        actor_not_before: u64,
    ) -> Result<MakerXmrAcceptanceCommit, StoreError> {
        let staged = self
            .load_xmr_maker_negotiation(offer_id)?
            .ok_or(StoreError::InvalidXmrApplicationState)?;
        let agreement =
            validate_xmr_stage_a(&staged).map_err(|_| StoreError::InvalidXmrApplicationState)?;
        let initial_json = serde_json::to_string(initial)?;
        let initial_snapshot_sha256: [u8; 32] = Sha256::digest(initial_json.as_bytes()).into();
        if staged.reservation_id() != reservation_id
            || agreement.agreement_commitment() != accepted.agreement_commitment()
            || accepted.local_role() != Participant::Maker
            || accepted.initial_snapshot_digest() != initial_snapshot_sha256
            || initial.pair() != Pair::Monero
            || initial.direction() != SwapDirection::TakerSellsLez
            || initial.phase() != Phase::Offered
            || actor.swap_id() != initial.id()
            || actor.kind() != MakerActorKindV1::Monero
            || actor_not_before > i64::MAX as u64
        {
            return Err(StoreError::InvalidXmrApplicationState);
        }
        let activation_wire_sha256: [u8; 32] = Sha256::digest(accepted.activation_wire()).into();
        let request_json = serde_json::to_string(&CompleteXmrNegotiationRequest {
            offer_id,
            expected_offer_revision,
            reservation_id,
            agreement_commitment: accepted.agreement_commitment(),
            activation_wire_sha256,
            activation_commitment: accepted.activation_commitment(),
            accepted_at_unix_seconds: accepted.accepted_at_unix_seconds(),
            initial_snapshot_sha256,
            actor,
        })?;
        let committed_revision = expected_offer_revision
            .checked_add(1)
            .ok_or(StoreError::RevisionOverflow)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(commit) = replay_offer_mutation(
            &transaction,
            request_id,
            "xmr_negotiation_complete",
            &request_json,
        )? {
            let exact: bool = transaction.query_row(
                "SELECT EXISTS(
                     SELECT 1
                       FROM maker_offers o
                       JOIN maker_xmr_negotiations n USING (offer_id)
                       JOIN swaps s ON s.id = n.swap_id
                      WHERE o.offer_id = ?1
                        AND o.state = 'consumed' AND o.revision = ?2
                        AND o.reservation_id = ?3 AND o.swap_id = ?4
                        AND o.updated_request_id = ?8
                        AND n.state = 'activated' AND n.reservation_id = ?3
                        AND n.activation_wire = ?5
                        AND n.activation_commitment = ?6
                        AND n.activated_at_unix_seconds = ?7
                        AND n.swap_id = ?4 AND n.updated_request_id = ?8
                        AND s.state_json = ?9
                 )",
                params![
                    offer_id.as_str(),
                    u64_to_sql(committed_revision)?,
                    reservation_id.as_str(),
                    initial.id().as_str(),
                    accepted.activation_wire(),
                    accepted.activation_commitment().as_slice(),
                    u64_to_sql(accepted.accepted_at_unix_seconds())?,
                    request_id.as_str(),
                    initial_json,
                ],
                |row| row.get(0),
            )?;
            let record = load_offer(&transaction, offer_id, accepted.accepted_at_unix_seconds())?
                .ok_or(StoreError::CorruptMakerOffer)?;
            let replayed = load_xmr_negotiation(&transaction, offer_id)?
                .ok_or(StoreError::CorruptMakerOffer)?;
            let replayed_agreement =
                validate_xmr_stage_a(&replayed).map_err(|_| StoreError::CorruptMakerOffer)?;
            if !exact
                || commit.revision() != committed_revision
                || record.status() != MakerOfferStatus::Consumed
                || record.revision() != committed_revision
                || record.reservation_id() != Some(reservation_id)
                || record.swap_id() != Some(initial.id().as_str())
                || replayed.status() != MakerXmrNegotiationStatus::Activated
                || replayed.reservation_id() != reservation_id
                || replayed_agreement.agreement_commitment() != accepted.agreement_commitment()
                || maker_xmr_chat_swap_id(replayed.offer_commitment(), reservation_id)
                    != replayed_agreement.body().swap_id()
                || replayed.activation_wire() != Some(accepted.activation_wire())
                || replayed.activation_commitment() != Some(accepted.activation_commitment())
                || replayed.activated_at_unix_seconds() != Some(accepted.accepted_at_unix_seconds())
                || replayed.coordinator_swap_id() != Some(initial.id().as_str())
                || record.offer().route().pair() != Pair::Monero
                || record.offer().route().direction() != SwapDirection::TakerSellsLez
                || record
                    .offer()
                    .quote_foreign_amount(replayed.foreign_units())?
                    != replayed.lez_units()
            {
                return Err(StoreError::CorruptMakerOffer);
            }
            require_exact_maker_actor_in_transaction(&transaction, actor)
                .map_err(|_| StoreError::InvalidMakerActorRegistration)?;
            transaction.commit()?;
            return Ok(MakerXmrAcceptanceCommit {
                offer_revision: committed_revision,
                was_replay: true,
            });
        }
        let record = load_offer(&transaction, offer_id, accepted.accepted_at_unix_seconds())?
            .ok_or(StoreError::MissingMakerOffer)?;
        let negotiation = transaction
            .query_row(
                "SELECT reservation_id, payload_version, offer_commitment,
                        maker_agreement_identity, taker_agreement_identity, foreign_units,
                        lez_units, reserved_at_unix_seconds, agreement_commitment,
                        stage_a_wire, state, activation_wire, activation_commitment,
                        activated_at_unix_seconds, swap_id
                   FROM maker_xmr_negotiations WHERE offer_id = ?1",
                params![offer_id.as_str()],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                        row.get(7)?,
                        row.get(8)?,
                        row.get(9)?,
                        row.get(10)?,
                        row.get(11)?,
                        row.get(12)?,
                        row.get(13)?,
                        row.get(14)?,
                    ))
                },
            )
            .optional()?
            .map(decode_xmr_negotiation_row)
            .transpose()?
            .ok_or(StoreError::InvalidXmrApplicationState)?;
        let durable_agreement = validate_xmr_stage_a(&negotiation)
            .map_err(|_| StoreError::InvalidXmrApplicationState)?;
        if record.revision != expected_offer_revision {
            return Err(StoreError::StaleMakerOffer {
                expected: expected_offer_revision,
                actual: record.revision,
            });
        }
        if record.status != MakerOfferStatus::Reserved
            || record.reservation_id.as_ref() != Some(reservation_id)
            || negotiation.status() != MakerXmrNegotiationStatus::StageAAccepted
            || negotiation.reservation_id() != reservation_id
            || accepted.accepted_at_unix_seconds() < negotiation.reserved_at_unix_seconds()
            || record.offer.route().pair() != Pair::Monero
            || record.offer.route().direction() != SwapDirection::TakerSellsLez
            || record.offer.route().direction() != initial.direction()
            || durable_agreement.agreement_commitment() != accepted.agreement_commitment()
            || maker_xmr_chat_swap_id(negotiation.offer_commitment(), reservation_id)
                != durable_agreement.body().swap_id()
            || record
                .offer
                .quote_foreign_amount(negotiation.foreign_units())?
                != negotiation.lez_units()
        {
            return Err(StoreError::InvalidXmrApplicationState);
        }
        transaction.execute(
            "INSERT INTO swaps (id, schema_version, state_json) VALUES (?1, ?2, ?3)",
            params![initial.id().as_str(), SWAP_PAYLOAD_VERSION, initial_json],
        )?;
        register_maker_actor_in_transaction(&transaction, actor, actor_not_before)
            .map_err(|_| StoreError::InvalidMakerActorRegistration)?;
        let negotiation_updated = transaction.execute(
            "UPDATE maker_xmr_negotiations
                SET state = 'activated', activation_wire = ?1,
                    activation_commitment = ?2, activated_at_unix_seconds = ?3,
                    swap_id = ?4, updated_request_id = ?5
              WHERE offer_id = ?6 AND reservation_id = ?7
                AND state = 'stage_a_accepted'",
            params![
                accepted.activation_wire(),
                accepted.activation_commitment().as_slice(),
                u64_to_sql(accepted.accepted_at_unix_seconds())?,
                initial.id().as_str(),
                request_id.as_str(),
                offer_id.as_str(),
                reservation_id.as_str(),
            ],
        )?;
        let offer_updated = transaction.execute(
            "UPDATE maker_offers
                SET state = 'consumed', revision = ?1, swap_id = ?2,
                    updated_request_id = ?3
              WHERE offer_id = ?4 AND revision = ?5 AND state = 'reserved'
                AND reservation_id = ?6",
            params![
                u64_to_sql(committed_revision)?,
                initial.id().as_str(),
                request_id.as_str(),
                offer_id.as_str(),
                u64_to_sql(expected_offer_revision)?,
                reservation_id.as_str(),
            ],
        )?;
        if negotiation_updated != 1 || offer_updated != 1 {
            return Err(StoreError::InvalidXmrApplicationState);
        }
        persist_offer_mutation(
            &transaction,
            request_id,
            "xmr_negotiation_complete",
            &request_json,
            committed_revision,
        )?;
        transaction.commit()?;
        Ok(MakerXmrAcceptanceCommit {
            offer_revision: committed_revision,
            was_replay: false,
        })
    }

    /// Atomically completes one signed Bitcoin negotiation and schedules its Maker actor.
    ///
    /// The canonical countersigned agreement, agreement-derived coordinator,
    /// completed negotiation, consumed offer, immutable actor registration, and
    /// global replay result share one `BEGIN IMMEDIATE` transaction.
    ///
    /// # Errors
    ///
    /// Fails closed on any signature, commitment, role, coordinator, route,
    /// amount, reservation, acceptance-window, actor, replay, or storage mismatch.
    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    pub fn complete_maker_btc_negotiation_and_register_actor(
        &mut self,
        request_id: &RequestId,
        offer_id: &MakerOfferId,
        expected_offer_revision: u64,
        reservation_id: &RequestId,
        accepted: &BtcAgreementAcceptance,
        initial: &SwapCoordinator,
        actor: &MakerActorManifestV1,
        actor_not_before: u64,
    ) -> Result<MakerBtcAcceptanceCommit, StoreError> {
        let agreement = BtcAgreementV1::from_wire(accepted.agreement_wire())
            .map_err(|_| StoreError::InvalidBtcApplicationState)?;
        let canonical_wire = agreement
            .encode_wire()
            .map_err(|_| StoreError::InvalidBtcApplicationState)?;
        let initial_json = serde_json::to_string(initial)?;
        let initial_snapshot_sha256: [u8; 32] = Sha256::digest(initial_json.as_bytes()).into();
        if canonical_wire.as_slice() != accepted.agreement_wire()
            || agreement.agreement_commitment() != accepted.agreement_commitment()
            || agreement.coordinator() != initial
            || accepted.swap_id() != initial.id()
            || accepted.local_role() != Participant::Maker
            || accepted.asset_extension_wire().is_some()
            || accepted.asset_commitment().is_some()
            || accepted.initial_snapshot_digest() != &initial_snapshot_sha256
            || initial.pair() != Pair::Bitcoin
            || initial.phase() != Phase::Offered
            || actor.swap_id() != initial.id()
            || actor.kind() != MakerActorKindV1::Bitcoin
            || actor_not_before > i64::MAX as u64
        {
            return Err(StoreError::InvalidBtcApplicationState);
        }
        let agreement_wire_sha256: [u8; 32] = Sha256::digest(&canonical_wire).into();
        let request_json = serde_json::to_string(&CompleteBtcNegotiationRequest {
            offer_id,
            expected_offer_revision,
            reservation_id,
            agreement_wire_sha256,
            agreement_commitment: *agreement.agreement_commitment(),
            initial_snapshot_sha256,
            actor,
        })?;
        let committed_revision = expected_offer_revision
            .checked_add(1)
            .ok_or(StoreError::RevisionOverflow)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(commit) = replay_offer_mutation(
            &transaction,
            request_id,
            "btc_negotiation_complete",
            &request_json,
        )? {
            let exact: bool = transaction.query_row(
                "SELECT EXISTS(
                     SELECT 1
                       FROM maker_offers o
                       JOIN maker_btc_negotiations n USING (offer_id)
                       JOIN swaps s ON s.id = n.swap_id
                      WHERE o.offer_id = ?1
                        AND o.state = 'consumed' AND o.revision = ?2
                        AND o.reservation_id = ?3 AND o.swap_id = ?4
                        AND n.state = 'completed' AND n.reservation_id = ?3
                        AND n.final_agreement_wire = ?5 AND n.swap_id = ?4
                        AND n.updated_request_id = ?6 AND s.state_json = ?7
                 )",
                params![
                    offer_id.as_str(),
                    u64_to_sql(committed_revision)?,
                    reservation_id.as_str(),
                    initial.id().as_str(),
                    canonical_wire,
                    request_id.as_str(),
                    initial_json,
                ],
                |row| row.get(0),
            )?;
            if !exact || commit.revision() != committed_revision {
                return Err(StoreError::CorruptMakerOffer);
            }
            require_exact_maker_actor_in_transaction(&transaction, actor)
                .map_err(|_| StoreError::InvalidMakerActorRegistration)?;
            transaction.commit()?;
            return Ok(MakerBtcAcceptanceCommit {
                offer_revision: committed_revision,
                was_replay: true,
            });
        }

        let record = load_offer(&transaction, offer_id, accepted.accepted_at_unix_seconds())?
            .ok_or(StoreError::MissingMakerOffer)?;
        let negotiation = transaction
            .query_row(
                "SELECT reservation_id, payload_version, offer_commitment,
                        maker_agreement_identity, taker_agreement_identity, foreign_units,
                        lez_units, reserved_at_unix_seconds, agreement_commitment,
                        maker_proposal_wire, state, final_agreement_wire, swap_id
                   FROM maker_btc_negotiations WHERE offer_id = ?1",
                params![offer_id.as_str()],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                        row.get(7)?,
                        row.get(8)?,
                        row.get(9)?,
                        row.get(10)?,
                        row.get(11)?,
                        row.get(12)?,
                    ))
                },
            )
            .optional()?
            .map(decode_btc_negotiation_row)
            .transpose()?
            .ok_or(StoreError::InvalidBtcApplicationState)?;
        let proposal = BtcMakerAgreementProposalV1::from_wire(negotiation.maker_proposal_wire())
            .map_err(|_| StoreError::InvalidBtcApplicationState)?;
        if record.revision != expected_offer_revision {
            return Err(StoreError::StaleMakerOffer {
                expected: expected_offer_revision,
                actual: record.revision,
            });
        }
        if record.status != MakerOfferStatus::Reserved
            || record.reservation_id.as_ref() != Some(reservation_id)
            || negotiation.status() != MakerBtcNegotiationStatus::Proposed
            || negotiation.reservation_id() != reservation_id
            || accepted.accepted_at_unix_seconds() < negotiation.reserved_at_unix_seconds()
            || accepted.accepted_at_unix_seconds() >= record.offer.expires_at_unix_seconds()
            || record.offer.route().pair() != Pair::Bitcoin
            || record.offer.route().direction() != agreement.direction()
            || initial.direction() != agreement.direction()
            || record
                .offer
                .quote_foreign_amount(negotiation.foreign_units())?
                != negotiation.lez_units()
            || proposal.body() != agreement.body()
            || proposal.commitment() != *agreement.agreement_commitment()
            || proposal.maker_signature() != agreement.record().signature(Participant::Maker)
            || negotiation.agreement_commitment() != agreement.agreement_commitment()
        {
            return Err(StoreError::InvalidBtcApplicationState);
        }
        transaction.execute(
            "INSERT INTO swaps (id, schema_version, state_json) VALUES (?1, ?2, ?3)",
            params![initial.id().as_str(), SWAP_PAYLOAD_VERSION, initial_json],
        )?;
        register_maker_actor_in_transaction(&transaction, actor, actor_not_before)
            .map_err(|_| StoreError::InvalidMakerActorRegistration)?;
        let negotiation_updated = transaction.execute(
            "UPDATE maker_btc_negotiations
                SET state = 'completed', final_agreement_wire = ?1,
                    swap_id = ?2, updated_request_id = ?3
              WHERE offer_id = ?4 AND reservation_id = ?5 AND state = 'proposed'",
            params![
                canonical_wire,
                initial.id().as_str(),
                request_id.as_str(),
                offer_id.as_str(),
                reservation_id.as_str(),
            ],
        )?;
        let offer_updated = transaction.execute(
            "UPDATE maker_offers
                SET state = 'consumed', revision = ?1, swap_id = ?2,
                    updated_request_id = ?3
              WHERE offer_id = ?4 AND revision = ?5 AND state = 'reserved'
                AND reservation_id = ?6",
            params![
                u64_to_sql(committed_revision)?,
                initial.id().as_str(),
                request_id.as_str(),
                offer_id.as_str(),
                u64_to_sql(expected_offer_revision)?,
                reservation_id.as_str(),
            ],
        )?;
        if negotiation_updated != 1 || offer_updated != 1 {
            return Err(StoreError::InvalidBtcApplicationState);
        }
        persist_offer_mutation(
            &transaction,
            request_id,
            "btc_negotiation_complete",
            &request_json,
            committed_revision,
        )?;
        transaction.commit()?;
        Ok(MakerBtcAcceptanceCommit {
            offer_revision: committed_revision,
            was_replay: false,
        })
    }

    /// Atomically binds the winning reservation to one validated swap identity.
    ///
    /// Expiry is intentionally not rechecked: reservation is the acceptance
    /// linearization point, so time passing cannot revoke already accepted terms.
    ///
    /// # Errors
    ///
    /// Fails on a wrong reservation, non-reserved state, stale revision,
    /// request conflict, missing/corrupt state, or `SQLite` failure.
    pub fn consume_maker_offer(
        &mut self,
        request_id: &RequestId,
        offer_id: &MakerOfferId,
        expected_revision: u64,
        reservation_id: &RequestId,
        swap: &SwapCoordinator,
    ) -> Result<MakerOfferCommit, StoreError> {
        let request_json = serde_json::to_string(&ConsumeRequest {
            offer_id,
            expected_revision,
            reservation_id,
            swap,
        })?;
        let staged_pair_negotiation: bool = self.connection.query_row(
            "SELECT EXISTS(
                 SELECT 1 FROM maker_zec_negotiations WHERE offer_id = ?1
                 UNION ALL SELECT 1 FROM maker_btc_negotiations WHERE offer_id = ?1
                 UNION ALL SELECT 1 FROM maker_xmr_negotiations WHERE offer_id = ?1
             )",
            params![offer_id.as_str()],
            |row| row.get(0),
        )?;
        if staged_pair_negotiation {
            return Err(StoreError::MakerOfferUnavailable);
        }
        transition_offer(
            self,
            OfferTransitionContext {
                request_id,
                operation: "offer_consume",
                request_json: &request_json,
                offer_id,
                expected_revision,
                swap_to_insert: Some(swap),
            },
            |record| {
                if record.status != MakerOfferStatus::Reserved
                    || record.reservation_id.as_ref() != Some(reservation_id)
                {
                    return Err(StoreError::MakerOfferReservationConflict);
                }
                if record.offer.route().pair() != swap.pair()
                    || record.offer.route().direction() != swap.direction()
                    || swap.phase() != Phase::Offered
                {
                    return Err(StoreError::MakerOfferSwapMismatch);
                }
                Ok((
                    "consumed",
                    Some(reservation_id.as_str().to_owned()),
                    Some(swap.id().as_str().to_owned()),
                ))
            },
        )
    }

    /// Atomically withdraws one active offer before reservation.
    ///
    /// # Errors
    ///
    /// Fails on non-active state, stale revision, request conflict,
    /// missing/corrupt state, or `SQLite` failure.
    pub fn withdraw_maker_offer(
        &mut self,
        request_id: &RequestId,
        offer_id: &MakerOfferId,
        expected_revision: u64,
    ) -> Result<MakerOfferCommit, StoreError> {
        let request_json = serde_json::to_string(&WithdrawRequest {
            offer_id,
            expected_revision,
        })?;
        transition_offer(
            self,
            OfferTransitionContext {
                request_id,
                operation: "offer_withdraw",
                request_json: &request_json,
                offer_id,
                expected_revision,
                swap_to_insert: None,
            },
            |record| {
                if record.status != MakerOfferStatus::Active {
                    return Err(StoreError::MakerOfferUnavailable);
                }
                Ok(("withdrawn", None, None))
            },
        )
    }

    /// Lists active, unexpired offers in stable identity order.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid trusted time, corrupt state, or `SQLite` failure.
    pub fn list_discoverable_maker_offers(
        &self,
        now_unix_seconds: u64,
    ) -> Result<Vec<MakerOfferRecordV1>, StoreError> {
        if now_unix_seconds > i64::MAX as u64 {
            return Err(MakerOfferError::InvalidTime.into());
        }
        list_offers(
            &self.connection,
            "WHERE state = 'active' AND expires_at_unix_seconds > ?1",
            Some(now_unix_seconds),
            now_unix_seconds,
        )
    }

    /// Lists unexpired active, reserved, or consumed offers needed for exact retry.
    ///
    /// Active advertisements allow first contact. Reserved advertisements retain
    /// the winning taker's exact authenticated envelope across restart. Consumed
    /// advertisements allow the same taker command to authenticate before exact
    /// completion replay. Withdrawn and expired offers are never projected.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid trusted time, corrupt state, or `SQLite` failure.
    pub fn list_retryable_maker_offers(
        &self,
        now_unix_seconds: u64,
    ) -> Result<Vec<MakerOfferRecordV1>, StoreError> {
        if now_unix_seconds > i64::MAX as u64 {
            return Err(MakerOfferError::InvalidTime.into());
        }
        list_offers(
            &self.connection,
            "WHERE state IN ('active', 'reserved', 'consumed') AND expires_at_unix_seconds > ?1",
            Some(now_unix_seconds),
            now_unix_seconds,
        )
    }

    /// Lists complete offer history with expiry projected at trusted caller time.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid trusted time, corrupt state, or `SQLite` failure.
    pub fn list_maker_offer_history(
        &self,
        now_unix_seconds: u64,
    ) -> Result<Vec<MakerOfferRecordV1>, StoreError> {
        if now_unix_seconds > i64::MAX as u64 {
            return Err(MakerOfferError::InvalidTime.into());
        }
        list_offers(&self.connection, "", None, now_unix_seconds)
    }

    /// Loads the exact durable ZEC Chat negotiation for one offer.
    ///
    /// # Errors
    ///
    /// Returns an error for malformed/corrupt state or `SQLite` failure.
    pub fn load_zec_maker_negotiation(
        &self,
        offer_id: &MakerOfferId,
    ) -> Result<Option<MakerZecNegotiationV1>, StoreError> {
        self.connection
            .query_row(
                "SELECT reservation_id, payload_version, offer_commitment,
                        maker_chat_identity, taker_chat_identity, foreign_units,
                        lez_units, reserved_at_unix_seconds, agreement_commitment,
                        maker_proposal_wire, state, final_agreement_wire, swap_id
                 FROM maker_zec_negotiations WHERE offer_id = ?1",
                params![offer_id.as_str()],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                        row.get(7)?,
                        row.get(8)?,
                        row.get(9)?,
                        row.get(10)?,
                        row.get(11)?,
                        row.get(12)?,
                    ))
                },
            )
            .optional()?
            .map(decode_zec_negotiation_row)
            .transpose()
    }

    /// Loads the exact durable XMR Stage-A negotiation for one offer.
    ///
    /// # Errors
    ///
    /// Returns an error for malformed, noncanonical, or corrupt durable state.
    pub fn load_xmr_maker_negotiation(
        &self,
        offer_id: &MakerOfferId,
    ) -> Result<Option<MakerXmrNegotiationV1>, StoreError> {
        load_xmr_negotiation(&self.connection, offer_id)
    }

    /// Loads the exact durable Bitcoin Chat negotiation for one offer.
    ///
    /// # Errors
    ///
    /// Returns an error for malformed/corrupt state or a `SQLite` failure.
    pub fn load_btc_maker_negotiation(
        &self,
        offer_id: &MakerOfferId,
    ) -> Result<Option<MakerBtcNegotiationV1>, StoreError> {
        self.connection
            .query_row(
                "SELECT reservation_id, payload_version, offer_commitment,
                        maker_agreement_identity, taker_agreement_identity, foreign_units,
                        lez_units, reserved_at_unix_seconds, agreement_commitment,
                        maker_proposal_wire, state, final_agreement_wire, swap_id
                 FROM maker_btc_negotiations WHERE offer_id = ?1",
                params![offer_id.as_str()],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                        row.get(7)?,
                        row.get(8)?,
                        row.get(9)?,
                        row.get(10)?,
                        row.get(11)?,
                        row.get(12)?,
                    ))
                },
            )
            .optional()?
            .map(decode_btc_negotiation_row)
            .transpose()
    }
}

#[derive(Clone, Copy)]
struct OfferTransitionContext<'a> {
    request_id: &'a RequestId,
    operation: &'static str,
    request_json: &'a str,
    offer_id: &'a MakerOfferId,
    expected_revision: u64,
    swap_to_insert: Option<&'a SwapCoordinator>,
}

fn transition_offer<F>(
    store: &mut SqliteSwapStore,
    context: OfferTransitionContext<'_>,
    transition: F,
) -> Result<MakerOfferCommit, StoreError>
where
    F: FnOnce(
        &MakerOfferRecordV1,
    ) -> Result<(&'static str, Option<String>, Option<String>), StoreError>,
{
    let transaction = store
        .connection
        .transaction_with_behavior(TransactionBehavior::Immediate)?;
    if let Some(commit) = replay_offer_mutation(
        &transaction,
        context.request_id,
        context.operation,
        context.request_json,
    )? {
        transaction.commit()?;
        return Ok(commit);
    }
    let record =
        load_offer(&transaction, context.offer_id, 0)?.ok_or(StoreError::MissingMakerOffer)?;
    if record.revision != context.expected_revision {
        return Err(StoreError::StaleMakerOffer {
            expected: context.expected_revision,
            actual: record.revision,
        });
    }
    let (state, reservation_id, swap_id) = transition(&record)?;
    if let Some(swap) = context.swap_to_insert {
        transaction.execute(
            "INSERT INTO swaps (id, schema_version, state_json) VALUES (?1, ?2, ?3)",
            params![
                swap.id().as_str(),
                SWAP_PAYLOAD_VERSION,
                serde_json::to_string(swap)?,
            ],
        )?;
    }
    let revision = context
        .expected_revision
        .checked_add(1)
        .ok_or(StoreError::RevisionOverflow)?;
    let updated = transaction.execute(
        "UPDATE maker_offers SET state = ?1, revision = ?2,
             reservation_id = ?3, swap_id = ?4, updated_request_id = ?5
         WHERE offer_id = ?6 AND revision = ?7",
        params![
            state,
            u64_to_sql(revision)?,
            reservation_id.as_deref(),
            swap_id.as_deref(),
            context.request_id.as_str(),
            context.offer_id.as_str(),
            u64_to_sql(context.expected_revision)?,
        ],
    )?;
    if updated != 1 {
        return Err(StoreError::StaleMakerOffer {
            expected: context.expected_revision,
            actual: record.revision,
        });
    }
    persist_offer_mutation(
        &transaction,
        context.request_id,
        context.operation,
        context.request_json,
        revision,
    )?;
    transaction.commit()?;
    Ok(MakerOfferCommit {
        revision,
        was_replay: false,
    })
}

fn replay_offer_mutation(
    transaction: &rusqlite::Transaction<'_>,
    request_id: &RequestId,
    operation: &str,
    request_json: &str,
) -> Result<Option<MakerOfferCommit>, StoreError> {
    let row = transaction
        .query_row(
            "SELECT operation, request_json, result_json FROM maker_application_mutations
             WHERE request_id = ?1",
            params![request_id.as_str()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .optional()?;
    let Some((stored_operation, stored_request, stored_result)) = row else {
        return Ok(None);
    };
    if stored_operation != operation {
        return Err(StoreError::MakerOfferRequestConflict);
    }
    let exact_request = stored_request == request_json;
    let compatible_legacy_publish =
        operation == "offer_publish" && equivalent_publish_requests(&stored_request, request_json);
    if !exact_request && !compatible_legacy_publish {
        return Err(StoreError::MakerOfferRequestConflict);
    }
    let result: StoredOfferCommitV1 = serde_json::from_str(&stored_result)?;
    if result.schema_version != 1 || result.revision == 0 {
        return Err(StoreError::CorruptMakerOffer);
    }
    Ok(Some(MakerOfferCommit {
        revision: result.revision,
        was_replay: true,
    }))
}

fn equivalent_publish_requests(stored: &str, current: &str) -> bool {
    let stored = serde_json::from_str::<ReplayPublishRequest>(stored);
    let current = serde_json::from_str::<ReplayPublishRequest>(current);
    match (stored, current) {
        (Ok(stored), Ok(current)) => stored == current,
        _ => false,
    }
}

fn persist_offer_mutation(
    transaction: &rusqlite::Transaction<'_>,
    request_id: &RequestId,
    operation: &str,
    request_json: &str,
    revision: u64,
) -> Result<(), StoreError> {
    let result_json = serde_json::to_string(&StoredOfferCommitV1 {
        schema_version: 1,
        revision,
    })?;
    transaction.execute(
        "INSERT INTO maker_application_mutations (
             request_id, operation, request_payload_version, request_json, result_json
         ) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            request_id.as_str(),
            operation,
            OFFER_PAYLOAD_VERSION,
            request_json,
            result_json,
        ],
    )?;
    Ok(())
}

fn load_pair(
    transaction: &rusqlite::Transaction<'_>,
    route: MakerRouteV1,
) -> Result<Option<(MakerPairConfigurationV1, u64)>, StoreError> {
    transaction
        .query_row(
            "SELECT payload_version, payload_json, revision FROM maker_pair_configurations
             WHERE pair = ?1 AND direction = ?2",
            params![pair_name(route.pair()), direction_name(route.direction())],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            },
        )
        .optional()?
        .map(|(version, json, revision)| {
            check_version(version, "maker pair configuration")?;
            let value: MakerPairConfigurationV1 = serde_json::from_str(&json)?;
            let validated = MakerPairConfigurationV1::new(
                value.route(),
                value.enabled(),
                value.price_source(),
                value.minimum_foreign_units(),
                value.maximum_foreign_units(),
                value.offer_ttl_seconds(),
            )?;
            if validated.route() != route || validated != value {
                return Err(StoreError::CorruptMakerConfiguration);
            }
            Ok((value, sql_to_u64(revision)?))
        })
        .transpose()
}

fn load_price(
    transaction: &rusqlite::Transaction<'_>,
    route: MakerRouteV1,
) -> Result<Option<(LocalPriceV1, u64)>, StoreError> {
    transaction
        .query_row(
            "SELECT payload_version, payload_json, revision FROM maker_local_prices
             WHERE pair = ?1 AND direction = ?2",
            params![pair_name(route.pair()), direction_name(route.direction())],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            },
        )
        .optional()?
        .map(|(version, json, revision)| {
            check_version(version, "maker local price")?;
            let value: LocalPriceV1 = serde_json::from_str(&json)?;
            let validated = LocalPriceV1::new(
                value.route(),
                value.lez_units_per_lot(),
                value.foreign_units_per_lot(),
            )?;
            if validated.route() != route || validated != value {
                return Err(StoreError::CorruptMakerConfiguration);
            }
            Ok((value, sql_to_u64(revision)?))
        })
        .transpose()
}

fn list_offers(
    connection: &rusqlite::Connection,
    predicate: &str,
    time_parameter: Option<u64>,
    now_unix_seconds: u64,
) -> Result<Vec<MakerOfferRecordV1>, StoreError> {
    let sql = format!(
        "SELECT offer_id, pair, direction, payload_version, payload_json,
                expires_at_unix_seconds, state, revision, reservation_id, swap_id
         FROM maker_offers {predicate} ORDER BY offer_id"
    );
    let mut statement = connection.prepare(&sql)?;
    let mut rows = match time_parameter {
        Some(value) => statement.query(params![u64_to_sql(value)?])?,
        None => statement.query([])?,
    };
    let mut records = Vec::new();
    while let Some(row) = rows.next()? {
        records.push(decode_offer_tuple(
            (
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
                row.get(6)?,
                row.get(7)?,
                row.get(8)?,
                row.get(9)?,
            ),
            now_unix_seconds,
        )?);
    }
    Ok(records)
}

fn load_offer(
    transaction: &rusqlite::Transaction<'_>,
    offer_id: &MakerOfferId,
    now_unix_seconds: u64,
) -> Result<Option<MakerOfferRecordV1>, StoreError> {
    transaction
        .query_row(
            "SELECT offer_id, pair, direction, payload_version, payload_json,
                    expires_at_unix_seconds, state, revision, reservation_id, swap_id
             FROM maker_offers WHERE offer_id = ?1",
            params![offer_id.as_str()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, i64>(7)?,
                    row.get::<_, Option<String>>(8)?,
                    row.get::<_, Option<String>>(9)?,
                ))
            },
        )
        .optional()?
        .map(|row| decode_offer_tuple(row, now_unix_seconds))
        .transpose()
}

type ZecNegotiationRow = (
    String,
    i64,
    Vec<u8>,
    Vec<u8>,
    Vec<u8>,
    i64,
    Vec<u8>,
    i64,
    Vec<u8>,
    Vec<u8>,
    String,
    Option<Vec<u8>>,
    Option<String>,
);

fn decode_zec_negotiation_row(row: ZecNegotiationRow) -> Result<MakerZecNegotiationV1, StoreError> {
    let (
        reservation_id,
        payload_version,
        offer_commitment,
        maker_chat_identity,
        taker_chat_identity,
        foreign_units,
        lez_units,
        reserved_at_unix_seconds,
        agreement_commitment,
        maker_proposal_wire,
        state,
        final_agreement_wire,
        swap_id,
    ) = row;
    check_version(payload_version, "maker ZEC negotiation")?;
    let offer_commitment: [u8; 32] = offer_commitment
        .try_into()
        .map_err(|_| StoreError::CorruptMakerOffer)?;
    let maker_chat_identity: [u8; 33] = maker_chat_identity
        .try_into()
        .map_err(|_| StoreError::CorruptMakerOffer)?;
    let taker_chat_identity: [u8; 33] = taker_chat_identity
        .try_into()
        .map_err(|_| StoreError::CorruptMakerOffer)?;
    let lez_units: [u8; 16] = lez_units
        .try_into()
        .map_err(|_| StoreError::CorruptMakerOffer)?;
    let agreement_commitment: [u8; 32] = agreement_commitment
        .try_into()
        .map_err(|_| StoreError::CorruptMakerOffer)?;
    let status = match state.as_str() {
        "proposed" => MakerZecNegotiationStatus::Proposed,
        "completed" => MakerZecNegotiationStatus::Completed,
        _ => return Err(StoreError::CorruptMakerOffer),
    };
    let swap_id = swap_id
        .map(|value| SwapId::new(value.clone()).map(|_| value.into_boxed_str()))
        .transpose()
        .map_err(|_| StoreError::CorruptMakerOffer)?;
    let value = MakerZecNegotiationV1 {
        reservation_id: RequestId::new(reservation_id)
            .map_err(|_| StoreError::CorruptMakerOffer)?,
        offer_commitment,
        maker_chat_identity,
        taker_chat_identity,
        foreign_units: sql_to_u64(foreign_units)?,
        lez_units: u128::from_be_bytes(lez_units),
        reserved_at_unix_seconds: sql_to_u64(reserved_at_unix_seconds)?,
        agreement_commitment,
        maker_proposal_wire,
        status,
        final_agreement_wire,
        swap_id,
    };
    value.validate()?;
    Ok(value)
}

type BtcNegotiationRow = ZecNegotiationRow;

fn decode_btc_negotiation_row(row: BtcNegotiationRow) -> Result<MakerBtcNegotiationV1, StoreError> {
    let (
        reservation_id,
        payload_version,
        offer_commitment,
        maker_agreement_identity,
        taker_agreement_identity,
        foreign_units,
        lez_units,
        reserved_at_unix_seconds,
        agreement_commitment,
        maker_proposal_wire,
        state,
        final_agreement_wire,
        swap_id,
    ) = row;
    check_version(payload_version, "maker BTC negotiation")?;
    let offer_commitment: [u8; 32] = offer_commitment
        .try_into()
        .map_err(|_| StoreError::CorruptMakerOffer)?;
    let maker_agreement_identity: [u8; 33] = maker_agreement_identity
        .try_into()
        .map_err(|_| StoreError::CorruptMakerOffer)?;
    let taker_agreement_identity: [u8; 33] = taker_agreement_identity
        .try_into()
        .map_err(|_| StoreError::CorruptMakerOffer)?;
    let lez_units: [u8; 16] = lez_units
        .try_into()
        .map_err(|_| StoreError::CorruptMakerOffer)?;
    let agreement_commitment: [u8; 32] = agreement_commitment
        .try_into()
        .map_err(|_| StoreError::CorruptMakerOffer)?;
    let status = match state.as_str() {
        "proposed" => MakerBtcNegotiationStatus::Proposed,
        "completed" => MakerBtcNegotiationStatus::Completed,
        _ => return Err(StoreError::CorruptMakerOffer),
    };
    let swap_id = swap_id
        .map(|value| SwapId::new(value.clone()).map(|_| value.into_boxed_str()))
        .transpose()
        .map_err(|_| StoreError::CorruptMakerOffer)?;
    let value = MakerBtcNegotiationV1 {
        reservation_id: RequestId::new(reservation_id)
            .map_err(|_| StoreError::CorruptMakerOffer)?,
        offer_commitment,
        maker_agreement_identity,
        taker_agreement_identity,
        foreign_units: sql_to_u64(foreign_units)?,
        lez_units: u128::from_be_bytes(lez_units),
        reserved_at_unix_seconds: sql_to_u64(reserved_at_unix_seconds)?,
        agreement_commitment,
        maker_proposal_wire,
        status,
        final_agreement_wire,
        swap_id,
    };
    value.validate()?;
    Ok(value)
}

fn validate_xmr_stage_a(
    negotiation: &MakerXmrNegotiationV1,
) -> Result<XmrAgreementV1, MakerOfferError> {
    negotiation.validate_metadata()?;
    let agreement = XmrAgreementV1::from_wire(negotiation.stage_a_wire())
        .map_err(|_| MakerOfferError::InvalidNegotiation)?;
    if agreement
        .encode_wire()
        .map_err(|_| MakerOfferError::InvalidNegotiation)?
        != negotiation.stage_a_wire()
        || agreement.body().direction() != XmrSwapDirectionV1::TakerSellsLez
        || agreement.body().swap_id() != negotiation.swap_id()
        || agreement.body().monero().amount_piconero() != negotiation.foreign_units()
        || agreement.body().lez().amount() != negotiation.lez_units()
    {
        return Err(MakerOfferError::InvalidNegotiation);
    }
    Ok(agreement)
}

type XmrNegotiationRow = (
    String,
    i64,
    Vec<u8>,
    Vec<u8>,
    Vec<u8>,
    i64,
    Vec<u8>,
    i64,
    Vec<u8>,
    Vec<u8>,
    String,
    Option<Vec<u8>>,
    Option<Vec<u8>>,
    Option<i64>,
    Option<String>,
);

fn load_xmr_negotiation(
    connection: &Connection,
    offer_id: &MakerOfferId,
) -> Result<Option<MakerXmrNegotiationV1>, StoreError> {
    connection
        .query_row(
            "SELECT reservation_id, payload_version, offer_commitment,
                    maker_agreement_identity, taker_agreement_identity, foreign_units,
                    lez_units, reserved_at_unix_seconds, agreement_commitment,
                    stage_a_wire, state, activation_wire, activation_commitment,
                    activated_at_unix_seconds, swap_id
               FROM maker_xmr_negotiations WHERE offer_id = ?1",
            params![offer_id.as_str()],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                    row.get(8)?,
                    row.get(9)?,
                    row.get(10)?,
                    row.get(11)?,
                    row.get(12)?,
                    row.get(13)?,
                    row.get(14)?,
                ))
            },
        )
        .optional()?
        .map(decode_xmr_negotiation_row)
        .transpose()
}

fn decode_xmr_negotiation_row(row: XmrNegotiationRow) -> Result<MakerXmrNegotiationV1, StoreError> {
    let (
        reservation_id,
        payload_version,
        offer_commitment,
        maker_agreement_identity,
        taker_agreement_identity,
        foreign_units,
        lez_units,
        reserved_at_unix_seconds,
        agreement_commitment,
        stage_a_wire,
        state,
        activation_wire,
        activation_commitment,
        activated_at_unix_seconds,
        coordinator_swap_id,
    ) = row;
    check_version(payload_version, "maker XMR negotiation")?;
    let offer_commitment: [u8; 32] = offer_commitment
        .try_into()
        .map_err(|_| StoreError::CorruptMakerOffer)?;
    let maker_agreement_identity: [u8; 33] = maker_agreement_identity
        .try_into()
        .map_err(|_| StoreError::CorruptMakerOffer)?;
    let taker_agreement_identity: [u8; 33] = taker_agreement_identity
        .try_into()
        .map_err(|_| StoreError::CorruptMakerOffer)?;
    let lez_units: [u8; 16] = lez_units
        .try_into()
        .map_err(|_| StoreError::CorruptMakerOffer)?;
    let agreement_commitment: [u8; 32] = agreement_commitment
        .try_into()
        .map_err(|_| StoreError::CorruptMakerOffer)?;
    let status = match state.as_str() {
        "stage_a_accepted" => MakerXmrNegotiationStatus::StageAAccepted,
        "activated" => MakerXmrNegotiationStatus::Activated,
        _ => return Err(StoreError::CorruptMakerOffer),
    };
    let activation_commitment = activation_commitment
        .map(|value| value.try_into().map_err(|_| StoreError::CorruptMakerOffer))
        .transpose()?;
    let coordinator_swap_id = coordinator_swap_id
        .map(|value| SwapId::new(value.clone()).map(|_| value.into_boxed_str()))
        .transpose()
        .map_err(|_| StoreError::CorruptMakerOffer)?;
    let value = MakerXmrNegotiationV1 {
        reservation_id: RequestId::new(reservation_id)
            .map_err(|_| StoreError::CorruptMakerOffer)?,
        offer_commitment,
        foreign_units: sql_to_u64(foreign_units)?,
        lez_units: u128::from_be_bytes(lez_units),
        reserved_at_unix_seconds: sql_to_u64(reserved_at_unix_seconds)?,
        stage_a_wire,
        activation_wire,
        activation_commitment,
        activated_at_unix_seconds: activated_at_unix_seconds.map(sql_to_u64).transpose()?,
        coordinator_swap_id,
        status,
    };
    value
        .validate_metadata()
        .map_err(|_| StoreError::CorruptMakerOffer)?;
    let agreement = validate_xmr_stage_a(&value).map_err(|_| StoreError::CorruptMakerOffer)?;
    if agreement.agreement_commitment() != agreement_commitment
        || agreement
            .body()
            .participants()
            .for_role(XmrRoleV1::Maker)
            .agreement_public_key()
            != maker_agreement_identity
        || agreement
            .body()
            .participants()
            .for_role(XmrRoleV1::Taker)
            .agreement_public_key()
            != taker_agreement_identity
    {
        return Err(StoreError::CorruptMakerOffer);
    }
    Ok(value)
}

type OfferRow = (
    String,
    String,
    String,
    i64,
    String,
    i64,
    String,
    i64,
    Option<String>,
    Option<String>,
);

fn decode_offer_tuple(row: OfferRow, now: u64) -> Result<MakerOfferRecordV1, StoreError> {
    let (offer_id, pair, direction, version, json, expires, state, revision, reservation, swap_id) =
        row;
    check_version(version, "maker offer")?;
    let offer: MakerOfferV1 = serde_json::from_str(&json)?;
    offer.validate()?;
    if offer.id.as_str() != offer_id
        || pair_name(offer.route().pair()) != pair
        || direction_name(offer.route().direction()) != direction
        || offer.expires_at_unix_seconds() != sql_to_u64(expires)?
    {
        return Err(StoreError::CorruptMakerOffer);
    }
    let reservation_id = reservation
        .map(RequestId::new)
        .transpose()
        .map_err(|_| StoreError::CorruptMakerOffer)?;
    let swap_id = swap_id
        .map(|value| SwapId::new(value.clone()).map(|_| value))
        .transpose()
        .map_err(|_| StoreError::CorruptMakerOffer)?;
    let (mut status, valid_shape) = match state.as_str() {
        "active" => (
            MakerOfferStatus::Active,
            reservation_id.is_none() && swap_id.is_none(),
        ),
        "reserved" => (
            MakerOfferStatus::Reserved,
            reservation_id.is_some() && swap_id.is_none(),
        ),
        "consumed" => (
            MakerOfferStatus::Consumed,
            reservation_id.is_some() && swap_id.is_some(),
        ),
        "withdrawn" => (
            MakerOfferStatus::Withdrawn,
            reservation_id.is_none() && swap_id.is_none(),
        ),
        _ => return Err(StoreError::CorruptMakerOffer),
    };
    if !valid_shape {
        return Err(StoreError::CorruptMakerOffer);
    }
    if status == MakerOfferStatus::Active && now >= offer.expires_at_unix_seconds() {
        status = MakerOfferStatus::Expired;
    }
    Ok(MakerOfferRecordV1 {
        revision: sql_to_u64(revision)?,
        status,
        offer,
        reservation_id,
        swap_id: swap_id.map(Into::into),
    })
}

fn check_version(version: i64, kind: &'static str) -> Result<(), StoreError> {
    if version != OFFER_PAYLOAD_VERSION {
        return Err(StoreError::UnsupportedPayloadVersion { kind, version });
    }
    Ok(())
}

fn sql_to_u64(value: i64) -> Result<u64, StoreError> {
    let value = u64::try_from(value).map_err(|_| StoreError::CorruptMakerOffer)?;
    if value == 0 {
        return Err(StoreError::CorruptMakerOffer);
    }
    Ok(value)
}

fn update_external_price_head(
    transaction: &rusqlite::Transaction<'_>,
    route: MakerRouteV1,
    source_identity_sha256: [u8; 32],
    price: &LocalPriceV1,
    source_revision: u64,
    observed_at_unix_seconds: u64,
) -> Result<(), StoreError> {
    let prior_head = transaction
        .query_row(
            "SELECT source_revision, observed_at_unix_seconds,
                    lez_units_per_lot, foreign_units_per_lot
             FROM maker_external_price_heads
             WHERE pair = ?1 AND direction = ?2 AND source_identity_sha256 = ?3",
            params![
                pair_name(route.pair()),
                direction_name(route.direction()),
                source_identity_sha256.as_slice(),
            ],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            },
        )
        .optional()?;
    if let Some((prior_revision, prior_observed, prior_lez, prior_foreign)) = prior_head {
        let prior_revision = sql_to_u64(prior_revision)?;
        let prior_observed = sql_to_u64(prior_observed)?;
        let prior_lez = sql_to_u64(prior_lez)?;
        let prior_foreign = sql_to_u64(prior_foreign)?;
        if prior_revision > source_revision
            || (prior_revision < source_revision && prior_observed > observed_at_unix_seconds)
        {
            return Err(StoreError::MakerPriceRevisionRollback);
        }
        if prior_revision == source_revision
            && (prior_observed != observed_at_unix_seconds
                || prior_lez != price.lez_units_per_lot()
                || prior_foreign != price.foreign_units_per_lot())
        {
            return Err(StoreError::MakerPriceRevisionConflict);
        }
    }
    transaction.execute(
        "INSERT INTO maker_external_price_heads (
             pair, direction, source_identity_sha256, source_revision,
             observed_at_unix_seconds, lez_units_per_lot, foreign_units_per_lot
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT(pair, direction, source_identity_sha256) DO UPDATE SET
             source_revision = excluded.source_revision,
             observed_at_unix_seconds = excluded.observed_at_unix_seconds,
             lez_units_per_lot = excluded.lez_units_per_lot,
             foreign_units_per_lot = excluded.foreign_units_per_lot",
        params![
            pair_name(route.pair()),
            direction_name(route.direction()),
            source_identity_sha256.as_slice(),
            u64_to_sql(source_revision)?,
            u64_to_sql(observed_at_unix_seconds)?,
            u64_to_sql(price.lez_units_per_lot())?,
            u64_to_sql(price.foreign_units_per_lot())?,
        ],
    )?;
    Ok(())
}

fn u64_to_sql(value: u64) -> Result<i64, StoreError> {
    i64::try_from(value).map_err(|_| MakerOfferError::InvalidTime.into())
}

const fn pair_name(pair: Pair) -> &'static str {
    match pair {
        Pair::Bitcoin => "bitcoin",
        Pair::Monero => "monero",
        Pair::Zcash => "zcash",
    }
}

const fn direction_name(direction: SwapDirection) -> &'static str {
    match direction {
        SwapDirection::TakerSellsForeign => "taker_sells_foreign",
        SwapDirection::TakerSellsLez => "taker_sells_lez",
    }
}

#[allow(clippy::too_many_lines)]
pub(super) fn migrate(transaction: &rusqlite::Transaction<'_>) -> Result<(), StoreError> {
    transaction.execute_batch(
        "CREATE TABLE IF NOT EXISTS maker_offers (
             offer_id                    TEXT PRIMARY KEY NOT NULL,
             pair                        TEXT NOT NULL CHECK (pair IN ('bitcoin', 'monero', 'zcash')),
             direction                   TEXT NOT NULL CHECK (direction IN ('taker_sells_foreign', 'taker_sells_lez')),
             payload_version             INTEGER NOT NULL CHECK (payload_version = 1),
             payload_json                TEXT NOT NULL,
             expires_at_unix_seconds     INTEGER NOT NULL CHECK (expires_at_unix_seconds > 0),
             state                       TEXT NOT NULL CHECK (state IN ('active', 'reserved', 'consumed', 'withdrawn')),
             revision                    INTEGER NOT NULL CHECK (revision > 0),
             reservation_id              TEXT,
             swap_id                     TEXT,
             updated_request_id          TEXT NOT NULL,
             CHECK (pair != 'monero' OR direction = 'taker_sells_lez'),
             FOREIGN KEY (swap_id) REFERENCES swaps(id) ON DELETE RESTRICT,
             CHECK (
                 (state IN ('active', 'withdrawn') AND reservation_id IS NULL AND swap_id IS NULL)
                 OR (state = 'reserved' AND reservation_id IS NOT NULL AND swap_id IS NULL)
                 OR (state = 'consumed' AND reservation_id IS NOT NULL AND swap_id IS NOT NULL)
             )
         ) STRICT;
         CREATE INDEX IF NOT EXISTS maker_offers_discovery
             ON maker_offers (state, expires_at_unix_seconds, pair, direction, offer_id);
         CREATE TABLE IF NOT EXISTS maker_external_price_heads (
             pair                        TEXT NOT NULL CHECK (pair IN ('bitcoin', 'monero', 'zcash')),
             direction                   TEXT NOT NULL CHECK (direction IN ('taker_sells_foreign', 'taker_sells_lez')),
             source_identity_sha256      BLOB NOT NULL CHECK (length(source_identity_sha256) = 32),
             source_revision             INTEGER NOT NULL CHECK (source_revision > 0),
             observed_at_unix_seconds    INTEGER NOT NULL CHECK (observed_at_unix_seconds > 0),
             lez_units_per_lot           INTEGER NOT NULL CHECK (lez_units_per_lot > 0),
             foreign_units_per_lot       INTEGER NOT NULL CHECK (foreign_units_per_lot > 0),
             PRIMARY KEY (pair, direction, source_identity_sha256),
             CHECK (pair != 'monero' OR direction = 'taker_sells_lez')
         ) STRICT;
         CREATE TABLE IF NOT EXISTS maker_zec_negotiations (
             offer_id                  TEXT PRIMARY KEY NOT NULL,
             reservation_id            TEXT NOT NULL UNIQUE,
             payload_version           INTEGER NOT NULL CHECK (payload_version = 1),
             offer_commitment           BLOB NOT NULL CHECK (length(offer_commitment) = 32),
             maker_chat_identity        BLOB NOT NULL CHECK (length(maker_chat_identity) = 33),
             taker_chat_identity        BLOB NOT NULL CHECK (length(taker_chat_identity) = 33),
             foreign_units              INTEGER NOT NULL CHECK (foreign_units > 0),
             lez_units                  BLOB NOT NULL CHECK (length(lez_units) = 16),
             reserved_at_unix_seconds   INTEGER NOT NULL CHECK (reserved_at_unix_seconds > 0),
             agreement_commitment       BLOB NOT NULL CHECK (length(agreement_commitment) = 32),
             maker_proposal_wire        BLOB NOT NULL CHECK (
                 length(maker_proposal_wire) BETWEEN 1 AND 16384
             ),
             state                      TEXT NOT NULL CHECK (state IN ('proposed', 'completed')),
             final_agreement_wire       BLOB CHECK (
                 final_agreement_wire IS NULL
                 OR length(final_agreement_wire) BETWEEN 1 AND 16384
             ),
             swap_id                    TEXT,
             updated_request_id         TEXT NOT NULL,
             FOREIGN KEY (offer_id) REFERENCES maker_offers(offer_id) ON DELETE RESTRICT,
             FOREIGN KEY (swap_id) REFERENCES swaps(id) ON DELETE RESTRICT,
             CHECK (maker_chat_identity != taker_chat_identity),
             CHECK (
                 (state = 'proposed' AND final_agreement_wire IS NULL AND swap_id IS NULL)
                 OR (state = 'completed' AND final_agreement_wire IS NOT NULL AND swap_id IS NOT NULL)
             )
         ) STRICT;
         CREATE INDEX IF NOT EXISTS maker_zec_negotiations_state_reservation
             ON maker_zec_negotiations (state, reservation_id);
         CREATE TABLE IF NOT EXISTS maker_btc_negotiations (
             offer_id                     TEXT PRIMARY KEY NOT NULL,
             reservation_id               TEXT NOT NULL UNIQUE,
             payload_version              INTEGER NOT NULL CHECK (payload_version = 1),
             offer_commitment              BLOB NOT NULL CHECK (length(offer_commitment) = 32),
             maker_agreement_identity      BLOB NOT NULL CHECK (length(maker_agreement_identity) = 33),
             taker_agreement_identity      BLOB NOT NULL CHECK (length(taker_agreement_identity) = 33),
             foreign_units                 INTEGER NOT NULL CHECK (foreign_units > 0),
             lez_units                     BLOB NOT NULL CHECK (length(lez_units) = 16),
             reserved_at_unix_seconds      INTEGER NOT NULL CHECK (reserved_at_unix_seconds > 0),
             agreement_commitment          BLOB NOT NULL CHECK (length(agreement_commitment) = 32),
             maker_proposal_wire           BLOB NOT NULL CHECK (
                 length(maker_proposal_wire) BETWEEN 1 AND 16384
             ),
             state                         TEXT NOT NULL CHECK (state IN ('proposed', 'completed')),
             final_agreement_wire          BLOB CHECK (
                 final_agreement_wire IS NULL
                 OR length(final_agreement_wire) BETWEEN 1 AND 16384
             ),
             swap_id                       TEXT,
             updated_request_id            TEXT NOT NULL,
             FOREIGN KEY (offer_id) REFERENCES maker_offers(offer_id) ON DELETE RESTRICT,
             FOREIGN KEY (swap_id) REFERENCES swaps(id) ON DELETE RESTRICT,
             CHECK (maker_agreement_identity != taker_agreement_identity),
             CHECK (
                 (state = 'proposed' AND final_agreement_wire IS NULL AND swap_id IS NULL)
                 OR (state = 'completed' AND final_agreement_wire IS NOT NULL AND swap_id IS NOT NULL)
             )
         ) STRICT;
         CREATE INDEX IF NOT EXISTS maker_btc_negotiations_state_reservation
             ON maker_btc_negotiations (state, reservation_id);
         CREATE TABLE IF NOT EXISTS maker_xmr_negotiations (
             offer_id                     TEXT PRIMARY KEY NOT NULL,
             reservation_id               TEXT NOT NULL UNIQUE,
             payload_version              INTEGER NOT NULL CHECK (payload_version = 1),
             offer_commitment              BLOB NOT NULL CHECK (length(offer_commitment) = 32),
             maker_agreement_identity      BLOB NOT NULL CHECK (length(maker_agreement_identity) = 33),
             taker_agreement_identity      BLOB NOT NULL CHECK (length(taker_agreement_identity) = 33),
             foreign_units                 INTEGER NOT NULL CHECK (foreign_units > 0),
             lez_units                     BLOB NOT NULL CHECK (length(lez_units) = 16),
             reserved_at_unix_seconds      INTEGER NOT NULL CHECK (reserved_at_unix_seconds > 0),
             agreement_commitment          BLOB NOT NULL CHECK (length(agreement_commitment) = 32),
             stage_a_wire                  BLOB NOT NULL CHECK (
                 length(stage_a_wire) BETWEEN 1 AND 276480
             ),
             state                         TEXT NOT NULL CHECK (state IN ('stage_a_accepted', 'activated')),
             activation_wire               BLOB CHECK (
                 activation_wire IS NULL OR length(activation_wire) BETWEEN 1 AND 2048
             ),
             activation_commitment         BLOB CHECK (
                 activation_commitment IS NULL OR length(activation_commitment) = 32
             ),
             activated_at_unix_seconds     INTEGER CHECK (
                 activated_at_unix_seconds IS NULL OR activated_at_unix_seconds > 0
             ),
             swap_id                       TEXT,
             updated_request_id            TEXT NOT NULL,
             FOREIGN KEY (offer_id) REFERENCES maker_offers(offer_id) ON DELETE RESTRICT,
             FOREIGN KEY (swap_id) REFERENCES swaps(id) ON DELETE RESTRICT,
             CHECK (maker_agreement_identity != taker_agreement_identity),
             CHECK (
                 (state = 'stage_a_accepted'
                  AND activation_wire IS NULL AND activation_commitment IS NULL
                  AND activated_at_unix_seconds IS NULL AND swap_id IS NULL)
                 OR
                 (state = 'activated'
                  AND activation_wire IS NOT NULL AND activation_commitment IS NOT NULL
                  AND activated_at_unix_seconds IS NOT NULL AND swap_id IS NOT NULL)
             )
         ) STRICT;
         CREATE INDEX IF NOT EXISTS maker_xmr_negotiations_state_reservation
             ON maker_xmr_negotiations (state, reservation_id);
",
    )?;
    let supports_xmr_activation: bool = transaction.query_row(
        "SELECT EXISTS(
             SELECT 1 FROM pragma_table_info('maker_xmr_negotiations')
              WHERE name = 'activation_wire'
         )",
        [],
        |row| row.get(0),
    )?;
    if !supports_xmr_activation {
        transaction.execute_batch(
            "DROP INDEX IF EXISTS maker_xmr_negotiations_state_reservation;
             ALTER TABLE maker_xmr_negotiations
                 RENAME TO maker_xmr_negotiations_before_activation;
             CREATE TABLE maker_xmr_negotiations (
                 offer_id                     TEXT PRIMARY KEY NOT NULL,
                 reservation_id               TEXT NOT NULL UNIQUE,
                 payload_version              INTEGER NOT NULL CHECK (payload_version = 1),
                 offer_commitment              BLOB NOT NULL CHECK (length(offer_commitment) = 32),
                 maker_agreement_identity      BLOB NOT NULL CHECK (length(maker_agreement_identity) = 33),
                 taker_agreement_identity      BLOB NOT NULL CHECK (length(taker_agreement_identity) = 33),
                 foreign_units                 INTEGER NOT NULL CHECK (foreign_units > 0),
                 lez_units                     BLOB NOT NULL CHECK (length(lez_units) = 16),
                 reserved_at_unix_seconds      INTEGER NOT NULL CHECK (reserved_at_unix_seconds > 0),
                 agreement_commitment          BLOB NOT NULL CHECK (length(agreement_commitment) = 32),
                 stage_a_wire                  BLOB NOT NULL CHECK (length(stage_a_wire) BETWEEN 1 AND 276480),
                 state                         TEXT NOT NULL CHECK (state IN ('stage_a_accepted', 'activated')),
                 activation_wire               BLOB CHECK (activation_wire IS NULL OR length(activation_wire) BETWEEN 1 AND 2048),
                 activation_commitment         BLOB CHECK (activation_commitment IS NULL OR length(activation_commitment) = 32),
                 activated_at_unix_seconds     INTEGER CHECK (activated_at_unix_seconds IS NULL OR activated_at_unix_seconds > 0),
                 swap_id                       TEXT,
                 updated_request_id            TEXT NOT NULL,
                 FOREIGN KEY (offer_id) REFERENCES maker_offers(offer_id) ON DELETE RESTRICT,
                 FOREIGN KEY (swap_id) REFERENCES swaps(id) ON DELETE RESTRICT,
                 CHECK (maker_agreement_identity != taker_agreement_identity),
                 CHECK (
                     (state = 'stage_a_accepted' AND activation_wire IS NULL
                      AND activation_commitment IS NULL AND activated_at_unix_seconds IS NULL
                      AND swap_id IS NULL)
                     OR
                     (state = 'activated' AND activation_wire IS NOT NULL
                      AND activation_commitment IS NOT NULL AND activated_at_unix_seconds IS NOT NULL
                      AND swap_id IS NOT NULL)
                 )
             ) STRICT;
             INSERT INTO maker_xmr_negotiations (
                 offer_id, reservation_id, payload_version, offer_commitment,
                 maker_agreement_identity, taker_agreement_identity, foreign_units,
                 lez_units, reserved_at_unix_seconds, agreement_commitment, stage_a_wire,
                 state, activation_wire, activation_commitment,
                 activated_at_unix_seconds, swap_id, updated_request_id
             )
             SELECT offer_id, reservation_id, payload_version, offer_commitment,
                    maker_agreement_identity, taker_agreement_identity, foreign_units,
                    lez_units, reserved_at_unix_seconds, agreement_commitment, stage_a_wire,
                    state, NULL, NULL, NULL, NULL, updated_request_id
               FROM maker_xmr_negotiations_before_activation;
             DROP TABLE maker_xmr_negotiations_before_activation;
             CREATE INDEX maker_xmr_negotiations_state_reservation
                 ON maker_xmr_negotiations (state, reservation_id);",
        )?;
    }
    Ok(())
}
/// Derives the signed 32-byte Chat session from its durable reservation ID.
///
/// Both peers can recompute this before signing, while the store can prove the
/// final transcript belongs to the exact winning reservation after restart.
#[must_use]
pub fn maker_zec_chat_session_id(reservation_id: &RequestId) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(ZEC_CHAT_SESSION_DOMAIN);
    hasher.update(reservation_id.as_str().as_bytes());
    hasher.finalize().into()
}

/// Derives the signed XMR Stage-A swap ID from Delivery and reservation.
///
/// A domain distinct from every other pair prevents cross-pair replay.
#[must_use]
pub fn maker_xmr_chat_swap_id(offer_commitment: &[u8; 32], reservation_id: &RequestId) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(XMR_CHAT_SWAP_ID_DOMAIN);
    hasher.update(offer_commitment);
    hasher.update(reservation_id.as_str().as_bytes());
    hasher.finalize().into()
}

/// Derives the signed BTC application swap ID from Delivery and reservation.
///
/// Both peers can recompute this before signing. The final agreement therefore
/// cannot be moved to a different authenticated offer or winning reservation.
#[must_use]
pub fn maker_btc_chat_swap_id(offer_commitment: &[u8; 32], reservation_id: &RequestId) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(BTC_CHAT_SWAP_ID_DOMAIN);
    hasher.update(offer_commitment);
    hasher.update(reservation_id.as_str().as_bytes());
    hasher.finalize().into()
}
