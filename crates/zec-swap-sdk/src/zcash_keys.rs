//! The boundary between the workspace's secp256k1 keys and librustzcash's.
//!
//! The workspace signs and verifies with `secp256k1` 0.29 (shared with the
//! Bitcoin corridor); `zcash_transparent` 0.11 takes its keys from `secp256k1`
//! 0.33. The two are the same curve and the same encodings, so keys cross the
//! boundary as their canonical bytes and never as a shared type.

use secp256k1::{PublicKey, SecretKey};
use zcash_transparent::address::TransparentAddress;

/// Returns the P2PKH address of a public key, as `zcash_transparent` derives it.
#[must_use]
pub fn p2pkh_address(public_key: &PublicKey) -> TransparentAddress {
    TransparentAddress::from_pubkey(&zcash_public_key(public_key))
}

/// Returns the 20-byte P2PKH hash of a public key.
#[must_use]
pub fn p2pkh_hash(public_key: &PublicKey) -> [u8; 20] {
    match p2pkh_address(public_key) {
        TransparentAddress::PublicKeyHash(hash) => hash,
        TransparentAddress::ScriptHash(_) => unreachable!("public keys always yield P2PKH"),
    }
}

pub(crate) fn zcash_public_key(public_key: &PublicKey) -> secp256k1_zcash::PublicKey {
    secp256k1_zcash::PublicKey::from_slice(&public_key.serialize())
        .expect("a valid compressed secp256k1 key is valid in every secp256k1 release")
}

pub(crate) fn zcash_secret_key(secret_key: &SecretKey) -> secp256k1_zcash::SecretKey {
    secp256k1_zcash::SecretKey::from_secret_bytes(secret_key.secret_bytes())
        .expect("a valid secp256k1 scalar is valid in every secp256k1 release")
}
