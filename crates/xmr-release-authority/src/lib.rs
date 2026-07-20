//! Durable Stage-B-bound at-most-once release authority.
//!
//! A concrete issuer consumes validated Stage B, canonical finalized LEZ Fund
//! evidence, an exact Monero output observation, and its authenticated topology
//! attestation before it can feed encrypted publication storage and the
//! crash-safe compare-and-swap journal. Live finalized-clock, node-submission,
//! claim-finality, and actor integration remain pending.
//!
//! # `PoC` trust and rollback preconditions
//!
//! The cross-crate prepared-authorization extraction is accepted only for the
//! trusted single-process `PoC`. The generic sidecar route rejects it and node
//! access stays isolated; production moves extraction into a dedicated release
//! service so the actor never receives signed authorization bytes.
//! This journal is safe only on one trusted local filesystem under one service
//! UID, with one canonical journal path and no concurrent clone, restore, or
//! backup rollback. Owner-private modes and `NOFOLLOW` reject many accidental
//! aliases; they do not defend against a hostile same-UID process racing `SQLite`
//! WAL/SHM paths. AEAD and HMAC detect substitution under the current key, but a
//! restored older database contains formerly valid authenticators and can roll
//! protocol state back. Operators must never restore or clone a live journal,
//! especially after publication has started. Production requires an external
//! monotonic/replicated rollback anchor and typed publisher integration.
#![allow(clippy::missing_errors_doc, clippy::must_use_candidate)]

use chacha20poly1305::{
    XChaCha20Poly1305, XNonce,
    aead::{Aead, KeyInit, Payload},
};
use hkdf::Hkdf;
use lez_xmr_monero_adapter::{MoneroNetwork, VerifiedMoneroOutputObservation};
use sha2::{Digest, Sha256};
use std::{
    fmt,
    fs::{self, File},
    io::Read as _,
    path::Path,
};
use thiserror::Error;
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

const KEY_ID_MAX_BYTES: usize = 128;
const MAX_PROTECTION_KEY_FILE_BYTES: usize = 66;
const MAX_PROTECTION_KEY_FILE_BYTES_U64: u64 = 66;
const TAG_BYTES: usize = 16;
const AAD_DOMAIN: &[u8] = b"lez-atomic-swaps/xmr-release/aad/v1";
const KEY_DOMAIN: &[u8] = b"lez-atomic-swaps/xmr-release/key/v1";
const FINGERPRINT_DOMAIN: &[u8] = b"lez-atomic-swaps/xmr-release/fingerprint/v1";
const CONTEXT_DOMAIN: &[u8] = b"lez-atomic-swaps/xmr-release/context/v3";
const EXACT_DOMAIN: &[u8] = b"lez-atomic-swaps/xmr-release/exact/v3";
const OBSERVATION_DOMAIN: &[u8] = b"lez-atomic-swaps/xmr-release/observation/v1";
const RESOURCE_DOMAIN: &[u8] = b"lez-atomic-swaps/xmr-release/monero-resource/v1";
const ACTIVATION_DOMAIN: &[u8] = b"lez-atomic-swaps/xmr-release/activation/v1";
const SEMANTIC_KEY_DOMAIN: &[u8] = b"lez-atomic-swaps/xmr-release/semantic-key/v1";
const SEMANTIC_MAC_DOMAIN: &[u8] = b"lez-atomic-swaps/xmr-release/semantic-mac/v1";
const OBSERVATION_KEY_DOMAIN: &[u8] = b"lez-atomic-swaps/xmr-release/observation-key/v1";
const OBSERVATION_MAC_DOMAIN: &[u8] = b"lez-atomic-swaps/xmr-release/observation-mac/v1";
const STATE_KEY_DOMAIN: &[u8] = b"lez-atomic-swaps/xmr-release/state-key/v1";
const STATE_MAC_DOMAIN: &[u8] = b"lez-atomic-swaps/xmr-release/state-mac/v1";

/// Maximum accepted exact publication payload.
pub const MAX_PUBLICATION_INTENT_BYTES: usize = 2_000_000;

