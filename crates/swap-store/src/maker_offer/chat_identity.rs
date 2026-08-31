//! Pair-domain-separated identifiers derived at the Delivery/Chat boundary.

use lez_bridge_protocol::RequestId;
use sha2::{Digest as _, Sha256};

const ZEC_CHAT_SESSION_DOMAIN: &[u8] = b"lez-atomic-swaps/maker-zec-chat-session/v1";
const BTC_CHAT_SWAP_ID_DOMAIN: &[u8] = b"lez-atomic-swaps/maker-btc-chat-swap-id/v1";
const XMR_CHAT_SWAP_ID_DOMAIN: &[u8] = b"lez-atomic-swaps/maker-xmr-chat-swap-id/v1";

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
