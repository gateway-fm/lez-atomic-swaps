//! Authenticated encryption for claim preimages at rest.
//!
//! The storage adapter owns nonce generation and persistence. It **must** use a
//! unique nonce for every encryption performed with the same key and context.
//! Neither the master key nor plaintext claim material implements serialization.

use std::{fmt, marker::PhantomData};

use chacha20poly1305::{
    XChaCha20Poly1305, XNonce,
    aead::{Aead, KeyInit, Payload},
};
use hkdf::Hkdf;
use lez_swap_core::{Pair, Participant, SwapDirection, SwapId};
use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{Error as _, IgnoredAny, SeqAccess, Visitor},
};
use sha2::{Digest, Sha256};
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

use crate::ClaimPreimage;

/// Current protected-claim envelope schema.
pub const PROTECTED_CLAIM_SCHEMA_V1: u16 = 1;

const AAD_DOMAIN: &[u8] = b"lez-atomic-swaps/protected-claim/aad/v1";
const KEY_DOMAIN: &[u8] = b"lez-atomic-swaps/protected-claim/key/v1";
const FINGERPRINT_DOMAIN: &[u8] = b"lez-atomic-swaps/protected-claim/fingerprint/v1";
const CLAIM_PREIMAGE_BYTES: usize = 32;
const AEAD_TAG_BYTES: usize = 16;
const PROTECTED_CLAIM_BYTES: usize = CLAIM_PREIMAGE_BYTES + AEAD_TAG_BYTES;
const MAX_KEY_ID_BYTES: usize = 128;

/// Why this process is retaining a claim preimage.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClaimMaterialPurpose {
    /// Locally generated material used by the first claimant.
    LocalFirstClaim,
    /// Canonically observed material used by the follow-up claimant.
    ObservedFollowUpClaim,
}

impl ClaimMaterialPurpose {
    const fn tag(self) -> u8 {
        match self {
            Self::LocalFirstClaim => 0,
            Self::ObservedFollowUpClaim => 1,
        }
    }
}

/// Immutable public facts that domain-separate a protected preimage.
///
/// Construct this from the validated durable agreement rather than from
/// untrusted envelope metadata. The schema is explicit so future migrations
/// cannot accidentally authenticate data under the wrong representation.
#[derive(Clone, Copy, Debug)]
pub struct ClaimMaterialContext<'a> {
    schema: u16,
    swap_id: &'a SwapId,
    pair: Pair,
    direction: SwapDirection,
    agreement_commitment: &'a [u8; 32],
    local_role: Participant,
    purpose: ClaimMaterialPurpose,
}

impl<'a> ClaimMaterialContext<'a> {
    /// Creates the complete authenticated context for one protected preimage.
    #[must_use]
    pub const fn new(
        schema: u16,
        swap_id: &'a SwapId,
        pair: Pair,
        direction: SwapDirection,
        agreement_commitment: &'a [u8; 32],
        local_role: Participant,
        purpose: ClaimMaterialPurpose,
    ) -> Self {
        Self {
            schema,
            swap_id,
            pair,
            direction,
            agreement_commitment,
            local_role,
            purpose,
        }
    }

    fn encode(self, key_id: &str) -> Vec<u8> {
        let swap_id = self.swap_id.as_str().as_bytes();
        let mut encoded = Vec::with_capacity(
            AAD_DOMAIN.len() + swap_id.len() + key_id.len() + self.agreement_commitment.len() + 16,
        );
        encoded.extend_from_slice(AAD_DOMAIN);
        encoded.extend_from_slice(&self.schema.to_be_bytes());
        append_length_prefixed(&mut encoded, swap_id);
        encoded.push(pair_tag(self.pair));
        encoded.push(direction_tag(self.direction));
        encoded.extend_from_slice(self.agreement_commitment);
        encoded.push(participant_tag(self.local_role));
        encoded.push(self.purpose.tag());
        append_length_prefixed(&mut encoded, key_id.as_bytes());
        encoded
    }
}

/// Caller-owned master key used only to derive envelope-specific AEAD keys.
///
/// This type is zeroized on drop and intentionally cannot be serialized or
/// cloned. Its debug representation never includes the key material.
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct ProtectedClaimKey {
    #[zeroize(skip)]
    key_id: Box<str>,
    material: [u8; 32],
}