/// Failure to load one release-service-owned journal protection key.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum ProtectionKeyFileError {
    /// The configured file could not be inspected, opened, or read.
    #[error("release protection-key file is unavailable")]
    Unavailable,
    /// The path was not one stable owner-private regular file.
    #[error("release protection-key file is unsafe")]
    Unsafe,
    /// Contents were not one nonzero lowercase-hex 32-byte key.
    #[error("release protection-key contents are invalid")]
    InvalidContents,
}
/// Caller-owned master key used only to derive intent-specific AEAD keys.
///
/// The material is zeroized on drop and is neither cloneable nor serializable.
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct PublicationProtectionKey {
    #[zeroize(skip)]
    key_id: Box<str>,
    material: [u8; 32],
}
impl PublicationProtectionKey {
    /// Constructs a key with a bounded non-secret rotation identifier.
    pub fn new(key_id: impl Into<Box<str>>, material: [u8; 32]) -> Result<Self, ProtectionError> {
        let key_id = key_id.into();
        validate_key_id(&key_id)?;
        if material == [0; 32] {
            return Err(ProtectionError::InvalidKeyMaterial);
        }
        Ok(Self { key_id, material })
    }

    /// Loads one stable owner-private lowercase-hex key file.
    ///
    /// The path, contents, and raw material are never included in errors or debug output.
    pub fn from_owner_private_file(
        key_id: impl Into<Box<str>>,
        path: impl AsRef<Path>,
    ) -> Result<Self, ProtectionKeyFileError> {
        let mut encoded = read_owner_private_key_file(path.as_ref())?;
        let trimmed_length = if encoded.ends_with(b"\r\n") {
            Some(encoded.len() - 2)
        } else if encoded.ends_with(b"\n") {
            Some(encoded.len() - 1)
        } else {
            None
        };
        if let Some(trimmed_length) = trimmed_length {
            encoded.truncate(trimmed_length);
        }
        if encoded.len() != 64
            || !encoded
                .iter()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
        {
            return Err(ProtectionKeyFileError::InvalidContents);
        }
        let mut material = Zeroizing::new([0_u8; 32]);
        hex::decode_to_slice(encoded.as_slice(), material.as_mut())
            .map_err(|_| ProtectionKeyFileError::InvalidContents)?;
        Self::new(key_id, *material).map_err(|_| ProtectionKeyFileError::InvalidContents)
    }

    /// Returns the non-secret rotation identifier.
    pub fn key_id(&self) -> &str {
        &self.key_id
    }
}
impl fmt::Debug for PublicationProtectionKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PublicationProtectionKey")
            .field("key_id", &self.key_id)
            .field("material", &"[REDACTED]")
            .finish()
    }
}

fn read_owner_private_key_file(path: &Path) -> Result<Zeroizing<Vec<u8>>, ProtectionKeyFileError> {
    let before = fs::symlink_metadata(path).map_err(|_| ProtectionKeyFileError::Unavailable)?;
    validate_protection_key_metadata(&before)?;

    let file = File::open(path).map_err(|_| ProtectionKeyFileError::Unavailable)?;
    let opened = file
        .metadata()
        .map_err(|_| ProtectionKeyFileError::Unavailable)?;
    validate_protection_key_metadata(&opened)?;
    if !same_protection_key_file(&before, &opened) {
        return Err(ProtectionKeyFileError::Unsafe);
    }

    let mut bytes = Zeroizing::new(Vec::with_capacity(MAX_PROTECTION_KEY_FILE_BYTES + 1));
    (&file)
        .take(MAX_PROTECTION_KEY_FILE_BYTES_U64 + 1)
        .read_to_end(bytes.as_mut())
        .map_err(|_| ProtectionKeyFileError::Unavailable)?;

    let opened_after = file
        .metadata()
        .map_err(|_| ProtectionKeyFileError::Unavailable)?;
    let path_after = fs::symlink_metadata(path).map_err(|_| ProtectionKeyFileError::Unavailable)?;
    validate_protection_key_metadata(&opened_after)?;
    validate_protection_key_metadata(&path_after)?;
    if !stable_protection_key_file(&opened, &opened_after)
        || !stable_protection_key_file(&opened, &path_after)
        || bytes.is_empty()
        || bytes.len() > MAX_PROTECTION_KEY_FILE_BYTES
    {
        return Err(ProtectionKeyFileError::Unsafe);
    }
    Ok(bytes)
}

