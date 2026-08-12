//! Secret-free typed contract for a future role-fixed Taker facade.
//!
//! This module deliberately contains no transport, filesystem, receipt, key,
//! actor, or chain authority. It fixes the messages that a later owner-local
//! service may expose without granting callers generic execution authority.

use lez_bridge_protocol::RequestId;
use lez_swap_core::{Pair, SwapDirection, SwapId};
use lez_swap_store::{MakerOfferId, MakerOfferV1, MakerRouteV1};
use secp256k1::PublicKey;
use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error as _};

/// Current schema version for every Taker facade v1 message.
pub const TAKER_FACADE_SCHEMA_VERSION_V1: u16 = 1;

/// Unsupported schema version supplied to a Taker facade v1 request.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[error("unsupported Taker facade request schema {actual}; expected {expected}")]
pub struct TakerFacadeSchemaVersionError {
    actual: u16,
    expected: u16,
}

impl TakerFacadeSchemaVersionError {
    /// Returns the unsupported version supplied by the caller.
    #[must_use]
    pub const fn actual(&self) -> u16 {
        self.actual
    }

    /// Returns the only schema version accepted by this contract.
    #[must_use]
    pub const fn expected(&self) -> u16 {
        self.expected
    }
}

/// Exact allowlist for the first Taker facade contract.
///
/// A server must register these methods individually. This list grants no
/// generic command, executable, path, key, receipt, or raw-payload method.
pub const TAKER_FACADE_METHODS_V1: [&str; 7] = [
    "taker_health",
    "taker_offer_list_v1",
    "taker_swap_list_v1",
    "taker_swap_initiate_v1",
    "taker_swap_monitor_v1",
    "taker_swap_claim_v1",
    "taker_swap_refund_v1",
];

/// Canonical compressed secp256k1 Maker identity used at the UI boundary.
///
/// The wire form is exactly 66 lowercase hexadecimal characters. Construction
/// and deserialization both require a valid 33-byte compressed curve point.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct TakerMakerIdentityV1([u8; 33]);

impl TakerMakerIdentityV1 {
    /// Validates one exact compressed secp256k1 public identity.
    ///
    /// # Errors
    ///
    /// Returns a secp256k1 error for a malformed or non-curve identity.
    pub fn new(bytes: [u8; 33]) -> Result<Self, secp256k1::Error> {
        PublicKey::from_slice(&bytes)?;
        Ok(Self(bytes))
    }

    /// Returns the exact compressed identity bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 33] {
        &self.0
    }
}

impl Serialize for TakerMakerIdentityV1 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&hex::encode(self.0))
    }
}

impl<'de> Deserialize<'de> for TakerMakerIdentityV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        if value.len() != 66
            || value
                .bytes()
                .any(|byte| !byte.is_ascii_digit() && !(b'a'..=b'f').contains(&byte))
        {
            return Err(D::Error::custom(
                "Maker identity is not canonical lowercase hex",
            ));
        }
        let mut bytes = [0_u8; 33];
        hex::decode_to_slice(&value, &mut bytes)
            .map_err(|_| D::Error::custom("Maker identity is not canonical lowercase hex"))?;
        Self::new(bytes)
            .map_err(|_| D::Error::custom("Maker identity is not a compressed secp256k1 point"))
    }
}

/// Parameters for reading Taker facade health and capabilities.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TakerHealthRequestV1 {
    /// Request schema version; must be one.
    pub schema_version: u16,
}

/// Optional exact route filter for authenticated offer discovery.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TakerOfferListRequestV1 {
    /// Request schema version; must be one.
    pub schema_version: u16,
    /// Exact supported route, or `None` to browse every configured route.
    pub route: Option<MakerRouteV1>,
}

/// Parameters for listing swaps already indexed by the role-fixed facade.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TakerSwapListRequestV1 {
    /// Request schema version; must be one.
    pub schema_version: u16,
}