impl ProtectedClaimKey {
    /// Creates a key identified by a non-secret rotation label.
    ///
    /// # Errors
    ///
    /// Returns [`ProtectedClaimError::InvalidKeyId`] when the identifier is
    /// empty or longer than 128 bytes.
    pub fn new(
        key_id: impl Into<Box<str>>,
        material: [u8; 32],
    ) -> Result<Self, ProtectedClaimError> {
        let key_id = key_id.into();
        validate_key_id(&key_id)?;
        Ok(Self { key_id, material })
    }

    /// Non-secret identifier used to select the key after rotation.
    #[must_use]
    pub fn key_id(&self) -> &str {
        &self.key_id
    }
}

impl fmt::Debug for ProtectedClaimKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProtectedClaimKey")
            .field("key_id", &self.key_id)
            .field("material", &"[REDACTED]")
            .finish()
    }
}

#[derive(Clone, Eq, PartialEq)]
struct ProtectedCiphertext([u8; PROTECTED_CLAIM_BYTES]);

impl Serialize for ProtectedCiphertext {
    fn serialize<SerializerT>(
        &self,
        serializer: SerializerT,
    ) -> Result<SerializerT::Ok, SerializerT::Error>
    where
        SerializerT: Serializer,
    {
        serializer.serialize_bytes(&self.0)
    }
}

impl<'de> Deserialize<'de> for ProtectedCiphertext {
    fn deserialize<DeserializerT>(deserializer: DeserializerT) -> Result<Self, DeserializerT::Error>
    where
        DeserializerT: Deserializer<'de>,
    {
        struct CiphertextVisitor(PhantomData<[u8; PROTECTED_CLAIM_BYTES]>);

        impl<'de> Visitor<'de> for CiphertextVisitor {
            type Value = ProtectedCiphertext;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(
                    formatter,
                    "exactly {PROTECTED_CLAIM_BYTES} ciphertext bytes"
                )
            }

            fn visit_bytes<ErrorT>(self, value: &[u8]) -> Result<Self::Value, ErrorT>
            where
                ErrorT: serde::de::Error,
            {
                let bytes = value
                    .try_into()
                    .map_err(|_| ErrorT::invalid_length(value.len(), &self))?;
                Ok(ProtectedCiphertext(bytes))
            }

            fn visit_seq<AccessT>(
                self,
                mut sequence: AccessT,
            ) -> Result<Self::Value, AccessT::Error>
            where
                AccessT: SeqAccess<'de>,
            {
                let mut bytes = [0_u8; PROTECTED_CLAIM_BYTES];
                for (index, byte) in bytes.iter_mut().enumerate() {
                    *byte = sequence
                        .next_element()?
                        .ok_or_else(|| AccessT::Error::invalid_length(index, &self))?;
                }
                if sequence.next_element::<IgnoredAny>()?.is_some() {
                    return Err(AccessT::Error::invalid_length(
                        PROTECTED_CLAIM_BYTES + 1,
                        &self,
                    ));
                }
                Ok(ProtectedCiphertext(bytes))
            }
        }

        deserializer.deserialize_bytes(CiphertextVisitor(PhantomData))
    }
}