fn validate_protection_key_metadata(metadata: &fs::Metadata) -> Result<(), ProtectionKeyFileError> {
    use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};

    if !metadata.file_type().is_file()
        || metadata.len() == 0
        || metadata.len() > MAX_PROTECTION_KEY_FILE_BYTES_U64
        || metadata.permissions().mode() & 0o7777 != 0o600
        || metadata.nlink() != 1
        || metadata.uid() != rustix::process::geteuid().as_raw()
    {
        return Err(ProtectionKeyFileError::Unsafe);
    }
    Ok(())
}

fn same_protection_key_file(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt as _;

    left.dev() == right.dev() && left.ino() == right.ino()
}

fn stable_protection_key_file(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt as _;

    same_protection_key_file(left, right)
        && left.len() == right.len()
        && left.uid() == right.uid()
        && left.mode() == right.mode()
        && left.nlink() == right.nlink()
        && left.mtime() == right.mtime()
        && left.mtime_nsec() == right.mtime_nsec()
        && left.ctime() == right.ctime()
        && left.ctime_nsec() == right.ctime_nsec()
}

/// Record-safe XChaCha20-Poly1305 encrypted publication bytes.
#[derive(Clone, Eq, PartialEq)]
pub struct ProtectedPublicationIntent {
    key_id: Box<str>,
    nonce: [u8; 24],
    ciphertext: Vec<u8>,
    fingerprint: [u8; 32],
}
impl ProtectedPublicationIntent {
    #[cfg_attr(not(test), allow(dead_code))]
    fn encrypt(
        plaintext: &[u8],
        key: &PublicationProtectionKey,
        release_context: &[u8],
    ) -> Result<Self, ProtectionError> {
        validate_plaintext_length(plaintext.len())?;
        let mut nonce = [0; 24];
        getrandom::fill(&mut nonce).map_err(|_| ProtectionError::Randomness)?;
        let aad = authentication_context(release_context, key.key_id());
        let derived = derive_key(key, &aad)?;
        let cipher = XChaCha20Poly1305::new_from_slice(derived.as_ref())
            .map_err(|_| ProtectionError::KeyDerivation)?;
        let ciphertext = cipher
            .encrypt(
                XNonce::from_slice(&nonce),
                Payload {
                    msg: plaintext,
                    aad: &aad,
                },
            )
            .map_err(|_| ProtectionError::Encryption)?;
        validate_ciphertext_length(ciphertext.len())?;
        let fingerprint = envelope_fingerprint(key.key_id(), &nonce, &ciphertext, release_context);
        Ok(Self {
            key_id: key.key_id.clone(),
            nonce,
            ciphertext,
            fingerprint,
        })
    }

    fn from_record_fields(
        key_id: String,
        nonce: [u8; 24],
        ciphertext: Vec<u8>,
        fingerprint: [u8; 32],
        release_context: &[u8],
    ) -> Result<Self, ProtectionError> {
        validate_key_id(&key_id)?;
        validate_ciphertext_length(ciphertext.len())?;
        if fingerprint != envelope_fingerprint(&key_id, &nonce, &ciphertext, release_context) {
            return Err(ProtectionError::FingerprintMismatch);
        }
        Ok(Self {
            key_id: key_id.into_boxed_str(),
            nonce,
            ciphertext,
            fingerprint,
        })
    }

    fn decrypt(
        &self,
        key: &PublicationProtectionKey,
        release_context: &[u8],
    ) -> Result<Zeroizing<Vec<u8>>, ProtectionError> {
        if self.key_id.as_ref() != key.key_id() {
            return Err(ProtectionError::KeyIdMismatch);
        }
        validate_ciphertext_length(self.ciphertext.len())?;
        if self.fingerprint
            != envelope_fingerprint(&self.key_id, &self.nonce, &self.ciphertext, release_context)
        {
            return Err(ProtectionError::FingerprintMismatch);
        }
        let aad = authentication_context(release_context, &self.key_id);
        let derived = derive_key(key, &aad)?;
        let cipher = XChaCha20Poly1305::new_from_slice(derived.as_ref())
            .map_err(|_| ProtectionError::KeyDerivation)?;
        let plaintext = Zeroizing::new(
            cipher
                .decrypt(
                    XNonce::from_slice(&self.nonce),
                    Payload {
                        msg: &self.ciphertext,
                        aad: &aad,
                    },
                )
                .map_err(|_| ProtectionError::Authentication)?,
        );
        validate_plaintext_length(plaintext.len())?;
        Ok(plaintext)
    }