/// Exact reviewed public facts required to initiate one swap.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TakerSwapInitiateRequestV1 {
    /// Request schema version; must be one.
    pub schema_version: u16,
    /// Global identity for exact durable replay of this initiation.
    pub request_id: RequestId,
    /// Authenticated Delivery offer selected by the user.
    pub offer_id: MakerOfferId,
    /// Reviewed pair and direction; another route is never substituted.
    pub route: MakerRouteV1,
    /// Compressed public Maker identity that authenticated the offer.
    pub maker_identity: TakerMakerIdentityV1,
    /// SHA-256 of the exact signed Delivery envelope reviewed by the user.
    pub signed_envelope_sha256: [u8; 32],
    /// Exact selected foreign-chain atomic-unit amount.
    pub foreign_units: u64,
    /// Exact integer LEZ quote reviewed by the user without rounding.
    pub expected_lez_units: u128,
}

/// Parameters for reading one receipt-bound lifecycle projection.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TakerSwapMonitorRequestV1 {
    /// Request schema version; must be one.
    pub schema_version: u16,
    /// Stable application swap identity; the facade resolves private state.
    pub swap_id: SwapId,
}

/// Parameters for an explicit generation-fenced Taker claim.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TakerClaimRequestV1 {
    /// Request schema version; must be one.
    pub schema_version: u16,
    /// Global identity for exact durable replay of this claim request.
    pub request_id: RequestId,
    /// Stable application swap identity; never a receipt or state path.
    pub swap_id: SwapId,
    /// Progress generation observed before the user confirmed the claim.
    pub expected_generation: u64,
}

/// Parameters for an explicit generation-fenced Taker refund.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TakerRefundRequestV1 {
    /// Request schema version; must be one.
    pub schema_version: u16,
    /// Global identity for exact durable replay of this refund request.
    pub request_id: RequestId,
    /// Stable application swap identity; never a receipt or state path.
    pub swap_id: SwapId,
    /// Progress generation observed before the user confirmed the refund.
    pub expected_generation: u64,
}

fn validate_taker_facade_schema_version(actual: u16) -> Result<(), TakerFacadeSchemaVersionError> {
    if actual == TAKER_FACADE_SCHEMA_VERSION_V1 {
        Ok(())
    } else {
        Err(TakerFacadeSchemaVersionError {
            actual,
            expected: TAKER_FACADE_SCHEMA_VERSION_V1,
        })
    }
}

macro_rules! impl_request_schema_validation {
    ($($request:ty),+ $(,)?) => {
        $(
            impl $request {
                /// Verifies that this request uses the exact supported schema version.
                ///
                /// # Errors
                ///
                /// Returns an error unless `schema_version` is exactly one.
                pub fn validate_schema_version(
                    &self,
                ) -> Result<(), TakerFacadeSchemaVersionError> {
                    validate_taker_facade_schema_version(self.schema_version)
                }
            }
        )+
    };
}

impl_request_schema_validation!(
    TakerHealthRequestV1,
    TakerOfferListRequestV1,
    TakerSwapListRequestV1,
    TakerSwapInitiateRequestV1,
    TakerSwapMonitorRequestV1,
    TakerClaimRequestV1,
    TakerRefundRequestV1,
);

/// Secret-free authenticated offer displayed before user confirmation.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TakerOfferViewV1 {
    /// Complete validated immutable public offer terms.
    pub offer: MakerOfferV1,
    /// Compressed public Maker identity that authenticated the offer.
    pub maker_identity: TakerMakerIdentityV1,
    /// SHA-256 of the retained signed envelope, without its raw bytes.
    pub signed_envelope_sha256: [u8; 32],
}

/// Versioned collection returned by authenticated offer discovery.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TakerOfferListV1 {
    /// Response schema version; currently one.
    pub schema_version: u16,
    /// Bounded authenticated public offers selected by the service.
    pub offers: Vec<TakerOfferViewV1>,
}

/// Normalized secret-free lifecycle state for the Taker UI.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TakerSwapStateV1 {
    /// A globally idempotent initiation is still being reconciled.
    Initiating,
    /// Acceptance and role provisioning are durable but not activated.
    NotActivated,
    /// The role actor is waiting for the agreement-ordered first lock.
    AwaitingFirstLock,
    /// The first lock is final and the role actor awaits the second lock.
    AwaitingSecondLock,
    /// Both agreement-ordered locks are final.
    BothLegsLocked,
    /// A claim is currently the only user-authorized terminal action.
    ClaimAvailable,
    /// A refund is currently the only user-authorized terminal action.
    RefundAvailable,
    /// A previously admitted claim is being reconciled.
    ClaimInProgress,
    /// A previously admitted refund is being reconciled.
    RefundInProgress,
    /// The complete claim lifecycle reached its terminal state.
    Completed,
    /// The complete recovery lifecycle reached its terminal state.
    Refunded,
    /// Durable state requires explicit operator attention.
    AttentionRequired,
}