/// Record-safe encrypted claim material.
///
/// These are the only envelope fields intended for serialization. The
/// fingerprint identifies storage corruption; authenticity is always decided
/// by XChaCha20-Poly1305 with the caller-supplied canonical context.
#[derive(Clone, Eq, PartialEq, Serialize)]
pub struct ProtectedClaimEnvelope {
    ciphertext: ProtectedCiphertext,
    nonce: [u8; 24],
    key_id: Box<str>,
    fingerprint: [u8; 32],
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProtectedClaimEnvelopeRecord {
    ciphertext: ProtectedCiphertext,
    nonce: [u8; 24],
    key_id: Box<str>,
    fingerprint: [u8; 32],
}

impl<'de> Deserialize<'de> for ProtectedClaimEnvelope {
    fn deserialize<DeserializerT>(deserializer: DeserializerT) -> Result<Self, DeserializerT::Error>
    where
        DeserializerT: Deserializer<'de>,
    {
        let record = ProtectedClaimEnvelopeRecord::deserialize(deserializer)?;
        Self::from_record_fields(
            record.ciphertext.0,
            record.nonce,
            record.key_id,
            record.fingerprint,
        )
        .map_err(DeserializerT::Error::custom)
    }
}

impl ProtectedClaimEnvelope {
    /// Encrypts a claim preimage under a context-derived key.
    ///
    /// The caller must never reuse `nonce` with the same key and context.
    ///
    /// # Errors
    ///
    /// Returns an error if authenticated encryption or key derivation fails.
    pub fn encrypt(
        preimage: &ClaimPreimage,
        key: &ProtectedClaimKey,
        nonce: [u8; 24],
        context: ClaimMaterialContext<'_>,
    ) -> Result<Self, ProtectedClaimError> {
        let aad = context.encode(key.key_id());
        let derived_key = derive_key(key, &aad)?;
        let cipher = XChaCha20Poly1305::new_from_slice(derived_key.as_ref())
            .map_err(|_| ProtectedClaimError::KeyDerivation)?;
        let ciphertext = ProtectedCiphertext(
            cipher
                .encrypt(
                    XNonce::from_slice(&nonce),
                    Payload {
                        msg: preimage.expose_secret(),
                        aad: &aad,
                    },
                )
                .map_err(|_| ProtectedClaimError::Encryption)?
                .try_into()
                .map_err(|_| ProtectedClaimError::InvalidCiphertextLength)?,
        );
        let fingerprint = fingerprint(key.key_id(), &nonce, &ciphertext.0);

        Ok(Self {
            ciphertext,
            nonce,
            key_id: key.key_id.clone(),
            fingerprint,
        })
    }

    /// Restores an envelope from its four record-safe fields.
    ///
    /// # Errors
    ///
    /// Returns an error for malformed field lengths or a mismatched
    /// fingerprint. This does not authenticate the plaintext; call
    /// [`Self::decrypt`] with canonical agreement context for that.
    pub fn from_record_fields(
        ciphertext: [u8; PROTECTED_CLAIM_BYTES],
        nonce: [u8; 24],
        key_id: impl Into<Box<str>>,
        fingerprint: [u8; 32],
    ) -> Result<Self, ProtectedClaimError> {
        let key_id = key_id.into();
        validate_key_id(&key_id)?;
        if fingerprint != self::fingerprint(&key_id, &nonce, &ciphertext) {
            return Err(ProtectedClaimError::FingerprintMismatch);
        }
        Ok(Self {
            ciphertext: ProtectedCiphertext(ciphertext),
            nonce,
            key_id,
            fingerprint,
        })
    }

    /// Authenticates and decrypts the preimage using canonical agreement facts.
    ///
    /// # Errors
    ///
    /// Fails closed for a wrong key ID, corrupt record field, wrong key, or any
    /// change to the authenticated context.
    pub fn decrypt(
        &self,
        key: &ProtectedClaimKey,
        context: ClaimMaterialContext<'_>,
    ) -> Result<ClaimPreimage, ProtectedClaimError> {
        if self.key_id.as_ref() != key.key_id() {
            return Err(ProtectedClaimError::KeyIdMismatch);
        }
        if self.fingerprint != fingerprint(&self.key_id, &self.nonce, &self.ciphertext.0) {
            return Err(ProtectedClaimError::FingerprintMismatch);
        }

        let aad = context.encode(&self.key_id);
        let derived_key = derive_key(key, &aad)?;
        let cipher = XChaCha20Poly1305::new_from_slice(derived_key.as_ref())
            .map_err(|_| ProtectedClaimError::KeyDerivation)?;
        let plaintext = Zeroizing::new(
            cipher
                .decrypt(
                    XNonce::from_slice(&self.nonce),
                    Payload {
                        msg: &self.ciphertext.0,
                        aad: &aad,
                    },
                )
                .map_err(|_| ProtectedClaimError::Authentication)?,
        );
        let secret = Zeroizing::new(
            plaintext
                .as_slice()
                .try_into()
                .map_err(|_| ProtectedClaimError::InvalidPlaintextLength)?,
        );
        Ok(ClaimPreimage::new(*secret))
    }