    /// Returns the non-secret key identifier.
    pub fn key_id(&self) -> &str {
        &self.key_id
    }

    /// Returns the authenticated ciphertext and tag.
    pub fn ciphertext(&self) -> &[u8] {
        &self.ciphertext
    }
}
impl fmt::Debug for ProtectedPublicationIntent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProtectedPublicationIntent")
            .field("key_id", &self.key_id)
            .field("nonce", &"[REDACTED]")
            .field(
                "ciphertext",
                &format_args!("[REDACTED; {} bytes]", self.ciphertext.len()),
            )
            .field("fingerprint", &"[REDACTED]")
            .finish()
    }
}

/// Publication protection validation or cryptographic failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Error)]
pub enum ProtectionError {
    #[error("publication key ID must contain 1 through 128 bytes")]
    InvalidKeyId,
    #[error("publication protection key material must be non-zero")]
    InvalidKeyMaterial,
    #[error("publication payload must contain 1 through 2000000 bytes")]
    InvalidPayloadLength,
    #[error("publication ciphertext has an invalid length")]
    InvalidCiphertextLength,
    #[error("publication envelope fingerprint mismatch")]
    FingerprintMismatch,
    #[error("publication key ID mismatch")]
    KeyIdMismatch,
    #[error("operating-system randomness unavailable")]
    Randomness,
    #[error("publication key derivation failed")]
    KeyDerivation,
    #[error("publication encryption failed")]
    Encryption,
    #[error("publication authentication failed")]
    Authentication,
}

#[cfg_attr(not(test), allow(dead_code))]
struct ReleasePlan {
    activation: [u8; 32],
    swap_id: [u8; 32],
    run_id: [u8; 32],
    lez_commitment: [u8; 32],
    topology_commitment: [u8; 32],
    resource_id: [u8; 32],
    observation: Vec<u8>,
    claim_partial_commitment: [u8; 32],
    target: Vec<u8>,
    publication_id: [u8; 32],
    window_start: u64,
    window_end: u64,
    publication: Zeroizing<Vec<u8>>,
}

impl fmt::Debug for ReleasePlan {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ReleasePlan")
            .field("authority", &"[REDACTED]")
            .finish_non_exhaustive()
    }
}

#[cfg_attr(not(test), allow(dead_code))]
impl ReleasePlan {
    fn immutable_context(&self) -> Vec<u8> {
        immutable_release_context_bytes(
            &self.activation,
            &self.swap_id,
            &self.run_id,
            &self.lez_commitment,
            &self.topology_commitment,
            &self.resource_id,
            &self.claim_partial_commitment,
            &self.target,
            &self.publication_id,
            self.window_start,
            self.window_end,
        )
    }
}

#[allow(dead_code)]
fn observation_bytes(value: &VerifiedMoneroOutputObservation) -> Vec<u8> {
    let genesis_hash = value.genesis_hash();
    let transaction_id = value.transaction_id().0;
    let destination = value.destination().to_string();
    let containing_block_hash = value.containing_block_hash();
    let stable_tip_hash = value.stable_tip_hash();
    encode_observation(&ObservationEncoding {
        network_tag: match value.network() {
            MoneroNetwork::Regtest => 0,
            MoneroNetwork::Stagenet => 1,
        },
        genesis_hash: &genesis_hash,
        daemon_origin: value.daemon_origin(),
        wallet_origin: value.wallet_origin(),
        transaction_id: &transaction_id,
        destination: &destination,
        amount_piconero: value.amount_piconero(),
        containing_block_hash: &containing_block_hash,
        containing_block_height: value.containing_block_height(),
        confirmations: value.confirmations(),
        stable_tip_hash: &stable_tip_hash,
        stable_tip_height: value.stable_tip_height(),
    })
}

#[allow(dead_code)]
fn monero_resource_id(value: &VerifiedMoneroOutputObservation) -> [u8; 32] {
    let genesis_hash = value.genesis_hash();
    let transaction_id = value.transaction_id().0;
    let destination = value.destination().to_string();
    hash(&encode_monero_resource(&ObservationEncoding {
        network_tag: match value.network() {
            MoneroNetwork::Regtest => 0,
            MoneroNetwork::Stagenet => 1,
        },
        genesis_hash: &genesis_hash,
        daemon_origin: value.daemon_origin(),
        wallet_origin: value.wallet_origin(),
        transaction_id: &transaction_id,
        destination: &destination,
        amount_piconero: value.amount_piconero(),
        containing_block_hash: &value.containing_block_hash(),
        containing_block_height: value.containing_block_height(),
        confirmations: value.confirmations(),
        stable_tip_hash: &value.stable_tip_hash(),
        stable_tip_height: value.stable_tip_height(),
    }))
}

