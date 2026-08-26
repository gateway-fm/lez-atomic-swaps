//! Monero Chat staging and daemon-owned Stage-B activation.

use std::collections::BTreeMap;

use anyhow::{Context as _, ensure};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
use serde::{Deserializer, Serializer, de::Error as _};

use super::*;

const MAX_XMR_MAKER_AUTHORITIES: usize = 256;

fn serialize_bounded_base64<S, const MAXIMUM: usize>(
    bytes: &[u8],
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    if bytes.len() > MAXIMUM {
        return Err(serde::ser::Error::custom(
            "XMR wire exceeds its binary bound",
        ));
    }
    serializer.serialize_str(&BASE64_STANDARD.encode(bytes))
}

fn deserialize_bounded_base64<'de, D, const MAXIMUM: usize>(
    deserializer: D,
) -> Result<Vec<u8>, D::Error>
where
    D: Deserializer<'de>,
{
    let encoded = Box::<str>::deserialize(deserializer)?;
    if encoded.len() > MAXIMUM.saturating_add(2) / 3 * 4 {
        return Err(D::Error::custom("XMR wire exceeds its encoded bound"));
    }
    let bytes = BASE64_STANDARD
        .decode(encoded.as_bytes())
        .map_err(|_| D::Error::custom("XMR wire is not canonical Base64"))?;
    if bytes.len() > MAXIMUM || BASE64_STANDARD.encode(&bytes).as_str() != encoded.as_ref() {
        return Err(D::Error::custom(
            "XMR wire exceeds its binary bound or is noncanonical",
        ));
    }
    Ok(bytes)
}

mod stage_a_wire_base64 {
    use super::{
        Deserializer, MAX_XMR_AGREEMENT_WIRE_BYTES, Serializer, deserialize_bounded_base64,
        serialize_bounded_base64,
    };

    pub fn serialize<S>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serialize_bounded_base64::<S, MAX_XMR_AGREEMENT_WIRE_BYTES>(bytes, serializer)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Vec<u8>, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserialize_bounded_base64::<D, MAX_XMR_AGREEMENT_WIRE_BYTES>(deserializer)
    }
}

mod activation_wire_base64 {
    use super::{
        Deserializer, MAX_XMR_ACTIVATION_WIRE_BYTES, Serializer, deserialize_bounded_base64,
        serialize_bounded_base64,
    };

    pub fn serialize<S>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serialize_bounded_base64::<S, MAX_XMR_ACTIVATION_WIRE_BYTES>(bytes, serializer)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Vec<u8>, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserialize_bounded_base64::<D, MAX_XMR_ACTIVATION_WIRE_BYTES>(deserializer)
    }
}

/// Daemon-owned XMR role material and immutable actor manifests.
///
/// The private view key validates Stage B but is never serialized or exposed
/// through `Debug`. Actor paths remain behind this authority boundary.
pub struct XmrMakerChatAuthority {
    maker_agreement_identity: [u8; 33],
    private_view_key: MoneroPrivateViewKey,
    actors: BTreeMap<Box<str>, MakerActorManifestV1>,
}

impl std::fmt::Debug for XmrMakerChatAuthority {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("XmrMakerChatAuthority")
            .field("maker_agreement_identity", &self.maker_agreement_identity)
            .field("private_view_key", &"[REDACTED]")
            .field("actor_count", &self.actors.len())
            .finish()
    }
}

impl XmrMakerChatAuthority {
    /// Validates one bounded daemon-owned authority registry.
    ///
    /// # Errors
    ///
    /// Rejects an invalid Maker key, an empty or oversized registry, duplicate
    /// swap identities, or a non-Monero actor manifest.
    pub fn new(
        maker_agreement_identity: [u8; 33],
        private_view_key: MoneroPrivateViewKey,
        actors: impl IntoIterator<Item = MakerActorManifestV1>,
    ) -> anyhow::Result<Self> {
        PublicKey::from_slice(&maker_agreement_identity)
            .context("XMR Maker agreement identity is invalid")?;
        let mut indexed = BTreeMap::new();
        for actor in actors {
            ensure!(
                actor.kind() == MakerActorKindV1::Monero,
                "XMR Chat authority contains a non-Monero actor"
            );
            ensure!(
                indexed
                    .insert(actor.swap_id().as_str().into(), actor)
                    .is_none(),
                "XMR Chat authority contains a duplicate swap identity"
            );
            ensure!(
                indexed.len() <= MAX_XMR_MAKER_AUTHORITIES,
                "XMR Chat authority registry is oversized"
            );
        }
        ensure!(!indexed.is_empty(), "XMR Chat authority registry is empty");
        Ok(Self {
            maker_agreement_identity,
            private_view_key,
            actors: indexed,
        })
    }