    /// Authenticated ciphertext and tag.
    #[must_use]
    pub fn ciphertext(&self) -> &[u8] {
        &self.ciphertext.0
    }

    /// Caller-generated `XChaCha20` nonce.
    #[must_use]
    pub const fn nonce(&self) -> &[u8; 24] {
        &self.nonce
    }

    /// Non-secret key-rotation identifier.
    #[must_use]
    pub fn key_id(&self) -> &str {
        &self.key_id
    }

    /// SHA-256 fingerprint over the record-safe envelope fields.
    #[must_use]
    pub const fn fingerprint(&self) -> &[u8; 32] {
        &self.fingerprint
    }
}

impl fmt::Debug for ProtectedClaimEnvelope {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProtectedClaimEnvelope")
            .field(
                "ciphertext",
                &format_args!("[REDACTED; {} bytes]", self.ciphertext.0.len()),
            )
            .field("nonce", &"[REDACTED]")
            .field("key_id", &self.key_id)
            .field("fingerprint", &"[REDACTED]")
            .finish()
    }
}

/// Protected claim-material validation or cryptographic failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum ProtectedClaimError {
    /// Key IDs are required and capped to keep durable records bounded.
    #[error("protected-claim key ID must contain 1 through 128 bytes")]
    InvalidKeyId,
    /// A record does not contain exactly one encrypted 32-byte preimage and tag.
    #[error("protected-claim ciphertext has an invalid length")]
    InvalidCiphertextLength,
    /// Record-safe fields do not match their corruption-detection fingerprint.
    #[error("protected-claim envelope fingerprint mismatch")]
    FingerprintMismatch,
    /// The supplied key does not match the envelope's rotation identifier.
    #[error("protected-claim key ID mismatch")]
    KeyIdMismatch,
    /// HKDF-SHA256 could not produce the envelope key.
    #[error("protected-claim key derivation failed")]
    KeyDerivation,
    /// XChaCha20-Poly1305 encryption failed.
    #[error("protected-claim encryption failed")]
    Encryption,
    /// Authentication failed; no plaintext is returned.
    #[error("protected-claim authentication failed")]
    Authentication,
    /// Authenticated plaintext did not have the expected fixed size.
    #[error("protected-claim plaintext has an invalid length")]
    InvalidPlaintextLength,
}

fn derive_key(
    master_key: &ProtectedClaimKey,
    aad: &[u8],
) -> Result<Zeroizing<[u8; 32]>, ProtectedClaimError> {
    let hkdf = Hkdf::<Sha256>::new(Some(aad), &master_key.material);
    let mut key = Zeroizing::new([0_u8; 32]);
    let mut info = Vec::with_capacity(KEY_DOMAIN.len() + aad.len());
    info.extend_from_slice(KEY_DOMAIN);
    info.extend_from_slice(aad);
    hkdf.expand(&info, key.as_mut())
        .map_err(|_| ProtectedClaimError::KeyDerivation)?;
    Ok(key)
}

fn fingerprint(key_id: &str, nonce: &[u8; 24], ciphertext: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(FINGERPRINT_DOMAIN);
    hasher.update((key_id.len() as u64).to_be_bytes());
    hasher.update(key_id.as_bytes());
    hasher.update(nonce);
    hasher.update((ciphertext.len() as u64).to_be_bytes());
    hasher.update(ciphertext);
    hasher.finalize().into()
}

fn validate_key_id(key_id: &str) -> Result<(), ProtectedClaimError> {
    if key_id.is_empty() || key_id.len() > MAX_KEY_ID_BYTES {
        Err(ProtectedClaimError::InvalidKeyId)
    } else {
        Ok(())
    }
}

fn append_length_prefixed(encoded: &mut Vec<u8>, value: &[u8]) {
    let length = u64::try_from(value.len()).expect("in-memory slice length fits u64");
    encoded.extend_from_slice(&length.to_be_bytes());
    encoded.extend_from_slice(value);
}

const fn pair_tag(pair: Pair) -> u8 {
    match pair {
        Pair::Bitcoin => 0,
        Pair::Monero => 1,
        Pair::Zcash => 2,
    }
}