struct ObservationEncoding<'a> {
    network_tag: u8,
    genesis_hash: &'a [u8; 32],
    daemon_origin: &'a str,
    wallet_origin: &'a str,
    transaction_id: &'a [u8; 32],
    destination: &'a str,
    amount_piconero: u64,
    containing_block_hash: &'a [u8; 32],
    containing_block_height: u64,
    confirmations: u64,
    stable_tip_hash: &'a [u8; 32],
    stable_tip_height: u64,
}

fn encode_observation(value: &ObservationEncoding<'_>) -> Vec<u8> {
    let mut encoded = OBSERVATION_DOMAIN.to_vec();
    for item in [
        [value.network_tag].as_slice(),
        value.genesis_hash.as_slice(),
        value.daemon_origin.as_bytes(),
        value.wallet_origin.as_bytes(),
        value.transaction_id.as_slice(),
        value.destination.as_bytes(),
        &value.amount_piconero.to_be_bytes(),
        value.containing_block_hash.as_slice(),
        &value.containing_block_height.to_be_bytes(),
        &value.confirmations.to_be_bytes(),
        value.stable_tip_hash.as_slice(),
        &value.stable_tip_height.to_be_bytes(),
    ] {
        append(&mut encoded, item);
    }
    encoded
}

fn encode_monero_resource(value: &ObservationEncoding<'_>) -> Vec<u8> {
    let mut encoded = RESOURCE_DOMAIN.to_vec();
    for item in [
        [value.network_tag].as_slice(),
        value.genesis_hash.as_slice(),
        value.transaction_id.as_slice(),
        value.destination.as_bytes(),
        &value.amount_piconero.to_be_bytes(),
    ] {
        append(&mut encoded, item);
    }
    encoded
}

#[allow(clippy::too_many_arguments)]
fn immutable_release_context_bytes(
    activation: &[u8; 32],
    swap_id: &[u8; 32],
    run_id: &[u8; 32],
    lez_commitment: &[u8; 32],
    topology_commitment: &[u8; 32],
    resource_id: &[u8; 32],
    claim_partial_commitment: &[u8; 32],
    target: &[u8],
    publication_id: &[u8; 32],
    window_start: u64,
    window_end: u64,
) -> Vec<u8> {
    let mut encoded = CONTEXT_DOMAIN.to_vec();
    for item in [
        activation.as_slice(),
        swap_id.as_slice(),
        run_id.as_slice(),
        lez_commitment.as_slice(),
        topology_commitment.as_slice(),
        resource_id.as_slice(),
        claim_partial_commitment.as_slice(),
        target,
        publication_id.as_slice(),
        &window_start.to_be_bytes(),
        &window_end.to_be_bytes(),
    ] {
        append(&mut encoded, item);
    }
    encoded
}

fn derive_activation_id(swap_id: &[u8; 32], run_id: &[u8; 32]) -> [u8; 32] {
    let mut encoded = ACTIVATION_DOMAIN.to_vec();
    append(&mut encoded, swap_id);
    append(&mut encoded, run_id);
    hash(&encoded)
}

fn authentication_context(release_context: &[u8], key_id: &str) -> Vec<u8> {
    let mut encoded = AAD_DOMAIN.to_vec();
    append(&mut encoded, release_context);
    append(&mut encoded, key_id.as_bytes());
    encoded
}

fn exact_binding_bytes(context: &[u8], intent: &ProtectedPublicationIntent) -> Vec<u8> {
    let mut encoded = EXACT_DOMAIN.to_vec();
    for item in [
        context,
        intent.key_id.as_bytes(),
        intent.nonce.as_slice(),
        intent.ciphertext.as_slice(),
        intent.fingerprint.as_slice(),
    ] {
        append(&mut encoded, item);
    }
    encoded
}

fn append(output: &mut Vec<u8>, value: &[u8]) {
    let length = u64::try_from(value.len()).expect("in-memory length fits u64");
    output.extend_from_slice(&length.to_be_bytes());
    output.extend_from_slice(value);
}

