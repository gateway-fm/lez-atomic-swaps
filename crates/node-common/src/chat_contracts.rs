//! Secret-free wire contracts exchanged across Maker and Taker Chat boundaries.

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
use lez_bridge_protocol::RequestId;
use lez_swap_store::MakerOfferId;
use lez_xmr_swap_sdk::{
    MAX_XMR_ACTIVATION_WIRE_BYTES, MAX_XMR_AGREEMENT_WIRE_BYTES,
    MAX_XMR_UNSIGNED_STAGE_A_WIRE_BYTES, MAX_XMR_UNSIGNED_STAGE_B_WIRE_BYTES,
};
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

/// A canonical `RolePacketV1` is bounded like the reference actor reads it (270 KiB), the
/// same bound as a signed Stage A.
const MAX_XMR_ROLE_PACKET_BYTES: usize = MAX_XMR_AGREEMENT_WIRE_BYTES;
/// A canonical adaptor-runner public packet is bounded like the runner reads it (8 KiB).
const MAX_XMR_CEREMONY_PACKET_BYTES: usize = 8 * 1024;
const XMR_SIGNATURE_BYTES: usize = 64;

macro_rules! bounded_base64_module {
    ($name:ident, $maximum:ident) => {
        mod $name {
            use super::{
                Deserializer, Serializer, deserialize_bounded_base64, serialize_bounded_base64,
                $maximum,
            };

            pub fn serialize<S>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                serialize_bounded_base64::<S, $maximum>(bytes, serializer)
            }

            pub fn deserialize<'de, D>(deserializer: D) -> Result<Vec<u8>, D::Error>
            where
                D: Deserializer<'de>,
            {
                deserialize_bounded_base64::<D, $maximum>(deserializer)
            }
        }
    };
}

bounded_base64_module!(role_packet_base64, MAX_XMR_ROLE_PACKET_BYTES);
bounded_base64_module!(unsigned_stage_a_base64, MAX_XMR_UNSIGNED_STAGE_A_WIRE_BYTES);
bounded_base64_module!(unsigned_stage_b_base64, MAX_XMR_UNSIGNED_STAGE_B_WIRE_BYTES);
bounded_base64_module!(ceremony_packet_base64, MAX_XMR_CEREMONY_PACKET_BYTES);

mod signature_base64 {
    use serde::de::Error as _;

    use super::{
        Deserializer, Serializer, XMR_SIGNATURE_BYTES, deserialize_bounded_base64,
        serialize_bounded_base64,
    };

    pub fn serialize<S>(
        signature: &[u8; XMR_SIGNATURE_BYTES],
        serializer: S,
    ) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serialize_bounded_base64::<S, XMR_SIGNATURE_BYTES>(signature, serializer)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<[u8; XMR_SIGNATURE_BYTES], D::Error>
    where
        D: Deserializer<'de>,
    {
        deserialize_bounded_base64::<D, XMR_SIGNATURE_BYTES>(deserializer)?
            .try_into()
            .map_err(|_| D::Error::custom("XMR signature is not 64 bytes"))
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
    /// SHA-256 of the Taker's claim preimage, in both directions (ADR 0216:
    /// the Taker holds the swap secret).
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
    /// Always `None` since ADR 0216 (the Maker holds no preimage); a Taker
    /// refuses an answer that names one.
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

/// `xmr_reserve_v1`: the Taker's facts for a Monero agreement composed at take time. Besides
/// public halves it carries the per-swap private view key, which both roles need to observe the
/// shared Monero output; it opens nothing but this swap's incoming outputs.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct XmrReserveRequestV1 {
    pub schema_version: u16,
    pub reservation_id: RequestId,
    pub offer_id: MakerOfferId,
    pub signed_offer_envelope: Vec<u8>,
    /// Piconero.
    pub foreign_units: u64,
    /// The Taker's canonical `RolePacketV1` JSON.
    #[serde(with = "role_packet_base64")]
    pub taker_packet: Vec<u8>,
    /// Monero little-endian scalar, generated for this swap only.
    pub taker_private_view_key: [u8; 32],
}

/// The Maker's answer: its packet, the unsigned Stage A both roles compose, its Stage-A
/// signature, and the facts only it chose.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct XmrReserveResponseV1 {
    pub schema_version: u16,
    pub was_replay: bool,
    pub swap_id: Box<str>,
    #[serde(with = "role_packet_base64")]
    pub maker_packet: Vec<u8>,
    #[serde(with = "unsigned_stage_a_base64")]
    pub unsigned_stage_a_wire: Vec<u8>,
    #[serde(with = "signature_base64")]
    pub maker_stage_a_signature: [u8; 64],
    pub lez_anchor_unix_seconds: u64,
    pub monero_genesis: [u8; 32],
    pub monero_restore_height: u64,
    pub maker_owner_nonce: u128,
}