/// Explicit terminal action the UI may offer at one observed generation.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TakerTerminalActionV1 {
    /// Agreement-ordered claim progression.
    Claim,
    /// Agreement-ordered timeout recovery.
    Refund,
}

/// Non-effect privacy guidance associated with one terminal ZEC claim.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TakerPrivacyGuidanceV1 {
    /// Transparent-pool linkage remains public; shielding is a separate wallet action.
    ShieldReceivedTransparentZecSeparately,
}

/// One secret-free receipt-bound progress projection.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TakerSwapViewV1 {
    /// Response schema version; currently one.
    pub schema_version: u16,
    /// Stable application swap identity.
    pub swap_id: SwapId,
    /// Immutable Delivery offer that initiated the swap.
    pub offer_id: MakerOfferId,
    /// Exact accepted pair and direction.
    pub route: MakerRouteV1,
    /// Exact accepted foreign-chain atomic-unit amount.
    pub foreign_units: u64,
    /// Exact accepted LEZ atomic-unit amount.
    pub lez_units: u128,
    /// Monotonic receipt/actor progress fence observed by the service.
    pub progress_generation: u64,
    /// Normalized receipt-bound lifecycle state.
    pub state: TakerSwapStateV1,
    /// Sole currently admissible terminal action, when one exists.
    pub available_action: Option<TakerTerminalActionV1>,
    /// Non-effect guidance shown only after receiving transparent ZEC by claim.
    pub privacy_guidance: Option<TakerPrivacyGuidanceV1>,
}

/// Versioned collection of swaps recoverable after UI or facade restart.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TakerSwapListV1 {
    /// Response schema version; currently one.
    pub schema_version: u16,
    /// Secret-free lifecycle projections indexed by the service.
    pub swaps: Vec<TakerSwapViewV1>,
}

/// Durable result of one exact initiation request.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TakerInitiationCommitV1 {
    /// Response schema version; currently one.
    pub schema_version: u16,
    /// Newly accepted or exactly replayed secret-free swap projection.
    pub swap: TakerSwapViewV1,
    /// Whether this exact request and payload were already durable.
    pub was_replay: bool,
}

/// Durable admission result for one explicit claim or refund request.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TakerActionCommitV1 {
    /// Response schema version; currently one.
    pub schema_version: u16,
    /// Stable application swap identity.
    pub swap_id: SwapId,
    /// Method-fixed action admitted by the service.
    pub action: TakerTerminalActionV1,
    /// Generation against which the original request was admitted.
    pub requested_after_generation: u64,
    /// Whether this exact request and payload were already durable.
    pub was_replay: bool,
}

/// Read-only state of one optional Taker application dependency.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TakerDependencyStateV1 {
    /// The optional dependency is not configured.
    Disabled,
    /// The configured dependency is available and internally consistent.
    Available,
    /// The service remains inspectable, but the dependency needs attention.
    Unavailable,
}

/// Current initiation boundary supported by one pair adapter.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TakerInitiationCapabilityV1 {
    /// Private drafts, keys, role templates, and effect material must be preprovisioned.
    PreparedPrivateMaterial,
}

/// Current monitoring boundary supported by one pair adapter.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TakerMonitoringCapabilityV1 {
    /// Monitoring selects and revalidates exact private receipt-bound authority.
    ReceiptBound,
}

/// Honest scope of one pair's receipt-bound claim or refund route.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TakerTerminalActionCapabilityV1 {
    /// The pair actor exposes complete receipt-bound terminal progression.
    FullLifecycle,
    /// Only a role-fixed effect checkpoint exists; it is not terminal swap proof.
    EffectCheckpointOnly,
}

/// Exact JSON-RPC methods registered by the current Taker service.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)]
pub struct TakerRegisteredMethodsV1 {
    health: bool,
    offer_list: bool,
    swap_list: bool,
    initiate: bool,
    monitor: bool,
    claim: bool,
    refund: bool,
}

impl TakerRegisteredMethodsV1 {
    /// Returns the honest method set of the current read-only service.
    #[must_use]
    pub const fn read_only() -> Self {
        Self {
            health: true,
            offer_list: true,
            swap_list: false,
            initiate: false,
            monitor: false,
            claim: false,
            refund: false,
        }
    }