fn hash(value: &[u8]) -> [u8; 32] {
    Sha256::digest(value).into()
}

fn derive_key(
    master: &PublicationProtectionKey,
    aad: &[u8],
) -> Result<Zeroizing<[u8; 32]>, ProtectionError> {
    let hkdf = Hkdf::<Sha256>::new(Some(aad), &master.material);
    let mut key = Zeroizing::new([0; 32]);
    let mut info = Vec::with_capacity(KEY_DOMAIN.len() + aad.len());
    info.extend_from_slice(KEY_DOMAIN);
    info.extend_from_slice(aad);
    hkdf.expand(&info, key.as_mut())
        .map_err(|_| ProtectionError::KeyDerivation)?;
    Ok(key)
}

#[cfg_attr(not(test), allow(dead_code))]
fn release_state_authenticator(
    master: &PublicationProtectionKey,
    release_context: &[u8],
    binding: &[u8; 32],
    state: &str,
    revision: u8,
) -> Result<[u8; 32], ProtectionError> {
    use hmac::Mac;

    Ok(
        release_state_mac(master, release_context, binding, state, revision)?
            .finalize()
            .into_bytes()
            .into(),
    )
}

fn verify_release_state_authenticator(
    master: &PublicationProtectionKey,
    release_context: &[u8],
    binding: &[u8; 32],
    state: &str,
    revision: u8,
    expected: &[u8; 32],
) -> Result<bool, ProtectionError> {
    use hmac::Mac;

    Ok(
        release_state_mac(master, release_context, binding, state, revision)?
            .verify_slice(expected)
            .is_ok(),
    )
}

fn release_state_mac(
    master: &PublicationProtectionKey,
    release_context: &[u8],
    binding: &[u8; 32],
    state: &str,
    revision: u8,
) -> Result<hmac::Hmac<Sha256>, ProtectionError> {
    use hmac::Mac;

    let hkdf = Hkdf::<Sha256>::new(Some(release_context), &master.material);
    let mut key = Zeroizing::new([0; 32]);
    let mut info = STATE_KEY_DOMAIN.to_vec();
    append(&mut info, master.key_id.as_bytes());
    hkdf.expand(&info, key.as_mut())
        .map_err(|_| ProtectionError::KeyDerivation)?;
    let mut mac = <hmac::Hmac<Sha256> as Mac>::new_from_slice(key.as_ref())
        .map_err(|_| ProtectionError::KeyDerivation)?;
    mac.update(STATE_MAC_DOMAIN);
    mac.update(binding);
    mac.update(&[revision]);
    mac.update(state.as_bytes());
    Ok(mac)
}

#[cfg_attr(not(test), allow(dead_code))]
fn semantic_intent_authenticator(
    master: &PublicationProtectionKey,
    immutable_context: &[u8],
    plaintext: &[u8],
) -> Result<[u8; 32], ProtectionError> {
    use hmac::Mac;

    Ok(record_mac(
        master,
        immutable_context,
        SEMANTIC_KEY_DOMAIN,
        SEMANTIC_MAC_DOMAIN,
        plaintext,
    )?
    .finalize()
    .into_bytes()
    .into())
}

fn verify_semantic_intent_authenticator(
    master: &PublicationProtectionKey,
    immutable_context: &[u8],
    plaintext: &[u8],
    expected: &[u8; 32],
) -> Result<bool, ProtectionError> {
    use hmac::Mac;

    Ok(record_mac(
        master,
        immutable_context,
        SEMANTIC_KEY_DOMAIN,
        SEMANTIC_MAC_DOMAIN,
        plaintext,
    )?
    .verify_slice(expected)
    .is_ok())
}

#[cfg_attr(not(test), allow(dead_code))]
fn observation_authenticator(
    master: &PublicationProtectionKey,
    immutable_context: &[u8],
    observation: &[u8],
) -> Result<[u8; 32], ProtectionError> {
    use hmac::Mac;

    Ok(record_mac(
        master,
        immutable_context,
        OBSERVATION_KEY_DOMAIN,
        OBSERVATION_MAC_DOMAIN,
        observation,
    )?
    .finalize()
    .into_bytes()
    .into())
}