/// One canonical adaptor-runner packet per session.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct XmrLegPacketsV1 {
    #[serde(with = "ceremony_packet_base64")]
    pub claim: Vec<u8>,
    #[serde(with = "ceremony_packet_base64")]
    pub refund: Vec<u8>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct XmrCeremonyReserveRequestV1 {
    pub schema_version: u16,
    pub request_id: RequestId,
    pub reservation_id: RequestId,
    pub taker_commitments: XmrLegPacketsV1,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct XmrCeremonyReserveResponseV1 {
    pub schema_version: u16,
    pub was_replay: bool,
    pub maker_commitments: XmrLegPacketsV1,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct XmrCeremonyNonceRequestV1 {
    pub schema_version: u16,
    pub request_id: RequestId,
    pub reservation_id: RequestId,
    pub taker_nonces: XmrLegPacketsV1,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct XmrCeremonyNonceResponseV1 {
    pub schema_version: u16,
    pub was_replay: bool,
    pub maker_nonces: XmrLegPacketsV1,
    pub maker_partials: XmrLegPacketsV1,
}

/// The Taker sends its refund partial only: its claim partial is published on LEZ at tag 14.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct XmrCeremonyPartialRequestV1 {
    pub schema_version: u16,
    pub request_id: RequestId,
    pub reservation_id: RequestId,
    #[serde(with = "ceremony_packet_base64")]
    pub taker_refund_partial: Vec<u8>,
    #[serde(with = "unsigned_stage_b_base64")]
    pub unsigned_stage_b_wire: Vec<u8>,
    #[serde(with = "signature_base64")]
    pub taker_stage_b_signature: [u8; 64],
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct XmrCeremonyPartialResponseV1 {
    pub schema_version: u16,
    pub was_replay: bool,
    #[serde(with = "ceremony_packet_base64")]
    pub refund_presignature: Vec<u8>,
    #[serde(with = "signature_base64")]
    pub maker_stage_b_signature: [u8; 64],
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;

    /// The Chat gateway's frame bound (`logos_chat_gateway::MAXIMUM_FRAME_BYTES`).
    const CHAT_FRAME_BYTES: usize = 1024 * 1024;

    fn request_id(value: &str) -> RequestId {
        RequestId::new(value).expect("bounded request ID")
    }

    fn reserve_request(taker_packet: Vec<u8>) -> XmrReserveRequestV1 {
        XmrReserveRequestV1 {
            schema_version: 1,
            reservation_id: request_id("xmr-reserve-reservation-001"),
            offer_id: MakerOfferId::new("xmr-reserve-offer-001").expect("bounded offer ID"),
            signed_offer_envelope: vec![u8::MAX; 65_536],
            foreign_units: 1_000_000_000_000,
            taker_packet,
            taker_private_view_key: [0x07; 32],
        }
    }

    fn reserve_response(maker_packet: Vec<u8>, unsigned_stage_a: Vec<u8>) -> XmrReserveResponseV1 {
        XmrReserveResponseV1 {
            schema_version: 1,
            was_replay: false,
            swap_id: "ab".repeat(32).into_boxed_str(),
            maker_packet,
            unsigned_stage_a_wire: unsigned_stage_a,
            maker_stage_a_signature: [0x5a; 64],
            lez_anchor_unix_seconds: 1_800_000_000,
            monero_genesis: [0x31; 32],
            monero_restore_height: 120,
            maker_owner_nonce: u128::from(u64::MAX),
        }
    }

    fn packets(byte: u8) -> XmrLegPacketsV1 {
        XmrLegPacketsV1 {
            claim: vec![byte; 300],
            refund: vec![byte.wrapping_add(1); 300],
        }
    }

    fn partial_request(stage_b: Vec<u8>) -> XmrCeremonyPartialRequestV1 {
        XmrCeremonyPartialRequestV1 {
            schema_version: 1,
            request_id: request_id("xmr-ceremony-partial-001"),
            reservation_id: request_id("xmr-reserve-reservation-001"),
            taker_refund_partial: vec![0x22; 300],
            unsigned_stage_b_wire: stage_b,
            taker_stage_b_signature: [0x6b; 64],
        }
    }

    fn round_trip<T>(value: &T) -> Vec<u8>
    where
        T: Serialize + for<'de> Deserialize<'de> + PartialEq + std::fmt::Debug,
    {
        let json = serde_json::to_vec(value).expect("serialize XMR wire");
        let decoded: T = serde_json::from_slice(&json).expect("deserialize XMR wire");
        assert_eq!(&decoded, value);
        json
    }

    #[test]
    fn xmr_take_time_wires_round_trip() {
        round_trip(&reserve_request(vec![0x7b; 4_096]));
        round_trip(&reserve_response(vec![0x7b; 4_096], vec![0x01; 2_048]));
        round_trip(&XmrCeremonyReserveRequestV1 {
            schema_version: 1,
            request_id: request_id("xmr-ceremony-reserve-001"),
            reservation_id: request_id("xmr-reserve-reservation-001"),
            taker_commitments: packets(0x10),
        });
        round_trip(&XmrCeremonyReserveResponseV1 {
            schema_version: 1,
            was_replay: true,
            maker_commitments: packets(0x20),
        });
        round_trip(&XmrCeremonyNonceRequestV1 {
            schema_version: 1,
            request_id: request_id("xmr-ceremony-nonce-001"),
            reservation_id: request_id("xmr-reserve-reservation-001"),
            taker_nonces: packets(0x30),
        });
        round_trip(&XmrCeremonyNonceResponseV1 {
            schema_version: 1,
            was_replay: false,
            maker_nonces: packets(0x40),
            maker_partials: packets(0x50),
        });
        round_trip(&partial_request(vec![0x03; 512]));
        round_trip(&XmrCeremonyPartialResponseV1 {
            schema_version: 1,
            was_replay: false,
            refund_presignature: vec![0x60; 300],
            maker_stage_b_signature: [0x6c; 64],
        });
    }

    #[test]
    fn maximum_xmr_take_time_wires_fit_chat() {
        let request = reserve_request(vec![u8::MAX; MAX_XMR_ROLE_PACKET_BYTES]);
        assert!(round_trip(&request).len() < CHAT_FRAME_BYTES);
        let response = reserve_response(
            vec![u8::MAX; MAX_XMR_ROLE_PACKET_BYTES],
            vec![u8::MAX; MAX_XMR_UNSIGNED_STAGE_A_WIRE_BYTES],
        );
        assert!(round_trip(&response).len() < CHAT_FRAME_BYTES);
        let partial = partial_request(vec![u8::MAX; MAX_XMR_UNSIGNED_STAGE_B_WIRE_BYTES]);
        assert!(round_trip(&partial).len() < CHAT_FRAME_BYTES);
        let nonces = XmrCeremonyNonceResponseV1 {
            schema_version: 1,
            was_replay: false,
            maker_nonces: XmrLegPacketsV1 {
                claim: vec![u8::MAX; MAX_XMR_CEREMONY_PACKET_BYTES],
                refund: vec![u8::MAX; MAX_XMR_CEREMONY_PACKET_BYTES],
            },
            maker_partials: XmrLegPacketsV1 {
                claim: vec![u8::MAX; MAX_XMR_CEREMONY_PACKET_BYTES],
                refund: vec![u8::MAX; MAX_XMR_CEREMONY_PACKET_BYTES],
            },
        };
        assert!(round_trip(&nonces).len() < CHAT_FRAME_BYTES);
    }

    #[test]
    fn oversized_xmr_take_time_wires_are_refused_both_ways() {
        assert!(
            serde_json::to_vec(&reserve_request(vec![0; MAX_XMR_ROLE_PACKET_BYTES + 1])).is_err()
        );
        assert!(
            serde_json::to_vec(&reserve_response(
                vec![0; 1],
                vec![0; MAX_XMR_UNSIGNED_STAGE_A_WIRE_BYTES + 1],
            ))
            .is_err()
        );
        let stage_b = vec![0; MAX_XMR_UNSIGNED_STAGE_B_WIRE_BYTES + 1];
        assert!(serde_json::to_vec(&partial_request(stage_b)).is_err());
        assert!(
            serde_json::to_vec(&XmrLegPacketsV1 {
                claim: vec![0; MAX_XMR_CEREMONY_PACKET_BYTES + 1],
                refund: Vec::new(),
            })
            .is_err()
        );

        let mut request = serde_json::to_value(reserve_request(vec![0; 1])).unwrap();
        request["taker_packet"] =
            json!(BASE64_STANDARD.encode(vec![0; MAX_XMR_ROLE_PACKET_BYTES + 1]));
        assert!(serde_json::from_value::<XmrReserveRequestV1>(request).is_err());
        let mut packets = serde_json::to_value(packets(0)).unwrap();
        packets["refund"] =
            json!(BASE64_STANDARD.encode(vec![0; MAX_XMR_CEREMONY_PACKET_BYTES + 1]));
        assert!(serde_json::from_value::<XmrLegPacketsV1>(packets).is_err());
    }

    #[test]
    fn xmr_take_time_wires_refuse_malformed_fields() {
        let valid = serde_json::to_value(reserve_response(vec![0; 1], vec![0; 1])).unwrap();
        let refused = |field: &str, value: Value| {
            let mut response = valid.clone();
            response[field] = value;
            assert!(
                serde_json::from_value::<XmrReserveResponseV1>(response).is_err(),
                "{field} must be refused"
            );
        };
        refused(
            "maker_stage_a_signature",
            json!(BASE64_STANDARD.encode([0; 63])),
        );
        refused(
            "maker_stage_a_signature",
            json!(BASE64_STANDARD.encode([0; 65])),
        );
        // Base64 with nonzero padding bits is not canonical.
        refused("maker_packet", json!("AB=="));
        refused("unsigned_stage_a_wire", json!([0, 1, 2]));
        refused("monero_genesis", json!(vec![0; 31]));

        let mut unknown = valid;
        unknown["maker_private_view_key"] = json!(vec![0; 32]);
        assert!(serde_json::from_value::<XmrReserveResponseV1>(unknown).is_err());
    }
}