    fn actor_for(&self, swap_id: &SwapId) -> Option<&MakerActorManifestV1> {
        self.actors.get(swap_id.as_str())
    }

    fn supports_agreement(&self, agreement: &XmrAgreementV1) -> bool {
        let body = agreement.body();
        let maker = body.participants().for_role(XmrRoleV1::Maker);
        let Ok(swap_id) = SwapId::new(hex::encode(body.swap_id())) else {
            return false;
        };
        maker.agreement_public_key() == self.maker_agreement_identity
            && body.monero().public_view_key() == self.private_view_key.public_key()
            && self.actor_for(&swap_id).is_some()
    }
}

/// Taker request carrying one canonical dual-signed XMR Stage-A agreement.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct XmrChatStageARequestV1 {
    /// Must be one for this DTO shape.
    pub schema_version: u16,
    /// Global exact-replay identity for reservation.
    pub request_id: RequestId,
    /// Selected immutable offer identity.
    pub offer_id: MakerOfferId,
    /// Current active offer revision, normally one.
    pub expected_offer_revision: u64,
    /// Winning reservation and Chat-session identity.
    pub reservation_id: RequestId,
    /// Exact selected Monero amount in piconero.
    pub foreign_units: u64,
    /// Exact signed Delivery envelope authenticated by the Taker.
    pub signed_offer_envelope: Vec<u8>,
    /// Canonical dual-signed Stage-A agreement.
    #[serde(with = "stage_a_wire_base64")]
    pub stage_a_wire: Vec<u8>,
}

/// Secret-free durable result of XMR Stage-A reservation.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct XmrChatStageAResponseV1 {
    /// Response schema version.
    pub schema_version: u16,
    /// Durable reserved offer revision.
    pub offer_revision: u64,
    /// Whether the exact request was already committed.
    pub was_replay: bool,
    /// Winning reservation identity.
    pub reservation_id: RequestId,
    /// Exact no-rounding LEZ amount.
    pub lez_units: u128,
    /// Delivery-and-reservation-derived public swap identity.
    pub swap_id: Box<str>,
    /// Canonical Stage-A commitment signed by both roles.
    pub agreement_commitment: [u8; 32],
}

/// Taker request carrying canonical countersigned XMR Stage B.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct XmrChatActivateRequestV1 {
    /// Must be one for this DTO shape.
    pub schema_version: u16,
    /// Global exact-replay identity for atomic activation.
    pub request_id: RequestId,
    /// Reserved immutable offer identity.
    pub offer_id: MakerOfferId,
    /// Current reserved offer revision, normally two.
    pub expected_offer_revision: u64,
    /// Winning reservation identity.
    pub reservation_id: RequestId,
    /// Canonical dual-signed Stage-B activation wire.
    #[serde(with = "activation_wire_base64")]
    pub activation_wire: Vec<u8>,
}

/// Secret-free durable result returned after Stage B and actor registration commit.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct XmrChatActivateResponseV1 {
    /// Response schema version.
    pub schema_version: u16,
    /// Durable consumed offer revision.
    pub offer_revision: u64,
    /// Whether the exact activation request was already committed.
    pub was_replay: bool,
    /// SDK-derived application swap identity.
    pub swap_id: Box<str>,
    /// Canonical Stage-B commitment signed by both roles.
    pub activation_commitment: [u8; 32],
}

pub(super) fn register_xmr_chat_methods(module: &mut RpcModule<MakerRpc>) -> anyhow::Result<()> {
    module.register_blocking_method::<RpcResult<XmrChatStageAResponseV1>, _>(
        "xmr_chat_stage_a_v1",
        |params, context, _| {
            let request: XmrChatStageARequestV1 = params.one()?;
            stage_xmr_chat(&request, &context)
        },
    )?;
    module.register_blocking_method::<RpcResult<XmrChatActivateResponseV1>, _>(
        "xmr_chat_activate_v1",
        |params, context, _| {
            let request: XmrChatActivateRequestV1 = params.one()?;
            activate_xmr_chat(&request, &context)
        },
    )?;
    Ok(())
}