fn verify_observation_authenticator(
    master: &PublicationProtectionKey,
    immutable_context: &[u8],
    observation: &[u8],
    expected: &[u8; 32],
) -> Result<bool, ProtectionError> {
    use hmac::Mac;

    Ok(record_mac(
        master,
        immutable_context,
        OBSERVATION_KEY_DOMAIN,
        OBSERVATION_MAC_DOMAIN,
        observation,
    )?
    .verify_slice(expected)
    .is_ok())
}

fn record_mac(
    master: &PublicationProtectionKey,
    immutable_context: &[u8],
    key_domain: &[u8],
    mac_domain: &[u8],
    value: &[u8],
) -> Result<hmac::Hmac<Sha256>, ProtectionError> {
    use hmac::Mac;

    let hkdf = Hkdf::<Sha256>::new(Some(immutable_context), &master.material);
    let mut key = Zeroizing::new([0; 32]);
    let mut info = key_domain.to_vec();
    append(&mut info, master.key_id.as_bytes());
    hkdf.expand(&info, key.as_mut())
        .map_err(|_| ProtectionError::KeyDerivation)?;
    let mut mac = <hmac::Hmac<Sha256> as Mac>::new_from_slice(key.as_ref())
        .map_err(|_| ProtectionError::KeyDerivation)?;
    mac.update(mac_domain);
    mac.update(
        &u64::try_from(value.len())
            .expect("in-memory length fits u64")
            .to_be_bytes(),
    );
    mac.update(value);
    Ok(mac)
}

fn envelope_fingerprint(
    key_id: &str,
    nonce: &[u8; 24],
    ciphertext: &[u8],
    context: &[u8],
) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(FINGERPRINT_DOMAIN);
    append_hash(&mut hasher, key_id.as_bytes());
    hasher.update(nonce);
    append_hash(&mut hasher, ciphertext);
    append_hash(&mut hasher, context);
    hasher.finalize().into()
}

fn append_hash(hasher: &mut Sha256, value: &[u8]) {
    hasher.update(
        u64::try_from(value.len())
            .expect("in-memory length fits u64")
            .to_be_bytes(),
    );
    hasher.update(value);
}

fn validate_key_id(key_id: &str) -> Result<(), ProtectionError> {
    if key_id.is_empty() || key_id.len() > KEY_ID_MAX_BYTES {
        Err(ProtectionError::InvalidKeyId)
    } else {
        Ok(())
    }
}
fn validate_plaintext_length(length: usize) -> Result<(), ProtectionError> {
    if length == 0 || length > MAX_PUBLICATION_INTENT_BYTES {
        Err(ProtectionError::InvalidPayloadLength)
    } else {
        Ok(())
    }
}
fn validate_ciphertext_length(length: usize) -> Result<(), ProtectionError> {
    if (TAG_BYTES + 1..=MAX_PUBLICATION_INTENT_BYTES + TAG_BYTES).contains(&length) {
        Ok(())
    } else {
        Err(ProtectionError::InvalidCiphertextLength)
    }
}

mod issuer;
mod store;

pub use issuer::XmrClaimReleasePreparationError;
pub use store::{
    FinalizedLezClockError, FinalizedLezClockSource, PublicationAdmissionStatus, ReleaseError,
    ReleasePublicationError, ReleasePublicationOutcome, ReleaseSnapshot, ReleaseState,
    ReleaseStore, ReleaseWindow, XmrReleaseSubmissionBindingV3,
};

#[cfg(test)]
mod tests {
    use super::*;

    fn encoded_observation(
        daemon_origin: &str,
        wallet_origin: &str,
        tip_byte: u8,
        tip_height: u64,
    ) -> Vec<u8> {
        encode_observation(&ObservationEncoding {
            network_tag: 0,
            genesis_hash: &[0x11; 32],
            daemon_origin,
            wallet_origin,
            transaction_id: &[0x22; 32],
            destination: "4canonical-monero-address",
            amount_piconero: 42,
            containing_block_hash: &[0x33; 32],
            containing_block_height: 100,
            confirmations: tip_height - 99,
            stable_tip_hash: &[tip_byte; 32],
            stable_tip_height: tip_height,
        })
    }

