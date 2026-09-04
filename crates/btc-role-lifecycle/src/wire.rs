//! Secret-free Chat contracts for the Node-owned Bitcoin lifecycle
//! (ADR 0213, S1.3). The Taker sends every request; the Maker answers.
//!
//! Wires and packets are raw bytes; the Maker validates each one against its
//! own derivation before persisting anything.

use lez_bridge_protocol::RequestId;
use lez_swap_core::SwapDirection;
use serde::{Deserialize, Serialize};

/// The Taker's proposal of the swap's public plan, validated by the Maker
/// against its own policy and chain view.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BtcSwapPlanV1 {
    pub foreign_units: u64,
    pub lez_units: u128,
    pub refund_csv_blocks: u32,
    pub claim_fee_sat: u64,
    pub lez_refund_at_ms: u64,
    pub maker_second_lock_cutoff_unix_seconds: u64,
    pub earlier_refund_latest_unix_seconds: u64,
    pub later_refund_earliest_unix_seconds: u64,
    pub required_margin_seconds: u64,
    /// The bridge run id both swap sidecars run under: `swap_run_id(reservation_id)`.
    pub bridge_run_id: String,
    /// Bitcoin facts when the Taker funds Bitcoin; `None` otherwise.
    pub taker_bitcoin_funding: Option<BtcFundingFactsV1>,
}

/// The funder's Bitcoin facts the peer cannot derive alone.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BtcFundingFactsV1 {
    pub transaction_id: [u8; 32],
    pub output_index: u32,
    pub value_sat: u64,
    pub anchor_height: u32,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BtcReserveRequestV1 {
    pub schema_version: u16,
    pub request_id: RequestId,
    /// The Maker offer id as published over Delivery.
    pub offer_id: Box<str>,
    pub expected_offer_revision: u64,
    pub reservation_id: RequestId,
    pub direction: SwapDirection,
    pub signed_offer_envelope: Vec<u8>,
    pub taker_contribution_wire: Vec<u8>,
    pub plan: BtcSwapPlanV1,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BtcReserveResponseV1 {
    pub schema_version: u16,
    pub was_replay: bool,
    pub maker_contribution_wire: Vec<u8>,
    /// Present when the Maker funds Bitcoin.
    pub maker_bitcoin_funding: Option<BtcFundingFactsV1>,
    /// Present when the Maker is the LEZ claimant: the claim message hash its
    /// sidecar prepared, which the draft binds.
    pub maker_claim_message_hash: Option<[u8; 32]>,
    /// The joint swap id both contributions derive.
    pub swap_id: [u8; 32],
}

/// One packet per leg, canonical adaptor-runner JSON bytes.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LegPacketsV1 {
    pub bitcoin: Vec<u8>,
    pub lez: Vec<u8>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BtcCeremonyReserveRequestV1 {
    pub schema_version: u16,
    pub request_id: RequestId,
    pub reservation_id: RequestId,
    pub bitcoin_session_id: [u8; 32],
    pub lez_session_id: [u8; 32],
    /// The claimant's `PrepareWitnessedClaimResult` JSON when the Taker is
    /// the LEZ claimant; `None` when the Maker is (it answers with its own).
    pub prepared_claim_result: Option<Vec<u8>>,
    pub taker_commitments: LegPacketsV1,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BtcCeremonyReserveResponseV1 {
    pub schema_version: u16,
    pub was_replay: bool,
    pub maker_commitments: LegPacketsV1,
    /// The Maker's final prepared claim when it is the LEZ claimant.
    pub prepared_claim_result: Option<Vec<u8>>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BtcCeremonyNonceRequestV1 {
    pub schema_version: u16,
    pub request_id: RequestId,
    pub reservation_id: RequestId,
    pub taker_nonces: LegPacketsV1,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BtcCeremonyNonceResponseV1 {
    pub schema_version: u16,
    pub was_replay: bool,
    pub maker_nonces: LegPacketsV1,
    pub maker_partials: LegPacketsV1,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BtcCeremonyPartialRequestV1 {
    pub schema_version: u16,
    pub request_id: RequestId,
    pub reservation_id: RequestId,
    pub taker_partials: LegPacketsV1,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BtcCeremonyPartialResponseV1 {
    pub schema_version: u16,
    pub was_replay: bool,
    pub presignatures: LegPacketsV1,
    /// The Maker activated its actor with this ceremony's material.
    pub maker_actor_activated: bool,
}

/// Method names both gateways allow through.
pub const BTC_LIFECYCLE_METHODS_V1: [&str; 4] = [
    "btc_reserve_v1",
    "btc_ceremony_reserve_v1",
    "btc_ceremony_nonce_v1",
    "btc_ceremony_partial_v1",
];