const fn offer_time_is_acceptable(
    has_durable_negotiation: bool,
    created_at: u64,
    expires_at: u64,
    now: u64,
) -> bool {
    has_durable_negotiation || (created_at <= now && now < expires_at)
}

#[allow(clippy::too_many_lines)]
fn stage_xmr_chat(
    request: &XmrChatStageARequestV1,
    context: &MakerRpc,
) -> RpcResult<XmrChatStageAResponseV1> {
    if request.schema_version != 1
        || request.foreign_units == 0
        || request.stage_a_wire.is_empty()
        || request.stage_a_wire.len() > MAX_XMR_AGREEMENT_WIRE_BYTES
    {
        return Err(invalid_request("unsupported or empty XMR Chat Stage A"));
    }
    let delivery = context
        .delivery
        .as_ref()
        .ok_or_else(|| invalid_request("maker Chat Delivery is unavailable"))?;
    let authority = context
        .xmr_chat_authority
        .as_ref()
        .ok_or_else(|| invalid_request("maker XMR Chat authority is unavailable"))?;
    let existing = {
        let store = context
            .store
            .lock()
            .map_err(|_| rpc_error(INTERNAL_ERROR, "swap store lock poisoned"))?;
        store
            .load_xmr_maker_negotiation(&request.offer_id)
            .map_err(application_store_error)?
    };
    let authenticated = delivery
        .authenticate_envelope(&request.signed_offer_envelope)
        .map_err(|error| invalid_request(error.to_string()))?;
    let offer = authenticated.offer();
    let agreement = XmrAgreementV1::from_wire(&request.stage_a_wire).map_err(invalid_request)?;
    let lez_units = offer
        .quote_foreign_amount(request.foreign_units)
        .map_err(invalid_request)?;
    let offer_commitment = authenticated.commitment();
    let now_unix_seconds = trusted_now_unix_seconds()?;
    if offer.id() != &request.offer_id
        || offer.route().pair() != Pair::Monero
        || offer.route().direction() != SwapDirection::TakerSellsLez
        || !offer_time_is_acceptable(
            existing.is_some(),
            offer.created_at_unix_seconds(),
            offer.expires_at_unix_seconds(),
            now_unix_seconds,
        )
        || agreement.body().direction() != XmrSwapDirectionV1::TakerSellsLez
        || agreement.body().swap_id()
            != maker_xmr_chat_swap_id(&offer_commitment, &request.reservation_id)
        || agreement.body().monero().amount_piconero() != request.foreign_units
        || agreement.body().lez().amount() != lez_units
        || !authority.supports_agreement(&agreement)
    {
        return Err(invalid_request(
            "XMR Stage A is not bound to the selected offer and daemon authority",
        ));
    }

    let mut store = context
        .store
        .lock()
        .map_err(|_| rpc_error(INTERNAL_ERROR, "swap store lock poisoned"))?;
    let reserved_at = existing.as_ref().map_or(
        now_unix_seconds,
        MakerXmrNegotiationV1::reserved_at_unix_seconds,
    );
    let candidate = MakerXmrNegotiationV1::stage_a(
        request.reservation_id.clone(),
        offer_commitment,
        request.foreign_units,
        lez_units,
        reserved_at,
        request.stage_a_wire.clone(),
    )
    .map_err(invalid_request)?;
    if let Some(commit) = store
        .preflight_maker_xmr_stage_a_replay(
            &request.request_id,
            &request.offer_id,
            request.expected_offer_revision,
            &candidate,
        )
        .map_err(application_store_error)?
    {
        return Ok(XmrChatStageAResponseV1 {
            schema_version: 1,
            offer_revision: commit.revision(),
            was_replay: true,
            reservation_id: request.reservation_id.clone(),
            lez_units,
            swap_id: hex::encode(agreement.body().swap_id()).into(),
            agreement_commitment: agreement.agreement_commitment(),
        });
    }
    let commit = store
        .stage_xmr_maker_negotiation(
            &request.request_id,
            &request.offer_id,
            request.expected_offer_revision,
            &candidate,
        )
        .map_err(application_store_error)?;
    Ok(XmrChatStageAResponseV1 {
        schema_version: 1,
        offer_revision: commit.revision(),
        was_replay: commit.was_replay(),
        reservation_id: request.reservation_id.clone(),
        lez_units,
        swap_id: hex::encode(agreement.body().swap_id()).into(),
        agreement_commitment: agreement.agreement_commitment(),
    })
}