    fn encoded_resource(
        network_tag: u8,
        genesis_byte: u8,
        transaction_byte: u8,
        destination: &str,
        amount_piconero: u64,
    ) -> [u8; 32] {
        hash(&encode_monero_resource(&ObservationEncoding {
            network_tag,
            genesis_hash: &[genesis_byte; 32],
            daemon_origin: "http://127.0.0.1:18081",
            wallet_origin: "http://127.0.0.1:18083",
            transaction_id: &[transaction_byte; 32],
            destination,
            amount_piconero,
            containing_block_hash: &[0x33; 32],
            containing_block_height: 100,
            confirmations: 10,
            stable_tip_hash: &[0x44; 32],
            stable_tip_height: 109,
        }))
    }

    fn frame(value: &[u8]) -> Vec<u8> {
        let mut framed = u64::try_from(value.len()).unwrap().to_be_bytes().to_vec();
        framed.extend_from_slice(value);
        framed
    }

    #[test]
    fn full_observation_identity_binds_both_exact_rpc_origins() {
        let daemon = "http://127.0.0.1:18081";
        let wallet = "http://127.0.0.1:18083";
        let baseline = encoded_observation(daemon, wallet, 0x44, 109);
        let daemon_drift = encoded_observation("http://127.0.0.1:28081", wallet, 0x44, 109);
        let wallet_drift = encoded_observation(daemon, "http://127.0.0.1:28083", 0x44, 109);
        let daemon_frame = frame(daemon.as_bytes());
        let wallet_frame = frame(wallet.as_bytes());

        assert_ne!(hash(&baseline), hash(&daemon_drift));
        assert_ne!(hash(&baseline), hash(&wallet_drift));
        assert!(
            baseline
                .windows(daemon_frame.len())
                .any(|window| window == daemon_frame)
        );
        assert!(
            baseline
                .windows(wallet_frame.len())
                .any(|window| window == wallet_frame)
        );
    }

    #[test]
    fn stable_resource_uses_only_immutable_monero_output_identity() {
        let baseline = encoded_resource(0, 0x11, 0x22, "4canonical-monero-address", 42);
        let rescanned_full_before = encoded_observation(
            "http://127.0.0.1:18081",
            "http://127.0.0.1:18083",
            0x44,
            109,
        );
        let rescanned_full_later = encoded_observation(
            "http://127.0.0.1:28081",
            "http://127.0.0.1:28083",
            0x55,
            120,
        );
        assert_ne!(rescanned_full_before, rescanned_full_later);
        assert_eq!(
            baseline,
            encoded_resource(0, 0x11, 0x22, "4canonical-monero-address", 42)
        );
        assert_ne!(
            baseline,
            encoded_resource(1, 0x11, 0x22, "4canonical-monero-address", 42)
        );
        assert_ne!(
            baseline,
            encoded_resource(0, 0x12, 0x22, "4canonical-monero-address", 42)
        );
        assert_ne!(
            baseline,
            encoded_resource(0, 0x11, 0x23, "4canonical-monero-address", 42)
        );
        assert_ne!(
            baseline,
            encoded_resource(0, 0x11, 0x22, "4other-monero-address", 42)
        );
        assert_ne!(
            baseline,
            encoded_resource(0, 0x11, 0x22, "4canonical-monero-address", 43)
        );
    }

    #[test]
    fn internal_plan_debug_redacts_commitments_and_protocol_identifiers() {
        let swap_id = [0x41; 32];
        let run_id = [0x42; 32];
        let plan = ReleasePlan {
            activation: derive_activation_id(&swap_id, &run_id),
            swap_id,
            run_id,
            lez_commitment: [0x43; 32],
            topology_commitment: [0x44; 32],
            resource_id: [0x45; 32],
            observation: b"observation-identifier-must-not-leak".to_vec(),
            claim_partial_commitment: [0x55; 32],
            target: b"target-identifier-must-not-leak".to_vec(),
            publication_id: [0x56; 32],
            window_start: 100,
            window_end: 200,
            publication: Zeroizing::new(b"actual-hidden-partial-only-inside-publication".to_vec()),
        };
        let debug = format!("{plan:?}");
        for forbidden in [
            "observation-identifier-must-not-leak",
            "target-identifier-must-not-leak",
            "actual-hidden-partial-only-inside-publication",
            "65, 65",
            "66, 66",
            "67, 67",
            "68, 68",
            "69, 69",
            "85, 85",
        ] {
            assert!(!debug.contains(forbidden), "Debug leaked {forbidden}");
        }
        assert!(debug.contains("[REDACTED]"));
    }
}