    /// Returns the honest method set of a receipt-monitoring admission service.
    #[must_use]
    pub const fn read_with_initiation() -> Self {
        Self {
            health: true,
            offer_list: true,
            swap_list: true,
            initiate: true,
            monitor: true,
            claim: false,
            refund: false,
        }
    }

    /// Returns the honest method set of a complete receipt-bound ZEC service.
    #[must_use]
    pub const fn full_zec_lifecycle() -> Self {
        Self {
            health: true,
            offer_list: true,
            swap_list: true,
            initiate: true,
            monitor: true,
            claim: true,
            refund: true,
        }
    }

    /// Reports whether health is registered.
    #[must_use]
    pub const fn health(self) -> bool {
        self.health
    }

    /// Reports whether authenticated offer listing is registered.
    #[must_use]
    pub const fn offer_list(self) -> bool {
        self.offer_list
    }

    /// Reports whether swap listing is registered.
    #[must_use]
    pub const fn swap_list(self) -> bool {
        self.swap_list
    }

    /// Reports whether initiation is registered.
    #[must_use]
    pub const fn initiate(self) -> bool {
        self.initiate
    }

    /// Reports whether monitoring is registered.
    #[must_use]
    pub const fn monitor(self) -> bool {
        self.monitor
    }

    /// Reports whether claim is registered.
    #[must_use]
    pub const fn claim(self) -> bool {
        self.claim
    }

    /// Reports whether refund is registered.
    #[must_use]
    pub const fn refund(self) -> bool {
        self.refund
    }
}

/// Current role-fixed capability of one supported pair.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TakerPairCapabilityV1 {
    pair: Pair,
    supported_direction: SwapDirection,
    authenticated_offer_browsing: bool,
    initiation: TakerInitiationCapabilityV1,
    monitoring: TakerMonitoringCapabilityV1,
    claim: TakerTerminalActionCapabilityV1,
    refund: TakerTerminalActionCapabilityV1,
}

impl TakerPairCapabilityV1 {
    /// Returns the foreign-chain pair.
    #[must_use]
    pub const fn pair(&self) -> Pair {
        self.pair
    }

    /// Returns the only currently composed Taker initiation direction.
    #[must_use]
    pub const fn supported_direction(&self) -> SwapDirection {
        self.supported_direction
    }

    /// Reports whether key-pinned authenticated Delivery browsing exists.
    #[must_use]
    pub const fn authenticated_offer_browsing(&self) -> bool {
        self.authenticated_offer_browsing
    }

    /// Returns the current initiation material boundary.
    #[must_use]
    pub const fn initiation(&self) -> TakerInitiationCapabilityV1 {
        self.initiation
    }

    /// Returns the current lifecycle monitoring boundary.
    #[must_use]
    pub const fn monitoring(&self) -> TakerMonitoringCapabilityV1 {
        self.monitoring
    }

    /// Returns the honest scope of the pair's claim route.
    #[must_use]
    pub const fn claim(&self) -> TakerTerminalActionCapabilityV1 {
        self.claim
    }

    /// Returns the honest scope of the pair's refund route.
    #[must_use]
    pub const fn refund(&self) -> TakerTerminalActionCapabilityV1 {
        self.refund
    }
}