const fn direction_tag(direction: SwapDirection) -> u8 {
    match direction {
        SwapDirection::TakerSellsForeign => 0,
        SwapDirection::TakerSellsLez => 1,
    }
}

const fn participant_tag(participant: Participant) -> u8 {
    match participant {
        Participant::Maker => 0,
        Participant::Taker => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: [u8; 32] = [0x42; 32];
    const COMMITMENT: [u8; 32] = [0x17; 32];
    const NONCE: [u8; 24] = [0x23; 24];

    fn swap_id() -> SwapId {
        SwapId::new("protected-claim-test").expect("valid test swap ID")
    }

    fn key(material: u8) -> ProtectedClaimKey {
        ProtectedClaimKey::new("claim-key-2026-07", [material; 32]).expect("valid test key")
    }

    fn context(swap_id: &SwapId) -> ClaimMaterialContext<'_> {
        ClaimMaterialContext::new(
            PROTECTED_CLAIM_SCHEMA_V1,
            swap_id,
            Pair::Zcash,
            SwapDirection::TakerSellsForeign,
            &COMMITMENT,
            Participant::Taker,
            ClaimMaterialPurpose::LocalFirstClaim,
        )
    }

    fn fixture() -> (SwapId, ProtectedClaimKey, ProtectedClaimEnvelope) {
        let swap_id = swap_id();
        let key = key(0x31);
        let envelope = ProtectedClaimEnvelope::encrypt(
            &ClaimPreimage::new(SECRET),
            &key,
            NONCE,
            context(&swap_id),
        )
        .expect("encryption succeeds");
        (swap_id, key, envelope)
    }

    #[test]
    fn round_trip_keeps_only_record_safe_fields_serializable() {
        let (swap_id, key, envelope) = fixture();
        assert_ne!(envelope.ciphertext(), SECRET);
        assert_eq!(envelope.ciphertext().len(), PROTECTED_CLAIM_BYTES);

        let encoded = serde_json::to_vec(&envelope).expect("record-safe envelope serializes");
        assert!(!encoded.windows(SECRET.len()).any(|window| window == SECRET));
        let restored: ProtectedClaimEnvelope =
            serde_json::from_slice(&encoded).expect("record-safe envelope deserializes");
        let preimage = restored
            .decrypt(&key, context(&swap_id))
            .expect("canonical context authenticates");
        assert_eq!(preimage.expose_secret(), &SECRET);
    }

    #[test]
    fn every_authenticated_context_field_fails_closed_when_changed() {
        let (swap_id, key, envelope) = fixture();
        let other_swap = SwapId::new("other-swap").expect("valid test swap ID");
        let other_commitment = [0x18; 32];
        let mutations = [
            ClaimMaterialContext::new(
                2,
                &swap_id,
                Pair::Zcash,
                SwapDirection::TakerSellsForeign,
                &COMMITMENT,
                Participant::Taker,
                ClaimMaterialPurpose::LocalFirstClaim,
            ),
            ClaimMaterialContext::new(
                PROTECTED_CLAIM_SCHEMA_V1,
                &other_swap,
                Pair::Zcash,
                SwapDirection::TakerSellsForeign,
                &COMMITMENT,
                Participant::Taker,
                ClaimMaterialPurpose::LocalFirstClaim,
            ),
            ClaimMaterialContext::new(
                PROTECTED_CLAIM_SCHEMA_V1,
                &swap_id,
                Pair::Bitcoin,
                SwapDirection::TakerSellsForeign,
                &COMMITMENT,
                Participant::Taker,
                ClaimMaterialPurpose::LocalFirstClaim,
            ),
            ClaimMaterialContext::new(
                PROTECTED_CLAIM_SCHEMA_V1,
                &swap_id,
                Pair::Zcash,
                SwapDirection::TakerSellsLez,
                &COMMITMENT,
                Participant::Taker,
                ClaimMaterialPurpose::LocalFirstClaim,
            ),
            ClaimMaterialContext::new(
                PROTECTED_CLAIM_SCHEMA_V1,
                &swap_id,
                Pair::Zcash,
                SwapDirection::TakerSellsForeign,
                &other_commitment,
                Participant::Taker,
                ClaimMaterialPurpose::LocalFirstClaim,
            ),
            ClaimMaterialContext::new(
                PROTECTED_CLAIM_SCHEMA_V1,
                &swap_id,
                Pair::Zcash,
                SwapDirection::TakerSellsForeign,
                &COMMITMENT,
                Participant::Maker,
                ClaimMaterialPurpose::LocalFirstClaim,
            ),
            ClaimMaterialContext::new(
                PROTECTED_CLAIM_SCHEMA_V1,
                &swap_id,
                Pair::Zcash,
                SwapDirection::TakerSellsForeign,
                &COMMITMENT,
                Participant::Taker,
                ClaimMaterialPurpose::ObservedFollowUpClaim,
            ),
        ];

        for mutation in mutations {
            assert!(matches!(
                envelope.decrypt(&key, mutation),
                Err(ProtectedClaimError::Authentication)
            ));
        }
    }

    #[test]
    fn ciphertext_key_nonce_and_key_id_changes_fail_closed() {
        let (swap_id, _key, envelope) = fixture();

        let mut ciphertext = envelope.ciphertext.0;
        ciphertext[0] ^= 1;
        assert_eq!(
            ProtectedClaimEnvelope::from_record_fields(
                ciphertext,
                envelope.nonce,
                envelope.key_id.clone(),
                envelope.fingerprint,
            ),
            Err(ProtectedClaimError::FingerprintMismatch)
        );

        let wrong_material_key =
            ProtectedClaimKey::new("claim-key-2026-07", [0x32; 32]).expect("valid test key");
        assert!(matches!(
            envelope.decrypt(&wrong_material_key, context(&swap_id)),
            Err(ProtectedClaimError::Authentication)
        ));

        let mut nonce = envelope.nonce;
        nonce[0] ^= 1;
        assert_eq!(
            ProtectedClaimEnvelope::from_record_fields(
                envelope.ciphertext.0,
                nonce,
                envelope.key_id.clone(),
                envelope.fingerprint,
            ),
            Err(ProtectedClaimError::FingerprintMismatch)
        );

        let wrong_id_key =
            ProtectedClaimKey::new("rotated-key", [0x31; 32]).expect("valid test key");
        assert!(matches!(
            envelope.decrypt(&wrong_id_key, context(&swap_id)),
            Err(ProtectedClaimError::KeyIdMismatch)
        ));
    }

    #[test]
    fn deserialization_revalidates_fingerprint_bounds_and_unknown_fields() {
        let (_, _, envelope) = fixture();
        let canonical = serde_json::to_value(&envelope).expect("envelope serializes");

        let mut unknown = canonical.clone();
        unknown
            .as_object_mut()
            .expect("envelope is a map")
            .insert("plaintext".to_owned(), serde_json::json!(SECRET));
        assert!(serde_json::from_value::<ProtectedClaimEnvelope>(unknown).is_err());

        let mut corrupt = canonical.clone();
        let fingerprint_byte = corrupt["fingerprint"][0]
            .as_u64()
            .expect("fingerprint byte is numeric");
        corrupt["fingerprint"][0] = serde_json::json!(fingerprint_byte ^ 1);
        assert!(serde_json::from_value::<ProtectedClaimEnvelope>(corrupt).is_err());

        let mut oversized = canonical;
        oversized["ciphertext"]
            .as_array_mut()
            .expect("ciphertext is an array")
            .push(serde_json::json!(0));
        assert!(serde_json::from_value::<ProtectedClaimEnvelope>(oversized).is_err());
    }

    #[test]
    fn debug_representations_redact_key_ciphertext_nonce_and_fingerprint() {
        let (_, key, envelope) = fixture();
        let key_debug = format!("{key:?}");
        let envelope_debug = format!("{envelope:?}");

        assert!(key_debug.contains("[REDACTED]"));
        assert!(!key_debug.contains(&hex::encode([0x31; 32])));
        assert!(envelope_debug.matches("[REDACTED]").count() >= 2);
        assert!(envelope_debug.contains("[REDACTED; 48 bytes]"));
        assert!(!envelope_debug.contains(&hex::encode(envelope.ciphertext())));
        assert!(!envelope_debug.contains(&hex::encode(NONCE)));
        assert!(!envelope_debug.contains(&hex::encode(envelope.fingerprint())));
    }
}