fn activate_xmr_chat(
    request: &XmrChatActivateRequestV1,
    context: &MakerRpc,
) -> RpcResult<XmrChatActivateResponseV1> {
    if request.schema_version != 1
        || request.activation_wire.is_empty()
        || request.activation_wire.len() > MAX_XMR_ACTIVATION_WIRE_BYTES
    {
        return Err(invalid_request("unsupported or empty XMR Chat Stage B"));
    }
    let authority = context
        .xmr_chat_authority
        .as_ref()
        .ok_or_else(|| invalid_request("maker XMR Chat authority is unavailable"))?;
    let now_unix_seconds = trusted_now_unix_seconds()?;
    let mut store = context
        .store
        .lock()
        .map_err(|_| rpc_error(INTERNAL_ERROR, "swap store lock poisoned"))?;
    let staged = store
        .load_xmr_maker_negotiation(&request.offer_id)
        .map_err(application_store_error)?
        .ok_or_else(|| rpc_error(NOT_FOUND, "XMR Stage A is unavailable"))?;
    if staged.reservation_id() != &request.reservation_id {
        return Err(invalid_request(
            "XMR Stage B does not match the durable reservation",
        ));
    }
    let agreement = XmrAgreementV1::from_wire(staged.stage_a_wire()).map_err(invalid_request)?;
    if !authority.supports_agreement(&agreement) {
        return Err(invalid_request(
            "durable XMR Stage A does not match daemon authority",
        ));
    }
    let activation = XmrActivatedAgreementV1::from_wire(
        &agreement,
        &request.activation_wire,
        &authority.private_view_key,
    )
    .map_err(invalid_request)?;
    let initial = activation
        .initial_coordinator(&agreement)
        .map_err(invalid_request)?;
    let actor = authority
        .actor_for(initial.id())
        .ok_or_else(|| rpc_error(INTERNAL_ERROR, "maker XMR actor authority is unavailable"))?
        .clone();
    let accepted_at = match staged.status() {
        MakerXmrNegotiationStatus::StageAAccepted => now_unix_seconds,
        MakerXmrNegotiationStatus::Activated => staged
            .activated_at_unix_seconds()
            .ok_or_else(|| rpc_error(INTERNAL_ERROR, "XMR activation state is corrupt"))?,
    };
    let accepted = MakerXmrActivationAcceptance::new(
        &initial,
        Participant::Maker,
        &agreement,
        &activation,
        request.activation_wire.clone(),
        accepted_at,
    )
    .map_err(invalid_request)?;
    let commit = store
        .complete_maker_xmr_negotiation_and_register_actor(
            &request.request_id,
            &request.offer_id,
            request.expected_offer_revision,
            &request.reservation_id,
            &accepted,
            &initial,
            &actor,
            accepted_at,
        )
        .map_err(application_store_error)?;
    Ok(XmrChatActivateResponseV1 {
        schema_version: 1,
        offer_revision: commit.offer_revision(),
        was_replay: commit.was_replay(),
        swap_id: initial.id().as_str().into(),
        activation_commitment: activation.activation_commitment(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request_id(value: &str) -> RequestId {
        RequestId::new(value).expect("bounded request ID")
    }

    fn offer_id(value: &str) -> MakerOfferId {
        MakerOfferId::new(value).expect("bounded offer ID")
    }

    #[test]
    fn live_offer_window_applies_only_before_durable_stage_a() {
        assert!(offer_time_is_acceptable(false, 100, 200, 100));
        assert!(!offer_time_is_acceptable(false, 100, 200, 200));
        assert!(offer_time_is_acceptable(true, 100, 200, 200));
        assert!(offer_time_is_acceptable(true, 100, 200, u64::MAX));
    }

    #[test]
    fn responses_are_explicitly_secret_free() {
        let stage = XmrChatStageAResponseV1 {
            schema_version: 1,
            offer_revision: 2,
            was_replay: false,
            reservation_id: request_id("xmr-chat-secret-free-reservation-001"),
            lez_units: 42,
            swap_id: "11".repeat(32).into(),
            agreement_commitment: [0x22; 32],
        };
        let activation = XmrChatActivateResponseV1 {
            schema_version: 1,
            offer_revision: 3,
            was_replay: false,
            swap_id: "11".repeat(32).into(),
            activation_commitment: [0x33; 32],
        };
        for encoded in [
            serde_json::to_string(&stage).unwrap(),
            serde_json::to_string(&activation).unwrap(),
        ] {
            for forbidden in ["private", "path", "manifest", "actor", "wire"] {
                assert!(!encoded.contains(forbidden), "response leaked {forbidden}");
            }
        }
    }

    fn authority_manifest(kind: MakerActorKindV1) -> MakerActorManifestV1 {
        MakerActorManifestV1::new(
            SwapId::new("11".repeat(32)).unwrap(),
            kind,
            "/tmp/xmr-maker-config.json".into(),
            [0x44; 32],
            "/tmp/xmr-maker-program".into(),
            [0x55; 32],
            "/tmp/xmr-maker-state.sqlite3".into(),
        )
        .unwrap()
    }

    fn authority_view_key() -> MoneroPrivateViewKey {
        let mut bytes = [0; 32];
        bytes[0] = 17;
        MoneroPrivateViewKey::from_monero_little_endian(bytes).unwrap()
    }

    fn maker_public_key() -> [u8; 33] {
        let secret = SecretKey::from_slice(&[7; 32]).unwrap();
        PublicKey::from_secret_key(&Secp256k1::signing_only(), &secret).serialize()
    }

    #[test]
    fn authority_is_monero_only_and_debug_redacts_private_deployment() {
        assert!(
            XmrMakerChatAuthority::new(
                maker_public_key(),
                authority_view_key(),
                [authority_manifest(MakerActorKindV1::Bitcoin)],
            )
            .is_err()
        );
        let authority = XmrMakerChatAuthority::new(
            maker_public_key(),
            authority_view_key(),
            [authority_manifest(MakerActorKindV1::Monero)],
        )
        .unwrap();
        let debug = format!("{authority:?}");
        assert!(debug.contains("[REDACTED]"));
        assert!(!debug.contains("/tmp/"));
        assert!(!debug.contains("sqlite"));
    }

    #[test]
    fn maximum_stage_a_wire_round_trips_below_chat_transport_cap() {
        let request = XmrChatStageARequestV1 {
            schema_version: 1,
            request_id: request_id("xmr-chat-size-stage-001"),
            offer_id: offer_id("xmr-chat-size-offer-001"),
            expected_offer_revision: 1,
            reservation_id: request_id("xmr-chat-size-reservation-001"),
            foreign_units: 1,
            signed_offer_envelope: vec![u8::MAX; 65_536],
            stage_a_wire: vec![u8::MAX; MAX_XMR_AGREEMENT_WIRE_BYTES],
        };
        let json = serde_json::to_vec(&request).expect("serialize maximum Stage A");
        assert!(
            json.len() < 1024 * 1024,
            "maximum binary wires plus worst-case Delivery JSON must fit Chat"
        );
        let decoded: XmrChatStageARequestV1 =
            serde_json::from_slice(&json).expect("round-trip maximum Stage A");
        assert_eq!(decoded.stage_a_wire, request.stage_a_wire);
        assert_eq!(decoded.signed_offer_envelope, request.signed_offer_envelope);
    }

    #[test]
    fn maximum_activation_wire_round_trips_canonically() {
        let request = XmrChatActivateRequestV1 {
            schema_version: 1,
            request_id: request_id("xmr-chat-size-activate-001"),
            offer_id: offer_id("xmr-chat-size-offer-002"),
            expected_offer_revision: 2,
            reservation_id: request_id("xmr-chat-size-reservation-002"),
            activation_wire: vec![0xa5; MAX_XMR_ACTIVATION_WIRE_BYTES],
        };
        let json = serde_json::to_vec(&request).expect("serialize maximum Stage B");
        let decoded: XmrChatActivateRequestV1 =
            serde_json::from_slice(&json).expect("round-trip maximum Stage B");
        assert_eq!(decoded.activation_wire, request.activation_wire);
    }

    #[test]
    fn oversized_wires_are_rejected_before_transport() {
        let stage_a = XmrChatStageARequestV1 {
            schema_version: 1,
            request_id: request_id("xmr-chat-oversize-stage-001"),
            offer_id: offer_id("xmr-chat-oversize-offer-001"),
            expected_offer_revision: 1,
            reservation_id: request_id("xmr-chat-oversize-reservation-001"),
            foreign_units: 1,
            signed_offer_envelope: Vec::new(),
            stage_a_wire: vec![0; MAX_XMR_AGREEMENT_WIRE_BYTES + 1],
        };
        assert!(serde_json::to_vec(&stage_a).is_err());
    }
}
