//! Secret-free wire contracts exchanged across Maker and Taker Chat boundaries.

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
use lez_bridge_protocol::RequestId;
use lez_swap_store::MakerOfferId;
use lez_xmr_swap_sdk::{MAX_XMR_ACTIVATION_WIRE_BYTES, MAX_XMR_AGREEMENT_WIRE_BYTES};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error as _};

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

/// One transparent Zcash outpoint, its transaction id in RPC display order.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ZecOutpointV1 {
    pub transaction_id: [u8; 32],
    pub output_index: u32,
}

/// `zec_reserve_v1`: the Taker's facts for a Zcash agreement composed at
/// take time (docs/sdk-lez-zec.md). Public halves only.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ZecReserveRequestV1 {
    pub schema_version: u16,
    pub reservation_id: RequestId,
    pub offer_id: MakerOfferId,
    pub signed_offer_envelope: Vec<u8>,
    pub foreign_units: u64,
    pub taker_zcash_public_key: Vec<u8>,
    pub taker_lez_account: [u8; 32],
    /// `TakerSellsForeign` only: the Taker's funding outpoint.
    pub taker_funding: Option<ZecOutpointV1>,
    /// `TakerSellsForeign` only: SHA-256 of the Taker's claim preimage.
    pub taker_secret_digest: Option<[u8; 32]>,
}

/// The Maker's answer: the unsigned draft and the facts only it chose.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ZecReserveResponseV1 {
    pub schema_version: u16,
    pub was_replay: bool,
    pub swap_id: Box<str>,
    pub unsigned_draft_wire: Vec<u8>,
    pub maker_lez_account: [u8; 32],
    pub zcash_anchor_height: u32,
    pub lez_anchor_unix_seconds: u64,
    pub zebra_genesis: [u8; 32],
    /// `TakerSellsLez` only: the Maker's funding outpoint.
    pub maker_funding: Option<ZecOutpointV1>,
    /// `TakerSellsLez` only: SHA-256 of the Maker's claim preimage.
    pub maker_secret_digest: Option<[u8; 32]>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ZecChatProposeRequestV1 {
    pub schema_version: u16,
    pub request_id: RequestId,
    pub offer_id: MakerOfferId,
    pub expected_offer_revision: u64,
    pub reservation_id: RequestId,
    pub foreign_units: u64,
    pub signed_offer_envelope: Vec<u8>,
    pub unsigned_draft_wire: Vec<u8>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ZecChatProposalV1 {
    pub schema_version: u16,
    pub offer_revision: u64,
    pub was_replay: bool,
    pub reservation_id: RequestId,
    pub lez_units: u128,
    pub maker_identity: Vec<u8>,
    pub taker_identity: Vec<u8>,
    pub agreement_commitment: [u8; 32],
    pub proposal_wire: Vec<u8>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ZecChatCompleteRequestV1 {
    pub schema_version: u16,
    pub request_id: RequestId,
    pub offer_id: MakerOfferId,
    pub expected_offer_revision: u64,
    pub reservation_id: RequestId,
    pub final_agreement_wire: Vec<u8>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ZecChatCompleteResponseV1 {
    pub schema_version: u16,
    pub offer_revision: u64,
    pub was_replay: bool,
    pub swap_id: Box<str>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BtcChatProposeRequestV1 {
    pub schema_version: u16,
    pub request_id: RequestId,
    pub offer_id: MakerOfferId,
    pub expected_offer_revision: u64,
    pub reservation_id: RequestId,
    pub foreign_units: u64,
    pub signed_offer_envelope: Vec<u8>,
    pub unsigned_draft_wire: Vec<u8>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BtcChatProposeRequestV2 {
    pub schema_version: u16,
    pub request_id: RequestId,
    pub offer_id: MakerOfferId,
    pub expected_offer_revision: u64,
    pub reservation_id: RequestId,
    pub foreign_units: u64,
    pub signed_offer_envelope: Vec<u8>,
    pub maker_contribution_wire: Vec<u8>,
    pub taker_contribution_wire: Vec<u8>,
    pub unsigned_draft_wire: Vec<u8>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BtcChatProposalV1 {
    pub schema_version: u16,
    pub offer_revision: u64,
    pub was_replay: bool,
    pub reservation_id: RequestId,
    pub lez_units: u128,
    pub maker_identity: Vec<u8>,
    pub taker_identity: Vec<u8>,
    pub agreement_commitment: [u8; 32],
    pub proposal_wire: Vec<u8>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BtcChatProposalV2 {
    pub schema_version: u16,
    pub offer_revision: u64,
    pub was_replay: bool,
    pub reservation_id: RequestId,
    pub lez_units: u128,
    pub maker_identity: Vec<u8>,
    pub taker_identity: Vec<u8>,
    pub joint_swap_id: [u8; 32],
    pub maker_contribution_commitment: [u8; 32],
    pub taker_contribution_commitment: [u8; 32],
    pub agreement_commitment: [u8; 32],
    pub proposal_wire: Vec<u8>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BtcChatCompleteRequestV1 {
    pub schema_version: u16,
    pub request_id: RequestId,
    pub offer_id: MakerOfferId,
    pub expected_offer_revision: u64,
    pub reservation_id: RequestId,
    pub final_agreement_wire: Vec<u8>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BtcChatCompleteRequestV2 {
    pub schema_version: u16,
    pub request_id: RequestId,
    pub offer_id: MakerOfferId,
    pub expected_offer_revision: u64,
    pub reservation_id: RequestId,
    pub final_agreement_wire: Vec<u8>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BtcChatCompleteResponseV1 {
    pub schema_version: u16,
    pub offer_revision: u64,
    pub was_replay: bool,
    pub swap_id: Box<str>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BtcChatCompleteResponseV2 {
    pub schema_version: u16,
    pub offer_revision: u64,
    pub was_replay: bool,
    pub swap_id: Box<str>,
    pub maker_role_bound: bool,
    pub ready_for_public_effects: bool,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct XmrChatStageARequestV1 {
    pub schema_version: u16,
    pub request_id: RequestId,
    pub offer_id: MakerOfferId,
    pub expected_offer_revision: u64,
    pub reservation_id: RequestId,
    pub foreign_units: u64,
    pub signed_offer_envelope: Vec<u8>,
    #[serde(with = "stage_a_wire_base64")]
    pub stage_a_wire: Vec<u8>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct XmrChatStageAResponseV1 {
    pub schema_version: u16,
    pub offer_revision: u64,
    pub was_replay: bool,
    pub reservation_id: RequestId,
    pub lez_units: u128,
    pub swap_id: Box<str>,
    pub agreement_commitment: [u8; 32],
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct XmrChatActivateRequestV1 {
    pub schema_version: u16,
    pub request_id: RequestId,
    pub offer_id: MakerOfferId,
    pub expected_offer_revision: u64,
    pub reservation_id: RequestId,
    #[serde(with = "activation_wire_base64")]
    pub activation_wire: Vec<u8>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct XmrChatActivateResponseV1 {
    pub schema_version: u16,
    pub offer_revision: u64,
    pub was_replay: bool,
    pub swap_id: Box<str>,
    pub activation_commitment: [u8; 32],
}