/// Returns the exact current route capabilities in stable pair/direction order.
///
/// Bitcoin and Zcash expose complete receipt-bound lifecycle commands. Monero
/// currently exposes only role-fixed tag-14/tag-16 effect checkpoints; neither
/// checkpoint alone is represented as terminal cross-chain completion.
#[must_use]
pub const fn taker_pair_capabilities_v1() -> [TakerPairCapabilityV1; 4] {
    [
        TakerPairCapabilityV1 {
            pair: Pair::Bitcoin,
            supported_direction: SwapDirection::TakerSellsForeign,
            authenticated_offer_browsing: true,
            initiation: TakerInitiationCapabilityV1::PreparedPrivateMaterial,
            monitoring: TakerMonitoringCapabilityV1::ReceiptBound,
            claim: TakerTerminalActionCapabilityV1::FullLifecycle,
            refund: TakerTerminalActionCapabilityV1::FullLifecycle,
        },
        TakerPairCapabilityV1 {
            pair: Pair::Monero,
            supported_direction: SwapDirection::TakerSellsLez,
            authenticated_offer_browsing: true,
            initiation: TakerInitiationCapabilityV1::PreparedPrivateMaterial,
            monitoring: TakerMonitoringCapabilityV1::ReceiptBound,
            claim: TakerTerminalActionCapabilityV1::EffectCheckpointOnly,
            refund: TakerTerminalActionCapabilityV1::EffectCheckpointOnly,
        },
        TakerPairCapabilityV1 {
            pair: Pair::Zcash,
            supported_direction: SwapDirection::TakerSellsLez,
            authenticated_offer_browsing: true,
            initiation: TakerInitiationCapabilityV1::PreparedPrivateMaterial,
            monitoring: TakerMonitoringCapabilityV1::ReceiptBound,
            claim: TakerTerminalActionCapabilityV1::FullLifecycle,
            refund: TakerTerminalActionCapabilityV1::FullLifecycle,
        },
        TakerPairCapabilityV1 {
            pair: Pair::Zcash,
            supported_direction: SwapDirection::TakerSellsForeign,
            authenticated_offer_browsing: true,
            initiation: TakerInitiationCapabilityV1::PreparedPrivateMaterial,
            monitoring: TakerMonitoringCapabilityV1::ReceiptBound,
            claim: TakerTerminalActionCapabilityV1::FullLifecycle,
            refund: TakerTerminalActionCapabilityV1::FullLifecycle,
        },
    ]
}

/// Versioned secret-free health and capability response.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TakerHealthV1 {
    schema_version: u16,
    ready: bool,
    degraded: bool,
    delivery: TakerDependencyStateV1,
    chat: TakerDependencyStateV1,
    pair_capabilities: [TakerPairCapabilityV1; 4],
    registered_methods: TakerRegisteredMethodsV1,
}

impl TakerHealthV1 {
    /// Builds health from service readiness and current dependency state.
    #[must_use]
    pub const fn new(
        ready: bool,
        delivery: TakerDependencyStateV1,
        chat: TakerDependencyStateV1,
    ) -> Self {
        Self {
            schema_version: TAKER_FACADE_SCHEMA_VERSION_V1,
            ready,
            degraded: matches!(delivery, TakerDependencyStateV1::Unavailable)
                || matches!(chat, TakerDependencyStateV1::Unavailable),
            delivery,
            chat,
            pair_capabilities: taker_pair_capabilities_v1(),
            registered_methods: TakerRegisteredMethodsV1::read_only(),
        }
    }

    /// Reports that the service registered initiation and receipt-bound reads.
    #[must_use]
    pub const fn with_initiation_registered(mut self) -> Self {
        self.registered_methods = TakerRegisteredMethodsV1::read_with_initiation();
        self
    }

    /// Reports that the service registered the complete receipt-bound ZEC lifecycle.
    #[must_use]
    pub const fn with_zec_lifecycle_registered(mut self) -> Self {
        self.registered_methods = TakerRegisteredMethodsV1::full_zec_lifecycle();
        self
    }

    /// Returns the health schema version.
    #[must_use]
    pub const fn schema_version(&self) -> u16 {
        self.schema_version
    }

    /// Returns whether the role-fixed service can accept bounded requests.
    #[must_use]
    pub const fn is_ready(&self) -> bool {
        self.schema_version == TAKER_FACADE_SCHEMA_VERSION_V1 && self.ready
    }

    /// Returns whether a configured dependency currently needs attention.
    #[must_use]
    pub const fn is_degraded(&self) -> bool {
        self.degraded
    }

    /// Returns the configured Delivery dependency state.
    #[must_use]
    pub const fn delivery(&self) -> TakerDependencyStateV1 {
        self.delivery
    }

    /// Returns the configured Chat dependency state.
    #[must_use]
    pub const fn chat(&self) -> TakerDependencyStateV1 {
        self.chat
    }

    /// Returns the exact JSON-RPC methods registered by this service.
    #[must_use]
    pub const fn registered_methods(&self) -> TakerRegisteredMethodsV1 {
        self.registered_methods
    }

    /// Returns all current pair capabilities in stable order.
    #[must_use]
    pub const fn pair_capabilities(&self) -> &[TakerPairCapabilityV1; 4] {
        &self.pair_capabilities
    }
}
